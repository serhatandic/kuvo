mod record_edits;

pub use record_edits::{RecordEditAnswer, RecordEditCall, RecordEditFuture, RecordEdits};

use std::{io, sync::Arc};

use http::StatusCode;
use kuvo_protocol::{
    JsonStatus, Route, RouteSpec,
    routes::*,
    shape::{Primitive, Shape},
};
use kuvo_server::{
    Access, Caller, JsonLine, LocalOrigin, Refusal, Registry, RouteReply, Transport,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{oneshot, watch};

use crate::{
    Error, SetupHandle,
    actor::{Command, Request, Response, Unlink, View, computer_requests::ComputerRequest},
    edits::Submitter,
    preferences::PersonalityChange,
    seed::source::Selection,
};

impl SetupHandle {
    pub fn register(&self, routes: &mut Registry) {
        self.register_with_record_edits(routes, None);
    }

    pub fn register_with_record_edits(
        &self,
        routes: &mut Registry,
        records: Option<Arc<dyn RecordEdits>>,
    ) {
        let setup = self.clone();
        routes.register::<get_kuvo_setup::Spec, _, _>(Access::Any, move |_| {
            let answer = typed::<get_kuvo_setup::Spec, 200>(setup.views.borrow().setup.clone());
            std::future::ready(answer)
        });

        let setup = self.clone();
        routes.register::<get_kuvo_setup_events::Spec, _, _>(Access::Any, move |_| {
            std::future::ready(Ok(events(setup.views.clone(), |view| {
                Some(view.setup.clone())
            })))
        });

        let setup = self.clone();
        routes.register::<post_kuvo_setup_welcomed::Spec, _, _>(Access::Any, move |call| {
            let setup = setup.clone();
            async move {
                let given: Welcome = read(&call.request.body)?;
                respond(setup.request(Command::Welcome(given.welcomed)).await?)
            }
        });

        let setup = self.clone();
        routes.register::<post_kuvo_setup_judge::Spec, _, _>(Access::Any, move |call| {
            let setup = setup.clone();
            async move {
                let body =
                    serde_json::from_slice::<Value>(&call.request.body).unwrap_or(Value::Null);
                let probe = body["probe"]
                    .as_str()
                    .filter(|text| text.encode_utf16().count() <= 200_000)
                    .ok_or_else(|| {
                        Refusal::new(
                            StatusCode::BAD_REQUEST,
                            "Send what the probe printed, as probe.",
                        )
                    })?;
                let verdict = crate::discovery::judge(probe, &setup.views.borrow().setup).map_err(
                    |message| {
                        Refusal::json(
                            StatusCode::CONFLICT,
                            json!({"code": "unsupportedFormatVersion", "message": message}),
                        )
                    },
                )?;
                typed::<post_kuvo_setup_judge::Spec, 200>(json!(verdict))
            }
        });

        let setup = self.clone();
        routes.register::<post_kuvo_setup_attached::Spec, _, _>(Access::Local, move |call| {
            let setup = setup.clone();
            async move {
                if matches!(
                    call.request.caller,
                    Caller::Local(Transport::Http {
                        origin: LocalOrigin::Direct,
                        ..
                    })
                ) {
                    return Err(Refusal::new(
                        StatusCode::FORBIDDEN,
                        "Only Kuvo Code on this computer says it attached.",
                    ));
                }
                respond(setup.request(Command::Attached).await?)
            }
        });

        let setup = self.clone();
        routes.register::<post_kuvo_setup_handover::Spec, _, _>(
            Access::DesktopOrPaired,
            move |call| {
                let setup = setup.clone();
                async move {
                    let given: Handover = read(&call.request.body)?;
                    if kuvo_wire::keys::decode_exact::<32>(&given.primary).is_none() {
                        return Err(Refusal::new(
                            StatusCode::BAD_REQUEST,
                            "Choose a computer by its key as the main computer.",
                        ));
                    }
                    if given.unlink.is_some() {
                        return Err(Refusal::new(
                            StatusCode::CONFLICT,
                            "Unlinking a computer is unavailable.",
                        ));
                    }
                    let submitter = match &call.request.caller {
                        Caller::Local(Transport::Ssh { .. }) => Submitter::Computer,
                        caller => submitter(caller)?,
                    };
                    let answer = setup
                        .request(Command::Handover {
                            primary: given.primary,
                            epoch: given.epoch,
                            submitter,
                        })
                        .await?;
                    if answer.status == 202 {
                        typed::<post_kuvo_setup_handover::Spec, 202>(answer.value)
                    } else {
                        respond(answer)
                    }
                }
            },
        );

        let setup = self.clone();
        routes.register::<SaveLocal, _, _>(Access::Internal, move |call| {
            let setup = setup.clone();
            async move {
                let body = call.body;
                if body.kind != crate::SharedKind::SetupUsageDisplay {
                    return Err(Refusal::json(StatusCode::BAD_REQUEST, json!({})));
                }
                respond(
                    setup
                        .request(Command::Usage {
                            id: body.id,
                            epoch: body.epoch,
                            sequence: body.sequence,
                            operation: body.operation,
                            value: body.value,
                        })
                        .await?,
                )
            }
        });
        let setup = self.clone();
        routes.register::<get_kuvo_computers_add_credentials::Spec, _, _>(
            Access::DesktopOrPaired,
            move |call| {
                let setup = setup.clone();
                async move {
                    let by = submitter(&call.request.caller)?;
                    let answer = setup
                        .request(Command::ComputerViaMain(
                            crate::actor::computer_requests::ComputerRequest::Checklist,
                            by,
                        ))
                        .await?;
                    if answer.status == 200 {
                        typed::<get_kuvo_computers_add_credentials::Spec, 200>(answer.value)
                    } else {
                        Err(response_refusal(answer))
                    }
                }
            },
        );
        let setup = self.clone();
        routes.register::<post_kuvo_computers_add_code::Spec, _, _>(
            Access::DesktopOrPaired,
            move |call| {
                let setup = setup.clone();
                async move {
                    let by = submitter(&call.request.caller)?;
                    let credentials = serde_json::from_slice::<Value>(&call.request.body)
                        .ok()
                        .and_then(|mut given| given.as_object_mut()?.shift_remove("credentials"));
                    let selection = Selection::read(credentials).ok_or_else(|| {
                        Refusal::new(
                            StatusCode::BAD_REQUEST,
                            "Choose sign-ins and keys by their checklist IDs.",
                        )
                    })?;
                    let answer = setup
                        .installer_on_main(selection, by)
                        .await
                        .map_err(refusal)?;
                    if answer.status == 201 {
                        typed::<post_kuvo_computers_add_code::Spec, 201>(answer.value)
                    } else {
                        Err(response_refusal(answer))
                    }
                }
            },
        );
        let setup = self.clone();
        routes.register::<InstallerRecovery, _, _>(Access::Any, move |call| {
            let setup = setup.clone();
            async move {
                if !matches!(
                    call.request.caller,
                    Caller::Local(Transport::Http {
                        origin: LocalOrigin::App | LocalOrigin::Mac,
                        ..
                    })
                ) {
                    return Err(Refusal::new(
                        StatusCode::FORBIDDEN,
                        "Apply the receipt directly on the setup’s main computer.",
                    ));
                }
                let given =
                    serde_json::from_slice::<Value>(&call.request.body).unwrap_or(Value::Null);
                let receipt = given["receipt"].as_str().ok_or_else(|| {
                    Refusal::new(
                        StatusCode::BAD_REQUEST,
                        "Enter an installer cancellation receipt.",
                    )
                })?;
                let receipt = crate::installer::Cancellation::read(receipt)
                    .map_err(|error| Refusal::new(StatusCode::CONFLICT, error.to_string()))?;
                respond(
                    setup
                        .request(Command::InstallerCancellation(receipt))
                        .await?,
                )
            }
        });
        let setup = self.clone();
        routes.register::<patch_kuvo_computers_key::Spec, _, _>(
            Access::DesktopOrPaired,
            move |call| {
                let setup = setup.clone();
                async move {
                    let by = submitter(&call.request.caller)?;
                    let key = call.param("key").unwrap_or_default().to_owned();
                    let name = serde_json::from_slice::<Value>(&call.request.body)
                        .ok()
                        .and_then(|body| body["name"].as_str().map(ToOwned::to_owned))
                        .unwrap_or_default();
                    let answer = setup
                        .request(Command::RenameComputer(key, name, by))
                        .await?;
                    if answer.status == 200 {
                        typed::<patch_kuvo_computers_key::Spec, 200>(answer.value)
                    } else {
                        respond(answer)
                    }
                }
            },
        );
        self.register_unlink::<delete_kuvo_computers_key::Spec>(routes, None);
        self.register_unlink::<post_kuvo_computers_key_unlink::Spec>(routes, Some(false));
        self.register_unlink::<post_kuvo_computers_key_unlink_and_wipe::Spec>(routes, Some(true));
        self.register_edits(routes, records);
        self.register_personalities(routes);
    }

    fn register_unlink<R>(&self, routes: &mut Registry, wipe: Option<bool>)
    where
        R: RouteSpec<Body = ()> + JsonStatus<200> + Send + Sync + 'static,
        <R as JsonStatus<200>>::Body: serde::de::DeserializeOwned,
    {
        let setup = self.clone();
        routes.register::<R, _, _>(Access::DesktopOrPaired, move |call| {
            let setup = setup.clone();
            async move {
                let by = submitter(&call.request.caller)?;
                let key = call.param("key").unwrap_or_default().to_owned();
                let body = &call.request.body;
                let mut request = if body.is_empty() {
                    Unlink::default()
                } else {
                    serde_json::from_slice::<Option<Unlink>>(body)
                        .map_err(|_| {
                            Refusal::new(StatusCode::BAD_REQUEST, "Send a valid removal request.")
                        })?
                        .unwrap_or_default()
                };
                request.on_route(wipe);
                respond(
                    setup
                        .request(Command::ComputerViaMain(
                            ComputerRequest::Unlink { key, request },
                            by,
                        ))
                        .await?,
                )
            }
        });
    }

    pub(crate) async fn request(&self, command: Command) -> Result<Response, Refusal> {
        self.request_command(command).await.map_err(refusal)
    }

    pub(crate) async fn request_command(&self, command: Command) -> Result<Response, Error> {
        let (reply, result) = oneshot::channel();
        self.inbox
            .send(Request { command, reply })
            .await
            .map_err(|_| Error::SetupUnavailable)?;
        result.await.map_err(|_| Error::SetupUnavailable)?
    }

    fn register_personalities(&self, routes: &mut Registry) {
        let setup = self.clone();
        routes.register::<get_kuvo_personalities::Spec, _, _>(Access::Any, move |_| {
            std::future::ready(setup.personality_document().and_then(|document| {
                typed::<get_kuvo_personalities::Spec, 200>(json!({
                    "personalities": document["personalities"],
                }))
            }))
        });

        let setup = self.clone();
        routes.register::<get_kuvo_personalities_events::Spec, _, _>(Access::Any, move |_| {
            std::future::ready(setup.personality_document().map(|_| {
                events(setup.views.clone(), |view| {
                    view.personalities
                        .as_ref()
                        .ok()
                        .map(|document| json!({"personalities": document["personalities"]}))
                })
            }))
        });

        let setup = self.clone();
        routes.register::<get_kuvo_personalities_document::Spec, _, _>(Access::Any, move |call| {
            std::future::ready(personality_app(&call.request.caller).and_then(|()| {
                setup
                    .personality_document()
                    .and_then(typed::<get_kuvo_personalities_document::Spec, 200>)
            }))
        });

        let setup = self.clone();
        routes.register::<post_kuvo_personalities::Spec, _, _>(Access::Any, move |call| {
            let setup = setup.clone();
            async move {
                setup.require_primary("personalities")?;
                let body = read(&call.request.body)?;
                respond(
                    setup
                        .request(Command::Personalities(PersonalityChange::Create(body)))
                        .await?,
                )
            }
        });

        let setup = self.clone();
        routes.register::<post_kuvo_personalities_take::Spec, _, _>(Access::Any, move |call| {
            let setup = setup.clone();
            async move {
                personality_app(&call.request.caller)?;
                setup.require_primary("personalities")?;
                let body: Value = read(&call.request.body)?;
                respond(
                    setup
                        .request(Command::Personalities(PersonalityChange::Merge(
                            body["document"].clone(),
                        )))
                        .await?,
                )
            }
        });

        let setup = self.clone();
        routes.register::<patch_kuvo_personalities_id::Spec, _, _>(Access::Any, move |call| {
            let setup = setup.clone();
            async move {
                setup.require_primary("personalities")?;
                let id = decoded_id(call.param("id"))?;
                let body = read(&call.request.body)?;
                respond(
                    setup
                        .request(Command::Personalities(PersonalityChange::Patch(id, body)))
                        .await?,
                )
            }
        });

        let setup = self.clone();
        routes.register::<delete_kuvo_personalities_id::Spec, _, _>(Access::Any, move |call| {
            let setup = setup.clone();
            async move {
                setup.require_primary("personalities")?;
                let id = decoded_id(call.param("id"))?;
                respond(
                    setup
                        .request(Command::Personalities(PersonalityChange::Delete(id)))
                        .await?,
                )
            }
        });
    }

    fn personality_document(&self) -> Result<Value, Refusal> {
        self.views
            .borrow()
            .personalities
            .clone()
            .map_err(|version| refusal(Error::PersonalityVersion(version)))
    }

    /// The clock page is setup-owned; the settings framework serves it with other pages.
    pub fn clock_page(&self) -> Value {
        self.views.borrow().clock.clone()
    }

    pub async fn change_clock(&self, zone: String, revision: String) -> Result<Value, Refusal> {
        let answer = self
            .request(Command::Zone {
                zone,
                revision: Some(revision),
            })
            .await?;
        if answer.status == 200 {
            Ok(answer.value)
        } else {
            Err(response_refusal(answer))
        }
    }

    fn require_primary(&self, setting: &str) -> Result<(), Refusal> {
        if self.views.borrow().setup["you"]["role"] == "member" {
            return Err(Refusal::new(
                StatusCode::CONFLICT,
                format!("Change your setup's {setting} on its main computer."),
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct Welcome {
    welcomed: bool,
}

#[derive(Deserialize)]
struct Handover {
    primary: String,
    epoch: Option<u64>,
    unlink: Option<Value>,
}

/// The holder's local save lane is internal and does not appear in the public route export.
struct SaveLocal;
impl RouteSpec for SaveLocal {
    type Body = Usage;
    const ROUTE: &'static Route = &Route {
        method: "POST",
        path: "kuvo/setup/save-local",
        feature: "",
        answers: &[(200, &Shape::Primitive(Primitive::Any))],
        body: Some(&Shape::Primitive(Primitive::Any)),
    };
}
impl JsonStatus<200> for SaveLocal {
    type Body = crate::Receipt;
}

struct InstallerRecovery;
impl RouteSpec for InstallerRecovery {
    type Body = Value;
    const ROUTE: &'static Route = &Route {
        method: "POST",
        path: crate::installer::RECOVERY_ROUTE,
        feature: "",
        answers: &[(200, &Shape::Primitive(Primitive::Any))],
        body: Some(&Shape::Primitive(Primitive::Any)),
    };
}
impl JsonStatus<200> for InstallerRecovery {
    type Body = Value;
}

#[derive(Deserialize)]
struct Usage {
    kind: crate::SharedKind,
    id: String,
    epoch: u64,
    sequence: u64,
    operation: Option<String>,
    value: Value,
}

fn personality_app(caller: &Caller) -> Result<(), Refusal> {
    if matches!(
        caller,
        Caller::Local(Transport::Http {
            origin: LocalOrigin::App,
            ..
        })
    ) {
        Ok(())
    } else {
        Err(Refusal::new(
            StatusCode::FORBIDDEN,
            "Only Kuvo Code on this computer can do that.",
        ))
    }
}

pub(crate) fn submitter(caller: &Caller) -> Result<Submitter, Refusal> {
    match caller {
        Caller::Local(Transport::Http {
            origin: LocalOrigin::App | LocalOrigin::Mac,
            ..
        }) => Ok(Submitter::Computer),
        Caller::Paired { pairing, .. } if pairing.is_current() => Ok(Submitter::Paired {
            key: kuvo_wire::keys::encode(&pairing.key),
            pairing: pairing.id.clone(),
        }),
        _ => Err(Refusal::new(
            StatusCode::FORBIDDEN,
            "Open this from a paired Kuvo Code client.",
        )),
    }
}

fn read<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, Refusal> {
    serde_json::from_slice(body)
        .map_err(|_| Refusal::new(StatusCode::BAD_REQUEST, "Send a valid request body."))
}

fn unavailable() -> Refusal {
    Refusal::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "This computer couldn't read its setup.",
    )
}

fn refusal(error: Error) -> Refusal {
    if matches!(error, Error::PersonalityVersion(_)) {
        return Refusal::json(
            StatusCode::CONFLICT,
            json!({
                "code": "unsupportedFormatVersion", "message": error.to_string(),
            }),
        );
    }
    let (status, message) = match error {
        Error::Conflict => (
            StatusCode::CONFLICT,
            "The setup changed before this could be saved.".into(),
        ),
        Error::Fenced => (StatusCode::SERVICE_UNAVAILABLE, error.to_string()),
        Error::Invalid("time zone") => (
            StatusCode::BAD_REQUEST,
            "This computer doesn't know that time zone.".into(),
        ),
        Error::Invalid(_) => (StatusCode::BAD_REQUEST, error.to_string()),
        Error::Capacity => (StatusCode::CONFLICT, error.to_string()),
        Error::SetupUnavailable => return unavailable(),
        _ => {
            eprintln!("setup route: {error}");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "This computer couldn't save the setup change.".into(),
            )
        }
    };
    Refusal::new(status, message)
}

fn decoded_id(given: Option<&str>) -> Result<String, Refusal> {
    percent_encoding::percent_decode_str(given.unwrap_or_default())
        .decode_utf8()
        .map(|id| id.into_owned())
        .map_err(|_| Refusal::json(StatusCode::NOT_FOUND, json!({})))
}

pub(crate) fn typed<R, const STATUS: u16>(value: Value) -> Result<RouteReply<R>, Refusal>
where
    R: JsonStatus<STATUS>,
    <R as JsonStatus<STATUS>>::Body: serde::de::DeserializeOwned,
{
    let mut value = value;
    let mut fields = Vec::new();
    if value.get("proof").is_some()
        && R::ROUTE.path != SaveLocal::ROUTE.path
        && let Some(value) = value.as_object_mut()
    {
        for name in ["setup", "epoch", "operation", "proof", "status"] {
            if let Some(field) = value.shift_remove(name) {
                fields.push((name, field));
            }
        }
    }
    let body = serde_json::from_value(value).map_err(|error| {
        eprintln!("encoding setup route: {error}");
        Refusal::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "This computer couldn't describe its setup.",
        )
    })?;
    RouteReply::json_with::<STATUS>(body, fields).map_err(|_| unavailable())
}

fn respond<R>(response: Response) -> Result<RouteReply<R>, Refusal>
where
    R: JsonStatus<200>,
    <R as JsonStatus<200>>::Body: serde::de::DeserializeOwned,
{
    if response.status == 200 {
        typed::<R, 200>(response.value)
    } else {
        Err(response_refusal(response))
    }
}

pub(crate) fn response_refusal(response: Response) -> Refusal {
    Refusal::json(
        StatusCode::from_u16(response.status).expect("setup response status"),
        response.value,
    )
}

fn events<R>(
    mut views: watch::Receiver<View>,
    project: impl Fn(&View) -> Option<Value> + Send + 'static,
) -> RouteReply<R>
where
    R: JsonStatus<200>,
    <R as JsonStatus<200>>::Body: serde::de::DeserializeOwned,
{
    RouteReply::ndjson::<200>(async_stream::stream! {
        let mut previous = None;
        loop {
            let value = project(&views.borrow_and_update());
            if let Some(value) = value && previous.as_ref() != Some(&value) {
                let line = serde_json::from_value(value.clone()).map_err(io::Error::other).and_then(JsonLine::<R, 200>::new);
                previous = Some(value);
                yield line;
            }
            if views.changed().await.is_err() {
                break;
            }
        }
    })
}
