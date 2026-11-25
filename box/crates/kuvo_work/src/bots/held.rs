//! Edits to a bot whose computer is away, held by the box that was asked (PROTOCOL.md,
//! `kuvo/bots`, "Held for another computer"): a client asks any box to change or delete a bot that
//! lives on another of your computers, and that box takes it at once, keeps it for that computer,
//! and hands it on by mail when that computer answers. So no client keeps an outbox of its own.
//!
//! One a bot, in `run/bots-held/<bot id>.json`: each field the last you gave it, and a delete over
//! all of it, sealed with this box's own secret, as a route left it: what anything else on this
//! computer leaves there never goes out as you. The computer that keeps the bot does it as Edit
//! Bot or Delete Bot… would there, once for each edit, and answers; what it refuses is let go, and
//! said, in plain words, to every client that follows this box's bots.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Duration,
};

use hmac::{Hmac, Mac};
use kuvo_peers::{
    directory::{Member, RecordScope},
    ownership::{self, PairingState},
    relay::{RelayEvent, RelayHandle},
    setup_lists::View,
};
use kuvo_server::Caller;
use kuvo_wire::{keys::encode, mail::Envelope};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::Sha256;

use super::{
    Bots,
    edit::AsOf,
    here::{BotHere, plain_name_of},
    job::{given_job, given_job_fields},
};

/// What a box's agents' mail says (`features`) when it holds edits for your other computers and
/// takes those held for it.
pub const HELD_FEATURE: &str = "bots.held";
/// What a computer says once it takes an `effort`, and a job's two fields apart, in a change: one
/// held with them waits for a computer that doesn't yet, which would drop them.
const EFFORT_FEATURE: &str = "bots.effort";
const JOB_FIELDS_FEATURE: &str = "bots.job-fields";
const SUBMITTER_FEATURE: &str = "data.queued-device-authority";
const SUBMITTER_REVOKED: &str =
    "The submitting device no longer has access. Check the record before sending again.";
const SUBMITTER_UNKNOWN: &str =
    "This work predates device tracking. Check the record before submitting it again.";
/// What's held: Edit Bot's fields. A bot's work settings aren't: they change only where it lives.
const FIELDS: [&str; 8] = [
    "name",
    "look",
    "personality",
    "job",
    "jobLine",
    "jobDetails",
    "model",
    "effort",
];
/// How often what's held goes again to a computer that answers, and how long a refusal shows.
const AGAIN_AFTER: i64 = 60_000;
const SCOPED_AGAIN_AFTER: i64 = 5_000;
const REFUSAL_SHOWS: i64 = 86_400_000;
const MOST_REFUSALS: usize = 50;
/// How long a keeper remembers what it did with each held edit, and how many at most.
const TAKEN_MEMORY: i64 = 7 * 86_400_000;
const MOST_TAKEN: usize = 1_000;
/// The most a held change may be, in bytes as JSON (what's held goes in one mail).
const MOST_CHANGE: usize = 32_000;
/// How many times a computer may say a bot isn't there before a change held for it is refused.
const NOT_THERE_TRIES: u32 = 3;
const GONE: &str = "That bot isn’t there any more.";

type Answer = Result<(u16, Value), (u16, String)>;

/// An edit held here: the bot's computer by its key (empty while this box doesn't know it: one you
/// removed, named by the client in `on`), the change and when each field was given (`times`), or a
/// delete, since when it's held and when it last changed, this version's id, and its seal.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct Held {
    id: String,
    bot: String,
    to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    on: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    change: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    times: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delete: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scope: Option<Value>,
    since: i64,
    at: i64,
    /// The bot's name and look, as its computer last told this box by the time this was held.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    known: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stopped: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    seal: Option<String>,
}

impl Held {
    fn deletes(&self) -> bool {
        self.delete == Some(true)
    }

    /// What its seal is of, in order (what it knew of the bot, its scope and why it stopped only
    /// where it has them).
    fn sealed_parts(&self) -> Value {
        let mut parts = vec![
            json!(self.id),
            json!(self.bot),
            json!(self.to),
            json!(self.on),
            json!(self.change),
            json!(self.times),
            json!(self.deletes()),
            json!(self.since),
            json!(self.at),
        ];
        parts.extend(self.known.clone());
        parts.extend(self.scope.clone());
        parts.extend(self.stopped.clone().map(Value::String));
        Value::Array(parts)
    }
}

/// This box's link as the holder reads it (`run/link.json`): its secret, and the setup it follows
/// (its id, and whether it's managed).
struct Link {
    secret: Option<[u8; 32]>,
    setup: Option<(String, bool)>,
}

fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

fn is_bot(id: &str) -> bool {
    id.strip_prefix("bot_").is_some_and(|rest| {
        (1..=60).contains(&rest.len()) && rest.chars().all(|c| c.is_ascii_alphanumeric())
    })
}

fn is_edit(id: &str) -> bool {
    id.strip_prefix("bte_").is_some_and(|rest| {
        (16..=40).contains(&rest.len())
            && rest
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    })
}

fn new_edit_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the system gives random bytes");
    format!("bte_{}", encode(&bytes))
}

fn seal_with(value: &Value, secret: &[u8; 32]) -> String {
    let mut key = Hmac::<Sha256>::new_from_slice(secret).expect("any length is a key");
    key.update(b"kuvo bot edits held");
    let key = key.finalize().into_bytes();
    let mut seal = Hmac::<Sha256>::new_from_slice(&key).expect("any length is a key");
    seal.update(value.to_string().as_bytes());
    encode(&seal.finalize().into_bytes())
}

