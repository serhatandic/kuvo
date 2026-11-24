//! Plan a label library restore against the accepted setup revision.

use super::*;
use crate::queue::forward::Forward;
use kuvo_peers::{
    memory_read::{Answer, Handle, Kind, Read, Standing},
    setup_lists::Reader,
};
use kuvo_protocol::routes::{
    get_kuvo_work_queue_labels_restore as preview, post_kuvo_work_queue_labels_restore as apply,
};
use kuvo_server::{LocalOrigin, Transport};
use serde::Serialize;

const MOST_LABELS: usize = 60;

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub(super) struct Plan {
    pub restored: usize,
    already_present: usize,
    conflicts: usize,
    details: Vec<Conflict>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    provisional: bool,
}

#[derive(Serialize)]
struct Conflict {
    id: String,
    reason: &'static str,
}

pub(super) struct Restore {
    wanted: Vec<QueueLabel>,
    force: bool,
}

impl Restore {
    pub fn read(request: &Value) -> Result<Self, Refusal> {
        let force = match request.get("force") {
            None => false,
            Some(value) => value
                .as_bool()
                .ok_or_else(|| bad("force must be true or false.".into()))?,
        };
        let library = &request["library"];
        let given = library.get("labels").unwrap_or(library);
        let rows = given
            .as_array()
            .filter(|rows| rows.len() <= MOST_LABELS)
            .ok_or_else(|| {
                bad(format!(
                    "Give a saved list of up to {MOST_LABELS} label definitions."
                ))
            })?;
        let mut seen = HashSet::new();
        let mut wanted = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let label = row["id"]
                .as_str()
                .zip(row["name"].as_str())
                .zip(row["color"].as_str())
                .and_then(|((id, name), color)| clean_label(id, name, color))
                .filter(|label| row["name"] == label.name && row["color"] == label.color)
                .filter(|label| seen.insert(label.id.clone()))
                .ok_or_else(|| {
                    bad(format!(
                        "Invalid or duplicate label at position {} in the saved list.",
                        index + 1
                    ))
                })?;
            wanted.push(label);
        }
        Ok(Self { wanted, force })
    }

    pub fn plan(self, previous: QueueLabelLibrary, provisional: bool) -> (QueueLabelLibrary, Plan) {
        let mut next = previous;
        let mut plan = Plan {
            provisional,
            ..Plan::default()
        };
        for label in self.wanted {
            let existing = next.labels.iter().find(|each| each.id == label.id);
            if existing.is_some_and(|each| each.name == label.name && each.color == label.color) {
                plan.already_present += 1;
                continue;
            }
            let reason = if existing.is_some() {
                Some("This ID already has a different name or color.")
            } else if !self.force && next.removed.contains(&label.id) {
                Some("This label was removed. Use --force to restore it.")
            } else if named(&next.labels, &Value::String(label.name.clone())).is_some() {
                Some("Another label already uses this name.")
            } else if next.labels.len() >= MOST_LABELS {
                Some("The label library is full.")
            } else {
                None
            };
            if let Some(reason) = reason {
                plan.details.push(Conflict {
                    id: label.id,
                    reason,
                });
            } else {
                next.removed.retain(|id| id != &label.id);
                next.labels.push(label);
                plan.restored += 1;
            }
        }
        plan.conflicts = plan.details.len();
        (next, plan)
    }
}
pub const ROUTE: &str = "kuvo/work-queue/labels/restore";
const LEADERSHIP_WAIT: &str =
    "Kuvo's relay can't be reached. Setup changes will resume when it reconnects.";
const PREVIEW: &str = "work-queue.labels.restore.preview";
const APPLY: &str = "work-queue.labels.restore";

#[derive(Clone)]
pub(in crate::queue) struct Restores {
    store: Store,
    setup: SetupHandle,
    reads: Handle,
    peers: Reader,
    forward: Forward,
}

impl Restores {
    pub fn new(
        store: Store,
        setup: SetupHandle,
        reads: Handle,
        peers: Reader,
        forward: Forward,
    ) -> Self {
        Self {
            store,
            setup,
            reads,
            peers,
            forward,
        }
    }

    pub fn register(&self, registry: &mut Registry) {
        use crate::queue::routes::typed;
        let service = self.clone();
        registry.register::<preview::Spec, _, _>(Access::Any, move |call| {
            let service = service.clone();
            async move {
                typed(
                    service
                        .answer_client(call.request.caller, call.request.body.as_ref(), true)
                        .await?,
                )
            }
        });
        let service = self.clone();
        registry.register::<apply::Spec, _, _>(Access::Any, move |call| {
            let service = service.clone();
            async move {
                typed(
                    service
                        .answer_client(call.request.caller, call.request.body.as_ref(), false)
                        .await?,
                )
            }
        });
    }

