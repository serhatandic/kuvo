//! Snapshots of a lead's work on another computer.

use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

use kuvo_wire::{
    keys::encode,
    mail::{Envelope, MAX_PLAINTEXT},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    calls::{self, FEATURE},
    directory::ComputerKey,
    kept,
    outbox::{Delivered, Id},
    reach::{Millis, Standing},
    setup_lists::{Reader, View},
};

const ROWS_EACH: usize = 40;
const LEADS: usize = 50;
const SPACING: Millis = 15_000;
const RETRY: Millis = 60_000;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkState {
    Working,
    Asking,
    Finished,
    Failed,
    Stopped,
    Idle,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub session: String,
    pub state: WorkState,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub folder: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default)]
    pub started_by: String,
    #[serde(default)]
    pub depth: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Model>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asks: Option<String>,
    #[serde(default, rename = "askID", skip_serializing_if = "Option::is_none")]
    pub ask_id: Option<String>,
    #[serde(default)]
    pub started: f64,
    #[serde(default)]
    pub tokens: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Model {
    pub id: String,
    #[serde(rename = "providerID")]
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

impl Row {
    fn clean(mut self) -> Option<Self> {
        if !calls::session(&self.session) {
            return None;
        }
        self.title = plain(&self.title, 200).unwrap_or_else(|| "New Session".into());
        self.folder = calls::prefix(&self.folder, 1000);
        self.project = self.project.and_then(|text| plain(&text, 200));
        if !calls::session(&self.started_by) {
            self.started_by.clear();
        }
        self.depth = self.depth.floor().clamp(1.0, 20.0);
        self.started = self.started.max(0.0);
        self.tokens = self.tokens.max(0.0);
        self.cost = self.cost.filter(|cost| *cost > 0.0);
        self.model = self.model.map(|model| Model {
            id: plain(&model.id, 120).unwrap_or_default(),
            provider: plain(&model.provider, 120).unwrap_or_default(),
            variant: model.variant.and_then(|text| plain(&text, 60)),
        });
        self.asks = self
            .asks
            .filter(|_| matches!(self.state, WorkState::Asking))
            .and_then(|text| plain(&text, 500));
        self.ask_id = self.ask_id.filter(|id| {
            matches!(self.state, WorkState::Asking)
                && (1..=100).contains(&id.len())
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        });
        Some(self)
    }
}

fn plain(text: &str, max: usize) -> Option<String> {
    let collapsed = calls::plain(text, max);
    (!collapsed.is_empty()).then_some(collapsed)
}

/// For a full engine snapshot, supply each row once under each remote ancestor it belongs to.
/// The peer task resolves each lead's home and only shares rows with that computer.
pub type RowsByLead = BTreeMap<String, Vec<Row>>;

#[derive(Deserialize, Serialize)]
struct Kept {
    at: Millis,
    #[serde(default)]
    heard: Option<Millis>,
    leads: RowsByLead,
}

#[derive(Deserialize)]
struct Incoming {
    id: Id,
    at: Millis,
    leads: BTreeMap<String, Value>,
}

#[derive(Deserialize, Serialize)]
struct Sent {
    id: Id,
    done: bool,
    sent: Millis,
}

pub struct RemoteRows {
    pub computer: ComputerKey,
    pub name: String,
    pub at: Millis,
    pub standing: Standing,
    pub rows: Vec<Row>,
}

/// Work rows survive resident and engine restarts. Use this for the last-known list when a
/// live call fails. The standing comes from the same reach view as the setup lists.
pub async fn for_lead(reader: &Reader, lead: &str) -> io::Result<Vec<RemoteRows>> {
    let view = reader.view();
    let now = reader.now();
    let run = reader.run.clone();
    let lead = lead.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut found = vec![];
        for peer in &view.roster.members {
            if let Some(mut copy) = kept::read::<Kept>(&file(&run, peer.key))
                && let Some(rows) = copy.leads.remove(&lead).filter(|rows| !rows.is_empty())
            {
                found.push(RemoteRows {
                    computer: peer.key,
                    name: peer.name.clone(),
                    at: copy.at,
                    standing: view.copy_standing(peer.key, copy.heard.or(Some(copy.at)), now),
                    rows,
                });
            }
        }
        found
    })
    .await
    .map_err(io::Error::other)
}

fn file(run: &Path, key: ComputerKey) -> PathBuf {
    run.join("work/elsewhere")
        .join(format!("{}.json", encode(&key)))
}

fn is_row_id(id: &Id) -> bool {
    id.as_str()
        .strip_prefix("rws_")
        .is_some_and(|part| (6..=60).contains(&part.len()))
}

pub(crate) struct Snapshots {
    run: PathBuf,
    sent: BTreeMap<String, Sent>,
    latest: Option<RowsByLead>,
}

impl Snapshots {
    pub fn open(run: &Path) -> Self {
        Self {
            run: run.into(),
            sent: kept::read(&run.join("work/pushed.json")).unwrap_or_default(),
            latest: None,
        }
    }