fn sealed(given: Option<&str>, value: &Value, link: &Link) -> bool {
    let (Some(given), Some(secret)) = (given, &link.secret) else {
        return false;
    };
    let seal = seal_with(value, secret);
    given.len() == seal.len()
        && given
            .bytes()
            .zip(seal.bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

fn read_json(file: &Path) -> Option<Value> {
    kuvo_store::files::text_of(file).and_then(|text| serde_json::from_str(&text).ok())
}

fn write_private(file: &Path, text: &str) -> std::io::Result<()> {
    if let Some(folder) = file.parent() {
        std::fs::create_dir_all(folder)?;
    }
    kuvo_store::files::write_secret(file, text.as_bytes())
}

/// What this process remembers of what's held: when each went, how many times a computer said its
/// bot isn't there, the edits being taken now; and how it reaches your other computers.
#[derive(Default)]
pub(super) struct State {
    sent: Mutex<HashMap<String, (String, i64)>>,
    not_there: Mutex<HashMap<String, u32>>,
    taking: Mutex<HashSet<String>>,
    reach: OnceLock<Reach>,
}

struct Reach {
    owners: ownership::Handle,
    relay: RelayHandle,
}

/// A held change or delete, as a route or a computer's mail gives it.
pub(super) enum Given {
    Change(Value),
    Delete,
}

impl Bots {
    fn held_dir(&self) -> PathBuf {
        self.inner.run.join("bots-held")
    }

    fn held_file(&self, bot: &str) -> PathBuf {
        self.held_dir().join(format!("{bot}.json"))
    }

    fn link(&self) -> Link {
        let link = read_json(&self.inner.run.join("link.json")).unwrap_or_default();
        // (This Mac's box keeps its secret in your Keychain, never in a file there.)
        let secret = (!self.inner.is_mac)
            .then(|| {
                link["secret"]
                    .as_str()
                    .and_then(kuvo_wire::keys::decode_exact)
            })
            .flatten();
        let setup = &link["setup"];
        let setup = (setup["id"].is_string() && setup["aside"] != true).then(|| {
            let id = setup["id"].as_str().unwrap_or_default().to_owned();
            (id, setup["managed"] == true)
        });
        Link { secret, setup }
    }

    /// The edits lock (`run/.bots-held.lock`), as the TypeScript box takes it.
    async fn held_lock(&self) -> Option<crate::wakes::fire::FolderLock> {
        let lock = self.inner.run.join(".bots-held.lock");
        for _ in 0..60 {
            if let Some(held) =
                crate::wakes::fire::FolderLock::take(lock.clone(), Duration::from_secs(10))
            {
                return Some(held);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        None
    }

    fn read_held(&self, bot: &str, link: &Link) -> Option<Held> {
        let held: Held = serde_json::from_value(read_json(&self.held_file(bot))?).ok()?;
        let valid = held.bot == bot
            && is_edit(&held.id)
            && (held.deletes() || held.change.is_some())
            && held
                .known
                .as_ref()
                .is_none_or(|known| known["name"].is_string())
            && held.scope.as_ref().is_none_or(|scope| {
                scope["setup"].is_string() && submitter_of_value(&scope["submitter"]).is_some()
            });
        (valid && sealed(held.seal.as_deref(), &held.sealed_parts(), link)).then_some(held)
    }

    fn write_held(&self, mut held: Held, link: &Link) -> Option<Held> {
        let secret = link.secret.as_ref()?;
        held.seal = Some(seal_with(&held.sealed_parts(), secret));
        let text = serde_json::to_string(&held).ok()?;
        write_private(&self.held_file(&held.bot), &text).ok()?;
        Some(held)
    }

    /// Every edit held here, as kept (unsealed ones left out), the oldest first.
    fn all_held(&self, link: &Link) -> Vec<Held> {
        let mut all: Vec<Held> = crate::wakes::folder::names(&self.held_dir())
            .iter()
            .filter_map(|name| name.strip_suffix(".json").filter(|bot| is_bot(bot)))
            .filter_map(|bot| self.read_held(bot, link))
            .collect();
        all.sort_by_key(|held| held.since);
        all
    }

    /// What your other computers' told this box of each bot: its name and look.
    fn known_elsewhere(view: &View, now: i64) -> HashMap<String, Value> {
        let mut known: HashMap<String, (i64, Value)> = HashMap::new();
        for told in view.bots_told(now) {
            for bot in &told.bots {
                let id = bot.id.clone().unwrap_or_default();
                if known
                    .get(&id)
                    .is_some_and(|(heard, _)| *heard >= told.heard)
                {
                    continue;
                }
                let mut said = json!({ "name": bot.name });
                if let Some(look) = &bot.look {
                    said["look"] = json!(look);
                }
                known.insert(id, (told.heard, said));
            }
        }
        known
            .into_iter()
            .map(|(id, (_, said))| (id, said))
            .collect()
    }

    /// The computer that keeps `bot`, as its box last told this one, or else as the client named it
    /// (`on`, its key): one of this box's peers, never this box.
    fn keeper_of<'a>(
        view: &'a View,
        bot: &str,
        on: Option<&str>,
        except: Option<&str>,
    ) -> Option<&'a Member> {
        let peers = view
            .roster
            .members
            .iter()
            .filter(|peer| except != Some(encode(&peer.key).as_str()));
        let listing = peers
            .clone()
            .filter_map(|peer| {
                let told = view.announcement(&encode(&peer.key))?;
                let lists = told.list.iter().any(|agent| {
                    agent.kind == kuvo_peers::directory::AgentKind::Bot
                        && agent.id.as_deref() == Some(bot)
                });
                lists.then_some((told.heard.unwrap_or(told.at), peer))
            })
            .max_by_key(|(heard, _)| *heard)
            .map(|(_, peer)| peer);
        listing.or_else(|| {
            peers
                .clone()
                .find(|peer| Some(encode(&peer.key).as_str()) == on)
        })
    }

    fn features_of(view: &View, key: &str) -> Vec<String> {
        view.announcement(key)
            .and_then(|told| told.features.clone())
            .unwrap_or_default()
    }

    fn name_of(view: &View, held: &Held) -> String {
        view.roster
            .members
            .iter()
            .find(|peer| !held.to.is_empty() && encode(&peer.key) == held.to)
            .map(|peer| peer.name.clone())
            .or_else(|| held.on.clone().filter(|on| !on.is_empty()))
            .unwrap_or_else(|| "another computer".into())
    }

    fn stopped_why(held: &Held, link: &Link) -> Option<String> {
        if let Some(stopped) = &held.stopped {
            return Some(stopped.clone());
        }
        match (&held.scope, &link.setup) {
            (None, Some((_, true))) => Some(SUBMITTER_UNKNOWN.into()),
            (Some(scope), setup)
                if scope["setup"].as_str() != setup.as_ref().map(|(id, _)| id.as_str()) =>
            {
                Some("This change belongs to the previous setup.".into())
            }
            _ => None,
        }
    }

    fn waits_of(view: &View, held: &Held, link: &Link) -> Option<String> {
        if let Some(stopped) = Self::stopped_why(held, link) {
            return Some(stopped);
        }
        view.announcement(&held.to)?;
        let features = Self::features_of(view, &held.to);
        let computer = Self::name_of(view, held);
        if held.scope.is_some() && !features.iter().any(|each| each == SUBMITTER_FEATURE) {
            return Some(format!(
                "{computer} needs an update to confirm the submitting device."
            ));
        }
        let missing = missing_feature(held.change.as_ref(), &features)?;
        Some(format!(
            "{computer} needs an update before it takes {missing}: this goes once Kuvo Code is updated on {computer}."
        ))
    }

    fn view_of(view: &View, held: &Held, link: &Link, known: &HashMap<String, Value>) -> Value {
        let mut said =
            json!({ "bot": held.bot, "box": held.to, "computer": Self::name_of(view, held) });
        bot_of(&mut said, known.get(&held.bot).or(held.known.as_ref()));
        if let Some(waits) = Self::waits_of(view, held, link) {
            said["waits"] = json!(waits);
        }
        match &held.change {
            Some(change) if !held.deletes() => said["change"] = json!(change),
            _ => said["delete"] = json!(true),
        }
        said["since"] = json!(held.since);
        said
    }

    /// What's held here for your other computers, and what they refused lately, as `kuvo/bots`
    /// lists them; neither while this box can't tell its own from what's planted (no secret).
    pub(super) fn held_lists(&self, view: &View) -> Option<(Vec<Value>, Vec<Value>)> {
        let link = self.link();
        link.secret?;
        let now = now_ms();
        let known = Self::known_elsewhere(view, now);
        let held = self
            .all_held(&link)
            .iter()
            .map(|each| Self::view_of(view, each, &link, &known))
            .collect();
        let refused = self
            .read_refused(now, &link)
            .into_iter()
            .map(|mut each| {
                let kept = each.get("name").is_some().then(|| {
                    let mut kept = json!({ "name": each["name"] });
                    if let Some(look) = each.get("look") {
                        kept["look"] = look.clone();
                    }
                    kept
                });
                if let Some(said) = each.as_object_mut() {
                    said.remove("name");
                    said.remove("look");
                }
                let message = each["message"].take();
                let at = each["at"].take();
                if let Some(said) = each.as_object_mut() {
                    said.remove("message");
                    said.remove("at");
                }
                let bot = each["bot"].as_str().unwrap_or_default().to_owned();
                bot_of(&mut each, known.get(&bot).or(kept.as_ref()));
                each["message"] = message;
                each["at"] = at;
                each
            })
            .collect();
        Some((held, refused))
    }

    /// `PATCH` or `DELETE kuvo/bots/<id>` for a bot that lives on another of your computers: held
    /// here for it, on top of what's held already, a field at a time, a delete over all of it.
    pub(super) async fn hold(
        &self,
        bot: &str,
        given: Given,
        on: Option<&str>,
        removed: bool,
        caller: &Caller,
    ) -> Answer {
        let no = |status: u16, message: &str| Err((status, message.to_owned()));
        if !is_bot(bot) {
            return no(404, GONE);
        }
        let link = self.link();
        let view = self.inner.peers.view();
        let keeper = Self::keeper_of(&view, bot, on, None);
        let named = match (removed, on) {
            (true, Some(on)) => plain_name_of(Some(&json!(on))).chars().take(200).collect(),
            _ => String::new(),
        };
        if keeper.is_none() && named.is_empty() {
            return no(404, GONE);
        }
        let paired = caller.pairing().is_some();
        let Some(submitter) = submitter_of(caller).filter(|_| !paired || link.setup.is_some())
        else {
            return no(403, "Use a paired Kuvo Code client to hold this change.");
        };
        let scope = link
            .setup
            .as_ref()
            .filter(|(_, managed)| *managed || paired)
            .map(|(id, _)| json!({ "setup": id, "submitter": submitter }));
        let computer = keeper.map_or_else(|| named.clone(), |peer| peer.name.clone());
        let checked = match &given {
            Given::Change(change) => {
                if change.get("work").is_some() && change.is_object() {
                    return no(
                        409,
                        &format!(
                            "{computer} isn’t connected: change its work settings once it’s back."
                        ),
                    );
                }
                match self.checked_change(change) {
                    Ok(change) => Some(change),
                    Err(error) => return no(400, error),
                }
            }
            Given::Delete => None,
        };
        let cant = format!(
            "{} can’t keep it now. Try again in a moment.",
            self.inner.computer
        );
        let Some(_lock) = self.held_lock().await else {
            return no(503, &cant);
        };
        let now = now_ms();
        let before = self
            .read_held(bot, &link)
            .filter(|before| Self::stopped_why(before, &link).is_none());
        if before.as_ref().is_some_and(|before| before.scope != scope) {
            return no(
                409,
                "A different device's change is still held for this bot. Check the held change before editing here.",
            );
        }
        let known = Self::known_elsewhere(&view, now)
            .remove(bot)
            .or_else(|| before.as_ref().and_then(|before| before.known.clone()));
        if before.as_ref().is_some_and(Held::deletes) && checked.is_some() {
            return no(
                404,
                &format!("That bot is deleted: it goes from {computer} once it’s back."),
            );
        }
        let mut change = before
            .as_ref()
            .and_then(|before| before.change.clone())
            .unwrap_or_default();
        let mut times = before
            .as_ref()
            .and_then(|before| before.times.clone())
            .unwrap_or_default();
        if let Some(checked) = &checked {
            // (A model given without an effort runs at its own: an effort held for the model it
            // had doesn't go on to the new one.)
            if checked.contains_key("model") && !checked.contains_key("effort") {
                change.remove("effort");
                times.remove("effort");
            }
            if checked.contains_key("job") {
                for field in ["jobLine", "jobDetails"] {
                    if !checked.contains_key(field) {
                        change.remove(field);
                        times.remove(field);
                    }
                }
            }
            for (field, value) in checked {
                change.insert(field.clone(), value.clone());
                times.insert(field.clone(), json!(now));
            }
            if Value::Object(change.clone()).to_string().len() > MOST_CHANGE {
                return no(400, "That’s too much to change at once.");
            }
        }
        let held = Held {
            id: new_edit_id(),
            bot: bot.to_owned(),
            to: keeper.map(|peer| encode(&peer.key)).unwrap_or_default(),
            on: keeper.is_none().then_some(named),
            change: checked.is_some().then_some(change),
            times: checked.is_some().then_some(times),
            delete: checked.is_none().then_some(true),
            scope,
            since: before.as_ref().map_or(now, |before| before.since),
            at: now,
            known,
            stopped: None,
            seal: None,
        };
        let Some(held) = self.write_held(held, &link) else {
            return no(503, &cant);
        };
        self.state().sent.lock().expect("sent").remove(bot);
        self.state()
            .not_there
            .lock()
            .expect("not there")
            .remove(bot);
        self.drop_refusal(bot, &link);
        let said = Self::view_of(&view, &held, &link, &Self::known_elsewhere(&view, now));
        drop(_lock);
        self.changed();
        self.send_held_soon();
        Ok((202, json!({ "held": said })))
    }

    fn state(&self) -> &State {
        &self.inner.held
    }

    /// A change as Edit Bot gives it, checked as far as this box can without the bot.
    fn checked_change(&self, given: &Value) -> Result<Map<String, Value>, &'static str> {
        let given = given.as_object().ok_or("That isn’t a change to a bot.")?;
        let mut change: Map<String, Value> = FIELDS
            .iter()
            .filter_map(|field| Some(((*field).to_owned(), given.get(*field)?.clone())))
            .collect();
        if change.is_empty() {
            return Err("That isn’t a change to a bot.");
        }
        if change.contains_key("name") {
            let name = plain_name_of(change.get("name"));
            if name.is_empty() {
                return Err("A bot needs a name.");
            }
            change.insert("name".into(), json!(name));
        }
        if change.contains_key("look") && !kuvo_peers::directory::Look::fits(change.get("look")) {
            return Err("That isn’t a look a bot can have.");
        }
        if change
            .get("personality")
            .is_some_and(|it| !it.is_null() && !it.is_string())
        {
            return Err("That isn’t a personality.");
        }
        if let Some(job) = change.get("job") {
            given_job(job)?;
        }
        given_job_fields(&change)?;
        if change
            .get("model")
            .is_some_and(|model| super::make::bot_model(model).is_none())
        {
            return Err("That isn’t a model.");
        }
        if change
            .get("effort")
            .is_some_and(|it| !it.is_null() && !it.is_string())
        {
            return Err("That isn’t an effort.");
        }
        Ok(change)
    }

    /// The computer that keeps a bot's settings, where that's another of yours (a move finished
    /// there): its key.
    pub(super) fn owner_elsewhere(&self, bot: &str) -> Option<String> {
        let view = self.inner.peers.view();
        let owner = view.owner_of(RecordScope::Bot, bot, self.inner.peers.now())?;
        (!owner.local)
            .then(|| owner.key.map(|key| encode(&key)))
            .flatten()
    }

    fn owned_elsewhere(&self, bot: &str) -> bool {
        let view = self.inner.peers.view();
        view.owner_of(RecordScope::Bot, bot, self.inner.peers.now())
            .is_some_and(|owner| !owner.local)
    }

    /// Whether a change to `bot` is held for another of your computers rather than made here: one
    /// that isn't here, or whose settings another keeps.
    pub(super) fn lives_elsewhere(&self, bot: &str) -> bool {
        BotHere::read(&self.inner.bots, bot).is_none() || self.owned_elsewhere(bot)
    }

    fn send_held_soon(&self) {
        let bots = self.clone();
        tokio::spawn(async move { bots.send_held(now_ms()).await });
    }

    /// How this box reaches your other computers with what it holds, and hears what they hold for
    /// it: what's held goes each round (a minute), at once as it's held and as its computer is
    /// back; their mail is taken as it comes.
    pub fn follow_held(
        &self,
        owners: ownership::Handle,
        relay: RelayHandle,
    ) -> tokio::task::JoinHandle<()> {
        let events = relay.subscribe();
        let _ = self.state().reach.set(Reach { owners, relay });
        let bots = self.clone();
        tokio::spawn(async move {
            let Some(mut events) = events else {
                return;
            };
            let mut round = tokio::time::interval(Duration::from_secs(15));
            loop {
                tokio::select! {
                    _ = round.tick() => bots.send_held(now_ms()).await,
                    event = events.recv() => match event {
                        Ok(event) => match event.as_ref() {
                            RelayEvent::Connected(_) => bots.send_held(now_ms()).await,
                            RelayEvent::Mail { key, envelope, .. } => bots.take_mail(encode(key.as_slice()), &envelope.0).await,
                            _ => {}
                        },
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(_) => return,
                    },
                }
            }
        })
    }

    async fn mail(&self, to: &str, payload: Value) {
        let Some(reach) = self.state().reach.get() else {
            return;
        };
        let view = self.inner.peers.view();
        let Some(peer) = view
            .roster
            .members
            .iter()
            .find(|peer| encode(&peer.key) == to)
        else {
            return;
        };
        let mut payload = payload.as_object().cloned().unwrap_or_default();
        payload.insert("at".into(), json!(now_ms()));
        let Some(host) = peer.host() else {
            return;
        };
        let _ = reach.relay.sender.mail(host, Envelope(payload)).await;
    }

    fn authority(&self, submitter: Option<&Value>) -> PairingState {
        match submitter.and_then(submitter_of_value) {
            Some(None) => PairingState::Allowed,
            Some(Some((key, pairing))) => match (
                self.state().reach.get(),
                kuvo_wire::keys::decode_exact(&key),
            ) {
                (Some(reach), Some(key)) => reach.owners.current_pairing(&key, &pairing),
                _ => PairingState::Revoked,
            },
            None => PairingState::Revoked,
        }
    }

    /// What's held here, sent to each computer it's for: again each minute until it answers (a
    /// scoped one each few seconds), at once when it's back. One whose bot another of your computers
    /// lists now goes there instead. Only to a computer that takes it.
    async fn send_held(&self, now: i64) {
        if self.state().reach.get().is_none() {
            return;
        }
        let link = self.link();
        let view = self.inner.peers.view();
        for mut held in self.all_held(&link) {
            if Self::stopped_why(&held, &link).is_some() {
                continue;
            }
            let authority = held
                .scope
                .as_ref()
                .map(|scope| self.authority(scope.get("submitter")));
            if authority == Some(PairingState::Checking) {
                continue;
            }
            let lists = |key: &str| {
                view.announcement(key).is_some_and(|told| {
                    told.list
                        .iter()
                        .any(|agent| agent.id.as_deref() == Some(held.bot.as_str()))
                })
            };
            let still_there = !held.to.is_empty()
                && view
                    .roster
                    .members
                    .iter()
                    .any(|peer| encode(&peer.key) == held.to)
                && lists(&held.to);
            let keeper = (!still_there)
                .then(|| Self::keeper_of(&view, &held.bot, held.on.as_deref(), None))
                .flatten()
                .map(|peer| encode(&peer.key));
            if let Some(keeper) = keeper.filter(|keeper| *keeper != held.to) {
                let Some(_lock) = self.held_lock().await else {
                    continue;
                };
                if self
                    .read_held(&held.bot, &link)
                    .is_none_or(|now| now.id != held.id)
                {
                    continue;
                }
                let moved = Held {
                    to: keeper,
                    on: None,
                    ..held.clone()
                };
                let Some(moved) = self.write_held(moved, &link) else {
                    continue;
                };
                held = moved;
                self.state().sent.lock().expect("sent").remove(&held.bot);
                self.changed();
            }
            let again = if held.scope.is_some() {
                SCOPED_AGAIN_AFTER
            } else {
                AGAIN_AFTER
            };
            let last = self
                .state()
                .sent
                .lock()
                .expect("sent")
                .get(&held.bot)
                .cloned();
            if last.is_some_and(|(id, at)| id == held.id && now - at < again) {
                continue;
            }
            let features = Self::features_of(&view, &held.to);
            let takes = |feature: &str| features.iter().any(|each| each == feature);
            if !takes(HELD_FEATURE)
                || missing_feature(held.change.as_ref(), &features).is_some()
                || (held.scope.is_some() && !takes(SUBMITTER_FEATURE))
            {
                continue;
            }
            let mut edit = json!({ "id": held.id, "bot": held.bot });
            if held.deletes() {
                edit["delete"] = json!(true);
            } else {
                // (How long ago each field was given, not when: the computers' clocks may differ.)
                let ages: Map<String, Value> = held
                    .times
                    .iter()
                    .flatten()
                    .filter_map(|(field, at)| {
                        Some((field.clone(), json!((now - at.as_i64()?).max(0))))
                    })
                    .collect();
                edit["change"] = json!(held.change);
                edit["ages"] = Value::Object(ages);
            }
            edit["at"] = json!(held.at);
            let payload = match &held.scope {
                Some(scope) => {
                    edit["scope"] = scope.clone();
                    if authority == Some(PairingState::Revoked) {
                        edit["replayOnly"] = json!(true);
                    }
                    json!({ "scopedBotEdit": edit })
                }
                None => json!({ "botEdit": edit }),
            };
            self.mail(&held.to, payload).await;
            self.state()
                .sent
                .lock()
                .expect("sent")
                .insert(held.bot.clone(), (held.id.clone(), now));
        }
    }

    async fn take_mail(&self, sender: String, payload: &Map<String, Value>) {
        let view = self.inner.peers.view();
        if !view
            .roster
            .members
            .iter()
            .any(|peer| encode(&peer.key) == sender)
        {
            return;
        }
        // (Back, telling its agents: what's held for it goes now, not at its next minute.)
        if payload.get("agents").is_some_and(Value::is_object) {
            let ids: HashSet<String> = self
                .all_held(&self.link())
                .into_iter()
                .filter(|held| held.to == sender)
                .map(|held| held.id)
                .collect();
            if !ids.is_empty() {
                self.state()
                    .sent
                    .lock()
                    .expect("sent")
                    .retain(|_, (id, _)| !ids.contains(id));
                self.send_held_soon();
            }
        }
        let scoped = payload.get("scopedBotEdit").filter(|edit| edit.is_object());
        if let Some(edit) =
            scoped.or_else(|| payload.get("botEdit").filter(|edit| edit.is_object()))
        {
            let bots = self.clone();
            let (sender, edit, scoped) = (sender.clone(), edit.clone(), scoped.is_some());
            tokio::spawn(async move { bots.take_bot_edit(&sender, &edit, scoped).await });
        }
        if let Some(answer) = payload.get("botEdited").filter(|answer| answer.is_object()) {
            self.take_bot_edited(&sender, answer, now_ms()).await;
        }
    }

    /// The answer from the computer a held edit went to, from that computer only: done, it's let
    /// go; refused for good, let go and said; one it can't take now goes again. A bot no longer
    /// there: a delete is done; a change goes to the computer that lists it now, or is refused.
    async fn take_bot_edited(&self, sender: &str, given: &Value, now: i64) {
        let (Some(id), Some(bot)) = (given["id"].as_str(), given["bot"].as_str()) else {
            return;
        };
        let Some(status) = given["status"]
            .as_u64()
            .filter(|status| (200..=599).contains(status))
        else {
            return;
        };
        if !is_edit(id) || !is_bot(bot) {
            return;
        }
        let link = self.link();
        let Some(_lock) = self.held_lock().await else {
            return;
        };
        let Some(held) = self
            .read_held(bot, &link)
            .filter(|held| held.id == id && held.to == sender)
        else {
            return;
        };
        let state = self.state();
        let changed;
        if status == 409 || status >= 500 {
            state
                .sent
                .lock()
                .expect("sent")
                .insert(bot.into(), (held.id, now));
            return;
        }
        let view = self.inner.peers.view();
        if status == 404 && !held.deletes() {
            let tries = state
                .not_there
                .lock()
                .expect("not there")
                .get(bot)
                .copied()
                .unwrap_or(0)
                + 1;
            if tries < NOT_THERE_TRIES {
                state
                    .not_there
                    .lock()
                    .expect("not there")
                    .insert(bot.into(), tries);
                // (Moved meanwhile: it goes where the bot is now, as a new edit.)
                if let Some(keeper) = Self::keeper_of(&view, bot, None, Some(sender)) {
                    let moved = Held {
                        id: new_edit_id(),
                        to: encode(&keeper.key),
                        ..held
                    };
                    if self.write_held(moved, &link).is_some() {
                        state.sent.lock().expect("sent").remove(bot);
                        drop(_lock);
                        self.changed();
                        self.send_held_soon();
                    }
                    return;
                }
                state
                    .sent
                    .lock()
                    .expect("sent")
                    .insert(bot.into(), (held.id, now));
                return;
            }
        }
        if status == 403 && held.scope.is_some() {
            let stopped = given["message"]
                .as_str()
                .map_or(SUBMITTER_REVOKED.to_owned(), |message| {
                    message.chars().take(500).collect()
                });
            changed = self
                .write_held(
                    Held {
                        stopped: Some(stopped),
                        ..held
                    },
                    &link,
                )
                .is_some();
        } else {
            state.not_there.lock().expect("not there").remove(bot);
            let _ = std::fs::remove_file(self.held_file(bot));
            state.sent.lock().expect("sent").remove(bot);
            changed = true;
            let why: String = given["message"]
                .as_str()
                .map(|message| {
                    plain_name_of(Some(&json!(message)))
                        .chars()
                        .take(500)
                        .collect()
                })
                .unwrap_or_default();
            // (Done: but for some of it, said.)
            let said = !((status < 300 && why.is_empty()) || (status == 404 && held.deletes()));
            if said {
                let computer = Self::name_of(&view, &held);
                let why = if why.is_empty() {
                    why
                } else {
                    format!(" {why}")
                };
                let message = if held.deletes() {
                    format!("{computer} didn’t delete the bot as you asked while it was away.{why}")
                } else if status < 300 {
                    format!(
                        "{computer} didn’t take all the changes you made while it was away.{why}"
                    )
                } else {
                    format!("{computer} didn’t take the changes you made while it was away.{why}")
                };
                let known = Self::known_elsewhere(&view, now)
                    .remove(bot)
                    .or(held.known.clone());
                let mut refusal = json!({ "bot": bot, "box": held.to, "computer": computer });
                if let Some(known) = known.filter(|known| known["name"].is_string()) {
                    refusal["name"] = known["name"].clone();
                    if let Some(look) = known.get("look") {
                        refusal["look"] = look.clone();
                    }
                }
                refusal["message"] = json!(message);
                refusal["at"] = json!(now);
                let mut refused = vec![refusal];
                refused.extend(
                    self.read_refused(now, &link)
                        .into_iter()
                        .filter(|each| each["bot"] != bot),
                );
                self.write_refused(refused, &link);
            }
        }
        drop(_lock);
        if changed {
            self.changed();
        }
    }

    fn refused_file(&self) -> PathBuf {
        self.held_dir().join("refused.json")
    }

    fn read_refused(&self, now: i64, link: &Link) -> Vec<Value> {
        let Some(kept) = read_json(&self.refused_file()) else {
            return Vec::new();
        };
        let Some(refused) = kept["refused"].as_array() else {
            return Vec::new();
        };
        if !sealed(kept["seal"].as_str(), &kept["refused"], link) {
            return Vec::new();
        }
        refused
            .iter()
            .filter(|each| {
                ["bot", "box", "computer", "message"]
                    .iter()
                    .all(|key| each[*key].is_string())
                    && each["at"]
                        .as_i64()
                        .is_some_and(|at| now - at < REFUSAL_SHOWS)
            })
            .cloned()
            .collect()
    }

    fn write_refused(&self, mut refused: Vec<Value>, link: &Link) {
        if refused.is_empty() {
            let _ = std::fs::remove_file(self.refused_file());
            return;
        }
        refused.truncate(MOST_REFUSALS);
        let refused = Value::Array(refused);
        if let Some(secret) = &link.secret {
            let seal = seal_with(&refused, secret);
            let _ = write_private(
                &self.refused_file(),
                &json!({ "refused": refused, "seal": seal }).to_string(),
            );
        }
    }

    fn drop_refusal(&self, bot: &str, link: &Link) {
        let refused = self.read_refused(now_ms(), link);
        if refused.iter().any(|each| each["bot"] == bot) {
            self.write_refused(
                refused
                    .into_iter()
                    .filter(|each| each["bot"] != bot)
                    .collect(),
                link,
            );
        }
    }

    fn taken_file(&self) -> PathBuf {
        self.inner.run.join("bots-held-taken.json")
    }

    fn read_taken(&self, now: i64) -> Map<String, Value> {
        let kept = read_json(&self.taken_file()).and_then(|kept| kept.as_object().cloned());
        kept.unwrap_or_default()
            .into_iter()
            .filter(|(_, each)| {
                each["until"].as_i64().is_some_and(|until| until > now)
                    && each["status"].is_number()
            })
            .collect()
    }

    fn keep_taken(&self, key: &str, taken: Value, now: i64) {
        let mut kept = self.read_taken(now);
        kept.remove(key);
        kept.insert(key.into(), taken);
        let skip = kept.len().saturating_sub(MOST_TAKEN);
        let kept: Map<String, Value> = kept.into_iter().skip(skip).collect();
        let _ = write_private(&self.taken_file(), &Value::Object(kept).to_string());
    }

    /// An edit another of your computers held for this one, done here as a client's would be,
    /// once for each id, and answered; one sent again is answered as it went. One this box can't do
    /// now (the bot moving, its engine starting) isn't remembered: it's done when it comes again.
    async fn take_bot_edit(&self, sender: &str, given: &Value, scoped: bool) {
        let (Some(id), Some(bot)) = (given["id"].as_str(), given["bot"].as_str()) else {
            return;
        };
        if !is_edit(id) || !is_bot(bot) || (given["delete"] != true && !given["change"].is_object())
        {
            return;
        }
        let answer = |status: u16, message: Option<String>| {
            let mut edited = json!({ "id": id, "bot": bot, "status": status });
            if let Some(message) = message {
                edited["message"] = json!(message);
            }
            json!({ "botEdited": edited })
        };
        let now = now_ms();
        let key = format!("{sender} {id}");
        if self.state().taking.lock().expect("taking").contains(&key) {
            return;
        }
        if let Some(kept) = self.read_taken(now).get(&key) {
            let message = kept["message"].as_str().map(str::to_owned);
            let status = kept["status"].as_u64().unwrap_or(200) as u16;
            return self.mail(sender, answer(status, message)).await;
        }
        let scope = given.get("scope").filter(|_| scoped);
        let setup = self.link().setup.map(|(id, _)| id);
        if scoped
            && (scope
                .and_then(|scope| submitter_of_value(&scope["submitter"]))
                .is_none()
                || scope.and_then(|scope| scope["setup"].as_str()) != setup.as_deref())
        {
            return;
        }
        let authority = || {
            let link = self.link();
            if !scoped {
                return if link.setup.is_some_and(|(_, managed)| managed) {
                    PairingState::Checking
                } else {
                    PairingState::Allowed
                };
            }
            let peer = self
                .inner
                .peers
                .view()
                .roster
                .members
                .iter()
                .any(|peer| encode(&peer.key) == sender);
            if scope.and_then(|scope| scope["setup"].as_str())
                != link.setup.as_ref().map(|(id, _)| id.as_str())
                || !peer
                || given["replayOnly"] == true
            {
                return PairingState::Revoked;
            }
            self.authority(scope.and_then(|scope| scope.get("submitter")))
        };
        match authority() {
            PairingState::Allowed => {}
            PairingState::Checking if scoped => return,
            PairingState::Checking => {
                return self
                    .mail(sender, answer(403, Some(SUBMITTER_UNKNOWN.into())))
                    .await;
            }
            PairingState::Revoked => {
                return self
                    .mail(sender, answer(403, Some(SUBMITTER_REVOKED.into())))
                    .await;
            }
        }
        self.state()
            .taking
            .lock()
            .expect("taking")
            .insert(key.clone());
        let admit = || matches!(authority(), PairingState::Allowed);
        let admit: &(dyn Fn() -> bool + Send + Sync) = &admit;
        let mut done = if given["delete"] == true {
            match self.owned_elsewhere(bot) {
                true => moved(),
                false => self.delete(bot).await.map(|value| (200, value)),
            }
        } else {
            self.take_change(bot, given, scoped.then_some(admit)).await
        };
        if done.as_ref().is_err_and(|(status, _)| *status >= 500)
            && matches!(authority(), PairingState::Revoked)
        {
            done = Err((403, SUBMITTER_REVOKED.into()));
        }
        let (status, message) = match done {
            Ok((status, value)) => (status, value["message"].as_str().map(str::to_owned)),
            Err((status, message)) => (status, Some(message)),
        };
        // (Not one it couldn't do now, nor a change for a bot not here yet.)
        if status != 409 && status < 500 && !(status == 404 && given["delete"] != true) {
            let mut taken = json!({ "until": now + TAKEN_MEMORY, "status": status });
            if let Some(message) = &message {
                taken["message"] = json!(message);
            }
            self.keep_taken(&key, taken, now);
        }
        self.mail(sender, answer(status, message)).await;
        self.state().taking.lock().expect("taking").remove(&key);
    }

    /// Only what was given after it last changed here (a change made here meanwhile stays), and
    /// never its work settings; when each was given worked out from how long ago the holder said.
    async fn take_change(
        &self,
        bot: &str,
        given: &Value,
        authorize: Option<&(dyn Fn() -> bool + Send + Sync)>,
    ) -> Answer {
        if self.owned_elsewhere(bot) {
            return moved();
        }
        let clock = now_ms();
        let mut times: Map<String, Value> = given["ages"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(field, age)| {
                Some((field.clone(), json!(clock - age.as_f64()?.max(0.0) as i64)))
            })
            .collect();
        let mut change = given["change"].as_object().cloned().unwrap_or_default();
        // A whole job from an older sender may be fresh for only one of its fields.
        if let Some(legacy) = change.get("job").and_then(|job| given_job(job).ok()) {
            for (field, value) in [("jobLine", legacy.line), ("jobDetails", legacy.details)] {
                if change.contains_key(field) {
                    continue;
                }
                change.insert(field.into(), json!(value));
                if let Some(at) = times.get("job").cloned() {
                    times.insert(field.into(), at);
                }
            }
            change.remove("job");
        }
        let edited = super::edit::edited_times(&self.inner.bots.join(bot));
        let at = |field: &str| edited.get(field).and_then(Value::as_i64).unwrap_or(0);
        // (An effort is its model's: one given before its model last changed here is stale too.)
        let last_edited = |field: &str| match field {
            "effort" => at("model").max(at("effort")),
            "jobLine" | "jobDetails" => at("job").max(at(field)),
            "job" => at("job").max(at("jobLine")).max(at("jobDetails")),
            _ => at(field),
        };
        let fresh: Vec<(String, Value)> = change
            .into_iter()
            .filter(|(field, _)| FIELDS.contains(&field.as_str()))
            .filter(|(field, _)| {
                times
                    .get(field)
                    .and_then(Value::as_i64)
                    .is_none_or(|given| given > last_edited(field))
            })
            .collect();
        // (An effort given with a model is that model's: not for the one it has once that model
        // isn't taken.)
        let model_given = given["change"].get("model").is_some();
        let taken: Vec<(String, Value)> =
            if model_given && !fresh.iter().any(|(field, _)| field == "model") {
                fresh
                    .into_iter()
                    .filter(|(field, _)| field != "effort")
                    .collect()
            } else {
                fresh
            };
        if taken.is_empty() {
            return Ok((200, json!({})));
        }
        self.change_what_it_can(bot, taken, &times, authorize).await
    }

    /// A held change done as far as it can be: whole, or, where that's refused for what's in it,
    /// each field on its own (a model with its effort), what's refused said.
    async fn change_what_it_can(
        &self,
        bot: &str,
        fields: Vec<(String, Value)>,
        times: &Map<String, Value>,
        authorize: Option<&(dyn Fn() -> bool + Send + Sync)>,
    ) -> Answer {
        let as_of = || Some(AsOf { times, authorize });
        let change = |part: &[(String, Value)]| Value::Object(part.iter().cloned().collect());
        let whole = self
            .change_as_of(bot, change(&fields), as_of())
            .await
            .map(|value| (200, value));
        let refused_for_what = |answer: &Answer| matches!(answer, Err((400 | 422, _)));
        if fields.len() < 2 || !refused_for_what(&whole) {
            return whole;
        }
        let (together, apart): (Vec<_>, Vec<_>) =
            if fields.iter().any(|(field, _)| field == "model") {
                fields
                    .into_iter()
                    .partition(|(field, _)| field == "model" || field == "effort")
            } else {
                (Vec::new(), fields)
            };
        let mut parts: Vec<Vec<(String, Value)>> =
            apart.into_iter().map(|field| vec![field]).collect();
        if !together.is_empty() {
            parts.push(together);
        }
        if parts.len() < 2 {
            return whole;
        }
        let (mut took, mut refused) = (false, Vec::new());
        for part in parts {
            match self.change_as_of(bot, change(&part), as_of()).await {
                Err((status, message)) if status == 409 || status >= 500 => {
                    return Err((status, message));
                }
                Ok(_) => took = true,
                Err((_, message)) => refused.push(message),
            }
        }
        match took {
            true if refused.is_empty() => Ok((200, json!({}))),
            true => Ok((200, json!({ "message": refused.join(" ") }))),
            false => whole,
        }
    }
}

/// What a bot moved away answers: not here.
fn moved() -> Answer {
    Err((404, GONE.into()))
}

/// The bot a held edit or a refusal is for, as its computer says it now, else as known then.
fn bot_of(said: &mut Value, known: Option<&Value>) {
    let Some(known) = known else {
        return;
    };
    if let Some(name) = known["name"].as_str().filter(|name| !name.is_empty()) {
        said["name"] = json!(name);
    }
    if let Some(look) = known.get("look").filter(|look| look.is_object()) {
        said["look"] = look.clone();
    }
}

/// What an older computer would drop silently: the edit waits until it can take them.
fn missing_feature(
    change: Option<&Map<String, Value>>,
    features: &[String],
) -> Option<&'static str> {
    let change = change?;
    let takes = |feature: &str| features.iter().any(|each| each == feature);
    if change.contains_key("effort") && !takes(EFFORT_FEATURE) {
        return Some("an effort");
    }
    if (change.contains_key("jobLine") || change.contains_key("jobDetails"))
        && !takes(JOB_FIELDS_FEATURE)
    {
        return Some("separate Job and Instructions changes");
    }
    None
}