    pub async fn answer_peer(&self, read: Read) -> Answer {
        if read.item != ROUTE || read.kind != Kind::Record {
            return Answer {
                status: 404,
                value: json!({"message":"There is no such read."}),
            };
        }
        let result = self
            .here(Value::Object(read.body.unwrap_or_default()), true)
            .await;
        match result {
            Ok(value) => Answer { status: 200, value },
            Err(error) => into_answer(error).await,
        }
    }

    async fn answer_client(
        &self,
        caller: Caller,
        bytes: &[u8],
        dry_run: bool,
    ) -> Result<Value, Refusal> {
        if !matches!(
            caller,
            Caller::Local(Transport::Http {
                origin: LocalOrigin::VaultHolder,
                ..
            })
        ) {
            return Err(Refusal::new(
                StatusCode::FORBIDDEN,
                "Run this from the box's labels restore command.",
            ));
        }
        let request: Value = serde_json::from_slice(bytes).unwrap_or_default();
        if request
            .get("force")
            .is_some_and(|force| !force.is_boolean())
        {
            return Err(bad("force must be true or false.".into()));
        }
        if !dry_run && request.get("dryRun").is_some_and(|value| value != false) {
            return Err(bad("Update the box and retry the preview.".into()));
        }
        let Some(setup) = self.setup.queue_setup().filter(|setup| !setup.main_here) else {
            return self.here(request, dry_run).await;
        };
        let primary = kuvo_wire::keys::decode_exact(&setup.primary).ok_or_else(no_main)?;
        let feature = if dry_run { PREVIEW } else { APPLY };
        match self.reads.standing(primary, feature) {
            Standing::Ready => {}
            Standing::Away => return Err(no_main()),
            Standing::Unavailable => {
                return Err(Refusal::new(
                    StatusCode::CONFLICT,
                    if dry_run {
                        "Update the box on the main computer before previewing a restore from another computer."
                    } else {
                        "Update the box on the main computer before restoring labels from another computer."
                    },
                ));
            }
        }
        if !dry_run {
            let view = self.peers.view();
            let computer = view.roster.member(&primary).ok_or_else(no_main)?;
            let location = json!({"key":setup.primary,"name":computer.name,"kind":computer.kind});
            return self
                .forward
                .owner_write(primary, location, ROUTE, request)
                .await;
        }
        let body = request.as_object().cloned();
        let read = Read {
            area: Some("ownership".into()),
            item: ROUTE.into(),
            kind: Kind::Record,
            body,
        };
        let answer = self
            .reads
            .request(primary, read)
            .await
            .map_err(|_| no_main())?;
        if answer.status == 200 {
            Ok(answer.value)
        } else {
            Err(Refusal::json(
                StatusCode::from_u16(answer.status).unwrap_or(StatusCode::CONFLICT),
                answer.value,
            ))
        }
    }

    pub(in crate::queue) async fn here(
        &self,
        value: Value,
        dry_run: bool,
    ) -> Result<Value, Refusal> {
        let restore = Restore::read(&value)?;
        let Some(setup) = self.setup.queue_setup() else {
            return Err(Refusal::new(StatusCode::CONFLICT, LEADERSHIP_WAIT));
        };
        if !setup.main_here || !self.setup.may_save_queue_labels().await {
            return Err(Refusal::new(StatusCode::CONFLICT, LEADERSHIP_WAIT));
        }
        let store = self.store.clone();
        let accepted = setup.clone();
        let (library, plan) = super::super::routes::blocking(move || {
            let provisional = dry_run && accepted.labels.is_none();
            let library = match accepted.labels {
                Some(accepted) => accepted.library,
                None => local(&store, Some(&accepted))?.library,
            };
            Ok(restore.plan(library, provisional))
        })
        .await?;
        let save = if !dry_run && plan.restored > 0 {
            Some(
                self.setup
                    .save_queue_labels_on_this_computer(&setup, library)
                    .await?,
            )
        } else {
            None
        };
        let mut answer = json!(plan);
        if let Some(save) = save {
            answer["save"] = json!(save);
        }
        Ok(answer)
    }
}

fn no_main() -> Refusal {
    Refusal::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "This box can't reach the main computer.",
    )
}

async fn into_answer(error: Refusal) -> Answer {
    use http_body_util::BodyExt;
    let reply = error.reply();
    let status = reply.status.as_u16();
    let value = reply
        .body
        .collect()
        .await
        .ok()
        .and_then(|body| serde_json::from_slice(&body.to_bytes()).ok())
        .unwrap_or_else(|| json!({"message":"The label library couldn't be read."}));
    Answer { status, value }
}