    pub fn update(&mut self, leads: RowsByLead) {
        self.latest = Some(
            leads
                .into_iter()
                .filter(|(lead, _)| calls::session(lead))
                .filter_map(|(lead, rows)| {
                    let rows: Vec<_> = rows
                        .into_iter()
                        .take(ROWS_EACH)
                        .filter_map(Row::clean)
                        .collect();
                    (!rows.is_empty()).then_some((lead, rows))
                })
                .collect(),
        );
    }

    pub fn receive(
        &self,
        key: ComputerKey,
        value: Value,
        now: Millis,
    ) -> io::Result<Option<Envelope>> {
        let Ok(given) = serde_json::from_value::<Incoming>(value) else {
            return Ok(None);
        };
        if !is_row_id(&given.id) {
            return Ok(None);
        }
        let path = file(&self.run, key);
        let at = given.at.min(now);
        if kept::read::<Kept>(&path).is_none_or(|copy| copy.at <= at) {
            let leads: RowsByLead = given
                .leads
                .into_iter()
                .take(LEADS)
                .filter(|(lead, _)| calls::session(lead))
                .filter_map(|(lead, rows)| {
                    let rows: Vec<_> = rows
                        .as_array()?
                        .iter()
                        .take(ROWS_EACH)
                        .filter_map(|row| serde_json::from_value::<Row>(row.clone()).ok())
                        .filter_map(Row::clean)
                        .collect();
                    (!rows.is_empty()).then_some((lead, rows))
                })
                .collect();
            calls::note_homes(
                &self.run,
                key,
                at,
                leads.values().flatten().map(|row| row.session.as_str()),
            )?;
            // An empty snapshot is a timestamped clear. Keeping the timestamp prevents an older
            // in-flight snapshot from bringing the rows back.
            kept::write(
                &path,
                &Kept {
                    at,
                    heard: Some(now),
                    leads,
                },
            )?;
        }
        Ok(Some(calls::receipt(&given.id, None, now)))
    }

    pub fn acknowledge(&mut self, key: ComputerKey, receipt: &Delivered) -> io::Result<()> {
        if !receipt.ok || !is_row_id(&receipt.id) {
            return Ok(());
        }
        if let Some(sent) = self.sent.get_mut(&encode(&key))
            && sent.id == receipt.id
            && !sent.done
        {
            sent.done = true;
            if let Err(error) = kept::write(&self.run.join("work/pushed.json"), &self.sent) {
                self.sent
                    .get_mut(&encode(&key))
                    .expect("receipt entry")
                    .done = false;
                return Err(error);
            }
        }
        Ok(())
    }

    pub fn round(
        &mut self,
        view: &View,
        now: Millis,
        home: impl Fn(&str) -> calls::Home,
    ) -> io::Result<calls::MailRound> {
        let Some(latest) = &self.latest else {
            return Ok((vec![], None));
        };
        if !view.relay_connected {
            return Ok((vec![], None));
        }
        let mut by_key = BTreeMap::<_, BTreeMap<_, _>>::new();
        for (lead, rows) in latest {
            if let calls::Home::Peer(key) = home(lead) {
                by_key.entry(key).or_default().insert(lead, rows);
            }
        }
        let mut mail = vec![];
        let mut next = None;
        let mut deadline = |at: Millis| next = Some(next.map_or(at, |before: Millis| before.min(at)));
        for peer in &view.roster.members {
            let key = encode(&peer.key);
            if peer.host().is_none()
                || view.reach.away_since(&key).is_some()
                || !view
                    .announced
                    .get(&key)
                    .and_then(|entry| entry.features.as_ref())
                    .is_some_and(|features| features.iter().any(|feature| feature == FEATURE))
            {
                continue;
            }
            let leads = by_key.remove(&peer.key).unwrap_or_default();
            let before = self.sent.get(&key);
            if leads.is_empty() && before.is_none() {
                continue;
            }
            let bytes = serde_json::to_vec(&leads)?;
            let digest = encode(&Sha256::digest(bytes));
            let id = Id::try_from(format!("rws_{}", &digest[..22])).map_err(io::Error::other)?;
            if let Some(before) = before {
                if before.id == id && before.done { continue; }
                let due = before.sent.saturating_add(if before.id == id { RETRY } else { SPACING });
                if now < due {
                    deadline(due);
                    continue;
                }
            }
            for limit in [ROWS_EACH, 20, 10, 4] {
                let fitted: BTreeMap<_, _> = leads
                    .iter()
                    .take(LEADS)
                    .map(|(lead, rows)| (*lead, &rows[..rows.len().min(limit)]))
                    .collect();
                let value = json!({"at":now, "rows":{"id":id, "at":now, "leads":fitted}});
                let envelope = Envelope(value.as_object().cloned().unwrap_or_default());
                if envelope.encode().len() <= MAX_PLAINTEXT {
                    self.sent.insert(
                        key,
                        Sent {
                            id,
                            done: false,
                            sent: now,
                        },
                    );
                    kept::write(&self.run.join("work/pushed.json"), &self.sent)?;
                    mail.push((peer.key, envelope));
                    deadline(now.saturating_add(RETRY));
                    break;
                }
            }
        }
        Ok((mail, next))
    }
}
