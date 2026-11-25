//! Setup-wide stars use the main computer's signed shared value.

use std::time::Duration;

use http::StatusCode;
use kuvo_server::{Caller, Refusal};
use kuvo_wire::keys::encode;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::{sync::oneshot, time::Instant};

use super::{Command, Owner, Response, SetupHandle};
use crate::{
    Error, SharedKind,
    edits::Submitter,
    model::{Role, fingerprint, safe_token},
    random,
};

const KIND: SharedKind = SharedKind::SetupStarredModels;
const MAX: usize = 200;
const WAIT: Duration = Duration::from_secs(12);
const SEED: &str = "setup-seed:";
const FIELDS: &str = "setup-seed:fields:";

#[derive(Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct ModelStar {
    #[serde(rename = "providerID")]
    pub provider: String,
    pub id: String,
}

impl ModelStar {
    fn valid(&self) -> bool {
        [&self.provider, &self.id]
            .into_iter()
            .all(|field| (1..=300).contains(&field.encode_utf16().count()))
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Change {
    model: ModelStar,
    starred: bool,
    by: Submitter,
}

#[derive(Deserialize, Serialize)]
struct Stars {
    starred: Vec<ModelStar>,
}

pub(super) struct Pending {
    primary: String,
    epoch: u64,
    expires: Instant,
    reply: oneshot::Sender<Result<Response, Error>>,
}

fn waiting() -> Refusal {
    Refusal::new(
        StatusCode::CONFLICT,
        "Your setup's main computer keeps your starred models, and this computer couldn't reach it. Try again in a moment.",
    )
}

fn invalid() -> Refusal {
    Refusal::new(
        StatusCode::BAD_REQUEST,
        "Say which model (providerID, id) and whether it's starred (starred, true or false).",
    )
}

impl SetupHandle {
    pub fn model_stars(&self) -> Option<Vec<ModelStar>> {
        self.views.borrow().model_stars.clone()
    }

    /// `None` leaves the preference local when this computer has no accepted setup.
    pub async fn change_model_star(
        &self,
        caller: &Caller,
        model: ModelStar,
        starred: bool,
    ) -> Result<Option<Vec<ModelStar>>, Refusal> {
        if !model.valid() {
            return Err(invalid());
        }
        let by = match caller {
            Caller::Local(_) => Submitter::Computer,
            Caller::Paired { pairing, .. } if pairing.is_current() => Submitter::Paired {
                key: encode(&pairing.key),
                pairing: pairing.id.clone(),
            },
            _ => {
                return Err(Refusal::new(
                    StatusCode::FORBIDDEN,
                    "The submitting device no longer has access.",
                ));
            }
        };
        let result = tokio::time::timeout(
            WAIT + Duration::from_secs(2),
            self.request(Command::ModelStar(Change { model, starred, by })),
        )
        .await
        .map_err(|_| waiting())??;
        match result.status {
            204 => Ok(None),
            200 => {
                let stars = serde_json::from_value::<Stars>(result.value).map_err(|_| waiting())?;
                Ok(Some(stars.starred))
            }
            status => Err(Refusal::json(
                StatusCode::from_u16(status).unwrap_or(StatusCode::CONFLICT),
                result.value,
            )),
        }
    }
}

pub(super) fn list(value: Option<&Value>) -> Vec<ModelStar> {
    let mut stars = Vec::new();
    for row in value
        .and_then(|value| value["starred"].as_array())
        .into_iter()
        .flatten()
    {
        if let Ok(model) = serde_json::from_value::<ModelStar>(row.clone())
            && model.valid()
            && !stars.contains(&model)
        {
            stars.push(model);
        }
    }
    stars.drain(..stars.len().saturating_sub(MAX));
    stars
}

fn unavailable() -> Response {
    Response {
        status: 409,
        value: json!({"message":
        "Your setup's main computer keeps your starred models, and this computer couldn't reach it. Try again in a moment."}),
    }
}

fn denied() -> Response {
    Response {
        status: 403,
        value: json!({"message":"The submitting device no longer has access."}),
    }
}

impl Owner {
    pub(super) fn model_stars_pending(&self) -> bool {
        let (Ok(own), Ok(local)) = (self.own(), self.local_stars()) else {
            return false;
        };
        !local.is_empty()
            && self.seed_revision(&own.public) != Some(fingerprint(&json!({"starred":local})))
    }

    pub(super) async fn offer_model_stars(&mut self) -> Result<(), Error> {
        if self.next_stars_offer > Instant::now() {
            return Ok(());
        }
        self.next_stars_offer = Instant::now() + Duration::from_secs(15);
        let (Ok(setup), Ok(own)) = (self.link.setup(), self.own()) else {
            return Ok(());
        };
        let source = own.public.clone();
        let local = self.local_stars()?;
        let change = json!({"starred":local});
        let revision = self.model_star_revision();
        if local.is_empty() || self.seed_revision(&source) == Some(fingerprint(&change)) {
            return Ok(());
        }
        if setup.role == Role::Primary {
            self.merge_model_star_copy(&source, revision, local).await
        } else {
            self.send_authority(
                &setup.position.primary,
                json!({"kind":"model-stars-copy","revision":revision,"copy":change}),
            )
            .await;
            Ok(())
        }
    }

    pub(super) async fn receive_model_star_copy(
        &mut self,
        member: &str,
        value: &Value,
    ) -> Result<(), Error> {
        let setup = self.link.setup()?;
        if setup.role != Role::Primary
            || value["epoch"].as_u64() != Some(setup.position.epoch)
            || self.link.disabled(&self.roster()?, member)
        {
            return Ok(());
        }
        let Some(revision) = value["revision"].as_u64() else {
            return Ok(());
        };
        self.merge_model_star_copy(member, revision, list(value.get("copy")))
            .await
    }

    fn local_stars(&self) -> Result<Vec<ModelStar>, Error> {
        Ok(list(self.files.read::<Value>("box/models.json")?.as_ref()))
    }

    fn model_star_revision(&self) -> u64 {
        self.link
            .shared
            .get(&KIND)
            .map_or(0, |entry| entry.sequence)
    }

    fn seed_revision(&self, source: &str) -> Option<String> {
        self.link.shared.get(&KIND)?.offline_edits.as_ref()?["additions"][format!("{SEED}{source}")]
            .as_str()
            .map(str::to_owned)
    }

    async fn merge_model_star_copy(
        &mut self,
        source: &str,
        revision: u64,
        copy: Vec<ModelStar>,
    ) -> Result<(), Error> {
        if revision != self.model_star_revision() || copy.is_empty() {
            return Ok(());
        }
        let own = self.own()?.public.clone();
        let prior = self.link.shared.get(&KIND);
        let mut history = prior
            .and_then(|entry| entry.offline_edits.clone())
            .unwrap_or_else(|| json!({"versions":[],"choices":[],"receipts":[],"additions":{}}));
        let Some(additions) = history["additions"].as_object_mut() else {
            return Ok(());
        };
        if prior.is_some() && additions.get(SEED) != Some(&json!("1")) {
            return Ok(());
        }
        additions.insert(SEED.into(), json!("1"));
        let mut stars = list(self.link.value(KIND));
        let mut touched = false;
        if prior.is_none() && source != own {
            touched |= merge_copy(&mut stars, additions, &own, self.local_stars()?);
        }
        touched |= merge_copy(&mut stars, additions, source, copy);
        if !touched {
            return Ok(());
        }
        if let Ok(roster) = self.roster() {
            additions.retain(|key, _| {
                key == SEED
                    || key.starts_with(FIELDS)
                    || key
                        .strip_prefix(SEED)
                        .is_none_or(|key| roster.boxes.iter().any(|peer| peer.key == key))
            });
        }
        let kept = additions
            .iter()
            .filter(|(key, _)| key.starts_with(SEED) && !key.starts_with(FIELDS))
            .filter_map(|(_, revision)| {
                revision
                    .as_str()
                    .map(|revision| format!("{FIELDS}{revision}"))
            })
            .collect::<std::collections::BTreeSet<_>>();
        additions.retain(|key, _| !key.starts_with(FIELDS) || kept.contains(key));
        self.can_write()?;
        let mut next = self.link.clone();
        next.put(
            self.own()?,
            KIND,
            json!({"starred": stars}),
            Some(revision),
            None,
        )?;
        next.shared
            .get_mut(&KIND)
            .expect("put installs the shared document")
            .offline_edits = Some(history);
        next.check_capacity()?;
        self.commit(next).await?;
        self.publish(false).await?;
        Ok(())
    }

    pub(super) async fn ask_model_star(
        &mut self,
        change: Change,
        reply: oneshot::Sender<Result<Response, Error>>,
    ) {
        let Ok(setup) = self.link.setup() else {
            let _ = reply.send(Ok(Response {
                status: 204,
                value: Value::Null,
            }));
            return;
        };
        if setup.role == Role::Primary {
            let result = self.save_model_star(change).await;
            let _ = reply.send(result);
            return;
        }
        if !change.by.current(&self.link) {
            let _ = reply.send(Ok(denied()));
            return;
        }
        let primary = setup.position.primary.clone();
        let epoch = setup.position.epoch;
        let Ok(nonce) = random::<16>().map(|nonce| encode(&nonce)) else {
            let _ = reply.send(Ok(unavailable()));
            return;
        };
        if self.pending_stars.len() >= 32
            || !self
                .send_authority(
                    &primary,
                    json!({"kind":"model-star","request":nonce,"change":change}),
                )
                .await
        {
            let _ = reply.send(Ok(unavailable()));
            return;
        }
        self.pending_stars.insert(
            nonce,
            Pending {
                primary,
                epoch,
                expires: Instant::now() + WAIT,
                reply,
            },
        );
    }

    pub(super) fn expire_model_stars(&mut self) {
        let now = Instant::now();
        let expired = self
            .pending_stars
            .iter()
            .filter(|(_, waiting)| waiting.expires <= now || waiting.reply.is_closed())
            .map(|(nonce, _)| nonce.clone())
            .collect::<Vec<_>>();
        for nonce in expired {
            if let Some(waiting) = self.pending_stars.remove(&nonce) {
                let _ = waiting.reply.send(Ok(unavailable()));
            }
        }
    }

    pub(super) async fn receive_model_star(
        &mut self,
        member: &str,
        value: &Value,
    ) -> Result<(), Error> {
        let setup = self.link.setup()?;
        if setup.role != Role::Primary
            || value["epoch"].as_u64() != Some(setup.position.epoch)
            || self.link.disabled(&self.roster()?, member)
        {
            return Ok(());
        }
        let Some(nonce) = value["request"]
            .as_str()
            .filter(|nonce| safe_token(nonce, 22, 22))
        else {
            return Ok(());
        };
        let change = serde_json::from_value::<Change>(value["change"].clone())
            .map_err(|_| Error::Invalid("forwarded model star"))?;
        let result = self.save_model_star(change).await?;
        self.send_authority(
            member,
            json!({"kind":"model-star-saved","request":nonce,
            "status":result.status,"value":result.value}),
        )
        .await;
        Ok(())
    }

    pub(super) fn receive_model_stars(
        &mut self,
        primary: &str,
        value: &Value,
    ) -> Result<(), Error> {
        let setup = self.link.setup()?;
        if setup.role != Role::Member
            || setup.position.primary != primary
            || value["epoch"].as_u64() != Some(setup.position.epoch)
        {
            return Ok(());
        }
        let Some(nonce) = value["request"].as_str() else {
            return Ok(());
        };
        if !self.pending_stars.get(nonce).is_some_and(|waiting| {
            waiting.primary == primary
                && waiting.epoch == setup.position.epoch
                && waiting.expires > Instant::now()
        }) {
            return Ok(());
        }
        let result = match value["status"].as_u64() {
            Some(200) => Response::ok(value["value"].clone()),
            Some(403) => denied(),
            Some(status @ (400 | 409 | 503)) if value["value"]["message"].is_string() => Response {
                status: status as u16,
                value: value["value"].clone(),
            },
            _ => unavailable(),
        };
        if let Some(waiting) = self.pending_stars.remove(nonce) {
            let _ = waiting.reply.send(Ok(result));
        }
        Ok(())
    }

    async fn save_model_star(&mut self, change: Change) -> Result<Response, Error> {
        if !change.model.valid() {
            return Err(Error::Invalid("model star"));
        }
        if !change.by.current(&self.link) {
            return Ok(denied());
        }
        if self.link.value(KIND).is_none() {
            let own = self.own()?.public.clone();
            let local = self.local_stars()?;
            if let Err(error) = self.merge_model_star_copy(&own, 0, local).await {
                let (status, message) = if matches!(error, Error::Capacity) {
                    (
                        409,
                        "These existing preferences exceed the setup’s saved preference capacity.",
                    )
                } else {
                    (503, "The main computer could not save setup preferences.")
                };
                return Ok(Response {
                    status,
                    value: json!({"message":message}),
                });
            }
        }
        let accepted = self.link.value(KIND);
        let mut stars = list(accepted);
        let mut changed = true;
        match (
            stars.iter().position(|model| model == &change.model),
            change.starred,
        ) {
            (Some(at), false) => {
                stars.remove(at);
            }
            (None, true) => {
                stars.push(change.model);
                stars.drain(..stars.len().saturating_sub(MAX));
            }
            _ => changed = false,
        }
        let value = json!({"starred": stars});
        if (changed || accepted.is_none())
            && self.save(KIND, value.clone(), None, None).await.is_err()
        {
            return Ok(Response {
                status: 409,
                value: json!({"message":
                "The main computer couldn't save your starred models just now. Try again in a moment."}),
            });
        }
        Ok(Response::ok(value))
    }
}

fn merge_copy(
    stars: &mut Vec<ModelStar>,
    additions: &mut Map<String, Value>,
    source: &str,
    copy: Vec<ModelStar>,
) -> bool {
    if copy.is_empty() {
        return false;
    }
    let record = format!("{SEED}{source}");
    let revision = fingerprint(&json!({"starred":copy}));
    let past = additions.get(&record).and_then(Value::as_str);
    if past == Some(&revision) {
        return false;
    }
    let prior = past
        .and_then(|revision| additions.get(&format!("{FIELDS}{revision}")))
        .and_then(Value::as_str)
        .and_then(|fields| serde_json::from_str::<Map<String, Value>>(fields).ok());
    let mut fields = Map::new();
    let named = fingerprint(&Value::Bool(true));
    for model in copy {
        let field = json!([model.provider, model.id]).to_string();
        if prior
            .as_ref()
            .is_none_or(|prior| !prior.contains_key(&field))
            && !stars.contains(&model)
        {
            stars.push(model);
        }
        fields.insert(field, json!(named));
    }
    stars.drain(..stars.len().saturating_sub(MAX));
    additions.insert(record, json!(revision));
    additions.insert(
        format!("{FIELDS}{revision}"),
        json!(Value::Object(fields).to_string()),
    );
    true
}