/// Who asked, as a held edit keeps it: this computer (its app, or a Mac over SSH), or a paired
/// device by its key and pairing; none for anything else that reaches its port.
fn submitter_of(caller: &Caller) -> Option<Value> {
    use kuvo_server::{LocalOrigin, Transport};
    if let Some(pairing) = caller.pairing() {
        return Some(
            json!({ "kind": "paired", "key": encode(&pairing.key), "pairing": pairing.id }),
        );
    }
    match caller.transport() {
        Transport::Http {
            origin: LocalOrigin::App | LocalOrigin::Mac,
            ..
        }
        | Transport::Ssh { .. }
        | Transport::AddressedSsh => Some(json!({ "kind": "computer" })),
        _ => None,
    }
}

/// A kept submitter: this computer (`None`), or a paired device's key and pairing.
fn submitter_of_value(value: &Value) -> Option<Option<(String, String)>> {
    match value["kind"].as_str()? {
        "computer" => Some(None),
        "paired" => Some(Some((
            value["key"].as_str()?.to_owned(),
            value["pairing"].as_str()?.to_owned(),
        ))),
        _ => None,
    }
}

/// The bots of your other computers as `kuvo/bots` says them: each as you left it (the name and
/// look a change held here gives it), marked `waiting` or `refused`; then any this box only holds
/// an edit for, or heard a refusal of, that no computer lists now (`unlisted`).
pub(super) fn elsewhere_said(
    listed: Vec<Value>,
    held: &[Value],
    refused: &[Value],
    here: impl Fn(&str) -> bool,
) -> Vec<Value> {
    let edit_of = |id: &str| held.iter().find(|each| each["bot"] == id);
    let refusal_of = |id: &str| refused.iter().any(|each| each["bot"] == id);
    let left = |said: &mut Value, edit: Option<&Value>| {
        if let Some(name) = edit
            .and_then(|edit| edit["change"]["name"].as_str())
            .filter(|name| !name.is_empty())
        {
            said["name"] = json!(name);
        }
        if let Some(look) = edit
            .map(|edit| &edit["change"]["look"])
            .filter(|look| kuvo_peers::directory::Look::fits(Some(look)))
        {
            said["look"] =
                json!({ "color": look["color"], "shape": look["shape"], "face": look["face"] });
        }
    };
    let marks = |said: &mut Value, id: &str| {
        if edit_of(id).is_some() {
            said["waiting"] = json!(true);
        }
        if refusal_of(id) {
            said["refused"] = json!(true);
        }
    };
    let mut seen: HashSet<String> = listed
        .iter()
        .filter_map(|bot| bot["id"].as_str().map(str::to_owned))
        .collect();
    let mut all: Vec<Value> = listed
        .into_iter()
        .map(|mut bot| {
            let id = bot["id"].as_str().unwrap_or_default().to_owned();
            left(&mut bot, edit_of(&id));
            marks(&mut bot, &id);
            bot
        })
        .collect();
    for each in held.iter().chain(refused) {
        let id = each["bot"].as_str().unwrap_or_default();
        if seen.contains(id) || here(id) {
            continue;
        }
        seen.insert(id.to_owned());
        let mut said = json!({ "id": id });
        bot_of(&mut said, Some(each));
        left(&mut said, edit_of(id));
        said["box"] = each["box"].clone();
        said["computer"] = each["computer"].clone();
        marks(&mut said, id);
        said["unlisted"] = json!(true);
        all.push(said);
    }
    all
}
