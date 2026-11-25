//! Dispatch the shared offline editing routes to each record's keeper.

use super::*;
use crate::edits::{Edit, Target};
use std::{future::Future, pin::Pin};

pub enum RecordEditCall {
    Read(Value),
    Submit(Value),
    Outbox,
    Receipt(String),
    /// Rename `key` on the released Mac that keeps it (`mac`).
    ReleasedRename {
        mac: [u8; 32],
        computer: Value,
        named: String,
        key: String,
        name: String,
    },
}

pub struct RecordEditAnswer {
    pub status: u16,
    pub value: Value,
}

pub type RecordEditFuture =
    Pin<Box<dyn Future<Output = Result<Option<RecordEditAnswer>, Refusal>> + Send>>;

pub trait RecordEdits: Send + Sync {
    fn perform(&self, caller: Caller, call: RecordEditCall) -> RecordEditFuture;
}

async fn other(
    handler: Option<Arc<dyn RecordEdits>>,
    caller: Caller,
    call: RecordEditCall,
) -> Result<Option<Response>, Refusal> {
    let Some(handler) = handler else {
        return Ok(None);
    };
    Ok(handler.perform(caller, call).await?.map(|answer| Response {
        status: answer.status,
        value: answer.value,
    }))
}

impl SetupHandle {
    pub(super) fn register_edits(
        &self,
        routes: &mut Registry,
        records: Option<Arc<dyn RecordEdits>>,
    ) {
        let setup = self.clone();
        let handler = records.clone();
        routes.register::<get_kuvo_data_edits::Spec, _, _>(Access::DesktopOrPaired, move |call| {
            let setup = setup.clone();
            let handler = handler.clone();
            async move {
                submitter(&call.request.caller)?;
                let query = call.request.path.split_once('?').map(|(_, query)| query);
                let url = reqwest::Url::parse(&format!(
                    "http://localhost/?{}",
                    query.unwrap_or_default()
                ))
                .map_err(|_| Refusal::new(StatusCode::BAD_REQUEST, "Name the record to edit."))?;
                let query: serde_json::Map<String, Value> = url
                    .query_pairs()
                    .map(|(name, value)| (name.into_owned(), value.into_owned().into()))
                    .collect();
                if let Some(response) = other(
                    handler,
                    call.request.caller,
                    RecordEditCall::Read(Value::Object(query.clone())),
                )
                .await?
                {
                    return respond(response);
                }
                let kind = query.get("kind");
                let target: Target =
                    serde_json::from_value(json!({"kind": kind})).map_err(|_| {
                        Refusal::new(StatusCode::BAD_REQUEST, "Name the record to edit.")
                    })?;
                respond(setup.request(Command::ReadEdit(target)).await?)
            }
        });

        let setup = self.clone();
        let handler = records.clone();
        routes.register::<post_kuvo_data_edits::Spec, _, _>(Access::DesktopOrPaired, move |call| {
            let setup = setup.clone();
            let handler = handler.clone();
            async move {
                let by = submitter(&call.request.caller)?;
                if call.request.body.len() > 512 * 1024 {
                    return Err(Refusal::new(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "This draft is too large to send to the owner.",
                    ));
                }
                let body: Value = read(&call.request.body)?;
                let answer = if let Some(response) = other(
                    handler,
                    call.request.caller,
                    RecordEditCall::Submit(body.clone()),
                )
                .await?
                {
                    response
                } else {
                    let edit = Edit::read(body).map_err(refusal)?;
                    setup.request(Command::SubmitEdit(edit, by)).await?
                };
                if answer.status == 202 {
                    typed::<post_kuvo_data_edits::Spec, 202>(answer.value)
                } else {
                    respond(answer)
                }
            }
        });

        let setup = self.clone();
        let handler = records.clone();
        routes.register::<get_kuvo_data_edits_outbox::Spec, _, _>(
            Access::DesktopOrPaired,
            move |call| {
                let setup = setup.clone();
                let handler = handler.clone();
                async move {
                    submitter(&call.request.caller)?;
                    let mut outbox = setup.request(Command::EditOutbox).await?;
                    if let Some(response) =
                        other(handler, call.request.caller, RecordEditCall::Outbox).await?
                    {
                        if outbox.status != 200 || response.status != 200 {
                            return respond(if outbox.status == 200 {
                                response
                            } else {
                                outbox
                            });
                        }
                        if let Some(entries) = outbox.value["entries"].as_array_mut()
                            && let Some(extra) = response.value["entries"].as_array()
                        {
                            entries.extend(extra.iter().cloned());
                        }
                    }
                    respond(outbox)
                }
            },
        );

        let setup = self.clone();
        routes.register::<get_kuvo_data_edits_operation::Spec, _, _>(
            Access::DesktopOrPaired,
            move |call| {
                let setup = setup.clone();
                let handler = records.clone();
                async move {
                    submitter(&call.request.caller)?;
                    let operation = decoded_id(call.param("operation"))?;
                    if let Some(response) = other(
                        handler,
                        call.request.caller,
                        RecordEditCall::Receipt(operation.clone()),
                    )
                    .await?
                    {
                        return respond(response);
                    }
                    respond(setup.request(Command::EditReceipt(operation)).await?)
                }
            },
        );
    }
}
