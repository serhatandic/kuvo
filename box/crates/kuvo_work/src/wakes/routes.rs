//! `kuvo/wakes` and `kuvo/wakes/<session>/…`: a conversation's wake-ups and watches, listed,
//! followed, added, held, cancelled and sent now, as the bar over a client's composer does.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use http::StatusCode;
use kuvo_protocol::routes::{
    get_kuvo_wakes as list, get_kuvo_wakes_events as list_events, get_kuvo_wakes_session as one,
    get_kuvo_wakes_session_events as one_events, post_kuvo_wakes_session as add,
    post_kuvo_wakes_session_cancel as cancel, post_kuvo_wakes_session_now as now_route,
    post_kuvo_wakes_session_pause as pause, post_kuvo_wakes_session_watch as watch_route,
};
use kuvo_server::{Access, JsonLine, Refusal, Registry, RouteCall, RouteReply};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};

use super::{
    Wakes, clock,
    fire::{self, FolderLock},
    folder::{self, Detail, Listing, WAKE_LIMIT, WATCH_LIMIT, letters, names, text},
};

const MOVING: &str = "It’s moving to another computer. Try again once it’s there.";

fn typed<T: DeserializeOwned>(value: impl Serialize) -> Result<T, Refusal> {
    serde_json::to_value(value)
        .and_then(serde_json::from_value)
        .map_err(|error| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

fn empty(status: StatusCode) -> Refusal {
    Refusal::json(status, json!({}))
}

fn refused(status: StatusCode, message: impl Into<String>) -> Refusal {
    Refusal::new(status, message)
}

/// A session's line in `kuvo/wakes`: its soonest wake-up, whether it watches, and the rest whole.
#[derive(Serialize)]
struct Row {
    session: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    next: Option<f64>,
    watching: bool,
    #[serde(flatten)]
    listing: Listing,
}

impl Row {
    fn of(session: String, listing: Listing) -> Self {
        Self {
            session,
            next: listing.wakes.first().map(|wake| wake.due),
            watching: !listing.watches.is_empty(),
            listing,
        }
    }
}

impl Wakes {
    /// Every conversation here with wake-ups or watches of its own (bots' come with
    /// `kuvo/bots`), as `kuvo/wakes` lists them.
    fn sessions(&self) -> Vec<Row> {
        let roots = self.roots();
        let away = roots.away();
        names(&roots.sessions)
            .into_iter()
            .filter_map(|name| {
                let folder = roots.sessions.join(name);
                let session = text(&folder.join("session")).trim().to_owned();
                if session.is_empty() || away.contains(&session) {
                    return None;
                }
                let listing = folder::read(&folder, Detail::Line);
                (!listing.wakes.is_empty() || !listing.watches.is_empty())
                    .then(|| Row::of(session, listing))
            })
            .collect()
    }

    /// Every conversation here with wake-ups, watches or a pause, a bot's or its own, whole: what
    /// a client keeps of them, followed.
    fn everything(&self) -> Vec<Row> {
        let roots = self.roots();
        let away = roots.away();
        roots
            .folders()
            .filter_map(|(_, folder)| {
                let session = text(&folder.join("session")).trim().to_owned();
                if session.is_empty() || away.contains(&session) {
                    return None;
                }
                let listing = folder::read(&folder, Detail::Whole);
                (!listing.wakes.is_empty() || !listing.watches.is_empty() || listing.paused)
                    .then(|| Row::of(session, listing))
            })
            .collect()
    }

    fn listed(&self, session: &str) -> Listing {
        self.roots()
            .folder_of(session)
            .map(|folder| folder::read(&folder, Detail::Line))
            .unwrap_or_default()
    }
}

fn session_of<T>(call: &RouteCall<T>) -> Result<String, Refusal> {
    let raw = call.param("session").unwrap_or_default();
    percent_encoding::percent_decode_str(raw)
        .decode_utf8()
        .map(|session| session.into_owned())
        .map_err(|_| refused(StatusCode::NOT_FOUND, "No such session."))
}

/// What a client sent, each field checked as it's read; nothing for what isn't JSON.
fn given<T>(call: &RouteCall<T>) -> Map<String, Value> {
    serde_json::from_slice::<Value>(&call.request.body)
        .ok()
        .and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default()
}

fn trimmed(given: &Map<String, Value>, field: &str) -> String {
    given
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_owned()
}

/// A value as JavaScript's `Number` reads it, for `due`.
fn number(value: Option<&Value>) -> f64 {
    match value {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(flag)) => f64::from(u8::from(*flag)),
        Some(Value::Number(number)) => number.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(text)) if text.trim().is_empty() => 0.0,
        Some(Value::String(text)) => text.trim().parse().unwrap_or(f64::NAN),
        Some(_) => f64::NAN,
    }
}

/// A value as JavaScript's `String` writes it, for `every`.
fn string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        _ => "[object Object]".into(),
    }
}

fn random_letters(count: usize) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut bytes = vec![0u8; count];
    let _ = getrandom::fill(&mut bytes);
    bytes
        .into_iter()
        .map(|byte| char::from(ALPHABET[usize::from(byte) % ALPHABET.len()]))
        .collect()
}

/// Something `kuvo-wake` would refuse, in its words (a client shows them).
struct Said(String);

impl Wakes {
    /// A conversation's folder for its first wake-up or watch, made as `kuvo-wake` makes one:
    /// its session, its folder, and held while it's archived. None when it isn't a conversation
    /// here.
    async fn folder_for(&self, session: &str) -> Option<PathBuf> {
        if !folder::session_id(session) {
            return None;
        }
        let engine = self.inner.engine.wake_engine(true).await?;
        let found = engine.session(session).await.ok().flatten()?;
        let folder = self.roots().sessions.join(session);
        kuvo_store::files::create_private_dirs(&folder).ok()?;
        fs::write(folder.join("session"), session).ok()?;
        let directory = found["location"]["directory"]
            .as_str()
            .or(found["directory"].as_str())
            .unwrap_or_default();
        fs::write(folder.join("directory"), directory).ok()?;
        if found["metadata"]["kuvo.archived"] == Value::Bool(true) {
            fs::write(folder.join("paused"), "archived").ok()?;
        }
        Some(folder)
    }

    /// A wake-up added as `kuvo-wake add` adds one: on the folder's clock, a weekday one moved to
    /// the next weekday at the same time.
    fn add_wake(folder: &Path, due: i64, every: Option<&str>, note: &str) -> Result<(), Said> {
        let kept_in = || Said(format!("Couldn't keep it in {}.", folder.display()));
        if let Some(every) = every
            && !clock::repeats(every)
        {
            return Err(Said(format!(
                "Couldn't read the repeat \"{every}\". Use 30m, 2h, 1d, 1w (at least 5m), or weekdays."
            )));
        }
        let every = every.map(|every| every.split('@').next().unwrap_or_default());
        let zone = clock::zone_of(folder);
        let due = if every == Some("weekdays") {
            clock::next_weekday(&zone, due)
        } else {
            due
        };
        if due <= fire::now() + 30 {
            return Err(Said(
                "That's not in the future. Wake-ups are for later; do it now instead.".into(),
            ));
        }
        let waiting = names(&folder.join("wakes")).len() + names(&folder.join("inbox/wakes")).len();
        if waiting >= WAKE_LIMIT {
            return Err(Said(
                "You already have 50 wake-ups waiting. Cancel some first.".into(),
            ));
        }
        let repeat = every.map(|every| clock::kept(&zone, due, every));
        let name = match &repeat {
            Some(repeat) => format!("{due}-{}-{repeat}", random_letters(6)),
            None => format!("{due}-{}", random_letters(6)),
        };
        let wakes = folder.join("wakes");
        fs::create_dir_all(&wakes).map_err(|_| kept_in())?;
        let fresh = folder.join(format!(".new-{name}"));
        fs::write(&fresh, note)
            .and_then(|()| fs::rename(&fresh, wakes.join(&name)))
            .map_err(|_| {
                let _ = fs::remove_file(&fresh);
                kept_in()
            })
    }

    /// A watch made as `kuvo-wake watch` makes one.
    fn add_watch(folder: &Path, every: &str, check: &str, note: &str) -> Result<(), Said> {
        let period = every.len() > 1 && {
            let (count, unit) = every.split_at(every.len() - 1);
            !count.starts_with('0')
                && count.len() <= 6
                && count.bytes().all(|b| b.is_ascii_digit())
                && match (unit, count.parse::<u32>()) {
                    ("m", Ok(count)) => count >= 5,
                    ("h" | "d", Ok(count)) => count >= 1,
                    _ => false,
                }
        };
        if !period {
            return Err(Said(format!(
                "Couldn't read how often, \"{every}\". Use 5m or more, 2h or 1d."
            )));
        }
        let count =
            names(&folder.join("watches")).len() + names(&folder.join("inbox/watches")).len();
        if count >= WATCH_LIMIT {
            return Err(Said(
                "You already have 20 watches. Cancel some first.".into(),
            ));
        }
        let letters = random_letters(6);
        let fresh = folder.join(format!(".new-{letters}"));
        let made = (|| {
            fs::create_dir_all(&fresh)?;
            fs::write(fresh.join("every"), every)?;
            fs::write(fresh.join("check"), check)?;
            fs::write(fresh.join("note"), note)?;
            fs::write(fresh.join("next"), fire::now().to_string())?;
            fs::create_dir_all(folder.join("watches"))?;
            fs::rename(&fresh, folder.join("watches").join(&letters))
        })();
        made.map_err(|_| {
            let _ = fs::remove_dir_all(&fresh);
            Said(format!("Couldn't keep it in {}.", folder.display()))
        })
    }

    async fn existing_or_new(&self, session: &str) -> Result<PathBuf, Refusal> {
        match self.roots().folder_of(session) {
            Some(folder) => Ok(folder),
            None => self
                .folder_for(session)
                .await
                .ok_or_else(|| empty(StatusCode::NOT_FOUND)),
        }
    }

    async fn post_add(&self, session: &str, given: Map<String, Value>) -> Result<Listing, Refusal> {
        let due = number(given.get("due"));
        let note = trimmed(&given, "note");
        let every = given
            .get("every")
            .filter(|every| !every.is_null())
            .map(string);
        if !due.is_finite() || note.is_empty() {
            return Err(refused(
                StatusCode::BAD_REQUEST,
                "Send when it comes (`due`, seconds since 1970) and what it says (`note`).",
            ));
        }
        if every.as_deref().is_some_and(|every| !repeat_word(every)) {
            return Err(refused(
                StatusCode::BAD_REQUEST,
                "That isn’t a repeat: 30m, 2h, 1d, 1w or weekdays.",
            ));
        }
        if due < fire::now() as f64 + 60.0 {
            return Err(refused(
                StatusCode::BAD_REQUEST,
                "Pick a time at least a minute from now.",
            ));
        }
        let folder = self.existing_or_new(session).await?;
        if folder::moving(&folder) {
            return Err(refused(StatusCode::CONFLICT, MOVING));
        }
        if folder::read(&folder, Detail::Line).wakes.len() >= WAKE_LIMIT {
            return Err(refused(
                StatusCode::CONFLICT,
                format!("There are {WAKE_LIMIT} wake-ups waiting already. Cancel some first."),
            ));
        }
        Self::add_wake(&folder, due.floor() as i64, every.as_deref(), &note)
            .map_err(|Said(words)| refused(StatusCode::BAD_REQUEST, words))?;
        self.changed();
        Ok(folder::read(&folder, Detail::Line))
    }

    async fn post_watch(
        &self,
        session: &str,
        given: Map<String, Value>,
    ) -> Result<Listing, Refusal> {
        let (every, check, note) = (
            trimmed(&given, "every"),
            trimmed(&given, "check"),
            trimmed(&given, "note"),
        );
        if every.is_empty() || check.is_empty() || note.is_empty() {
            return Err(refused(
                StatusCode::BAD_REQUEST,
                "Send how often (`every`, like 5m, 2h or 1d), the check to run (`check`) and what to do when it changes (`note`).",
            ));
        }
        let folder = self.existing_or_new(session).await?;
        if folder::moving(&folder) {
            return Err(refused(StatusCode::CONFLICT, MOVING));
        }
        Self::add_watch(&folder, &every, &check, &note)
            .map_err(|Said(words)| refused(StatusCode::BAD_REQUEST, words))?;
        self.changed();
        Ok(folder::read(&folder, Detail::Line))
    }

    async fn post_pause(&self, session: &str, given: Map<String, Value>) -> Result<Value, Refusal> {
        let folder = self.roots().folder_of(session);
        // Held while it's archived and let go when it's back, as its archiving says: a hold of
        // your own stays through both.
        if let Some(archived) = given.get("archived").and_then(Value::as_bool) {
            if folder.as_deref().is_some_and(folder::moving) {
                return Err(refused(StatusCode::CONFLICT, MOVING));
            }
            self.hold_archived(session, archived);
            if archived
                && given.get("stop") != Some(&Value::Bool(false))
                && let Some(engine) = self.inner.engine.wake_engine(false).await
                && let Ok(Some(found)) = engine.session(session).await
                && found["metadata"]["kuvo.archived"] == Value::Bool(true)
            {
                engine.stop_if_waiting(session).await;
            }
            self.changed();
            return Ok(json!(self.listed(session)));
        }
        let Some(paused) = given.get("paused").and_then(Value::as_bool) else {
            return Err(refused(
                StatusCode::BAD_REQUEST,
                "Send {\"paused\": true} or false.",
            ));
        };
        let Some(folder) = folder else {
            return Ok(json!(Listing::default()));
        };
        if folder::moving(&folder) {
            return Err(refused(StatusCode::CONFLICT, MOVING));
        }
        if !paused && restore_review(&folder, &given) {
            return Err(restore_refusal());
        }
        let hold = folder.join("paused");
        if paused {
            let _ = fs::write(&hold, "paused");
        } else if folder::hold(&folder).as_deref() != Some("archived") {
            let _ = fs::remove_file(&hold);
        }
        self.changed();
        Ok(json!(folder::read(&folder, Detail::Line)))
    }

    /// A conversation archived (or brought back) elsewhere: its wake-ups wait while it's archived
    /// and come back with it. A hold of your own stays; a move's becomes archiving's.
    fn hold_archived(&self, session: &str, archived: bool) {
        if !folder::session_id(session) {
            return;
        }
        let folder = self.roots().sessions.join(session);
        let file = folder.join("paused");
        let held = folder::hold(&folder);
        if archived {
            if folder.exists() && held.as_deref().is_none_or(|held| held == "moving") {
                let _ = fs::write(&file, "archived");
            }
        } else if matches!(held.as_deref(), Some("archived" | "")) {
            let _ = fs::remove_file(&file);
        }
    }

    async fn post_cancel(
        &self,
        session: &str,
        given: Map<String, Value>,
        deadline: Duration,
    ) -> Result<(), Refusal> {
        let folder = self
            .roots()
            .folder_of(session)
            .ok_or_else(|| empty(StatusCode::NOT_FOUND))?;
        if folder::moving(&folder) {
            return Err(refused(StatusCode::CONFLICT, MOVING));
        }
        let listing = folder::read(&folder, Detail::Line);
        let wanted = given.get("wake").and_then(Value::as_str);
        let chosen = wanted.and_then(|wanted| {
            listing
                .wakes
                .iter()
                .find(|wake| wake.id == wanted || wake.letters == wanted)
        });
        if let Some(wake) = chosen.filter(|wake| letters(&wake.letters)) {
            // A round holds the folder above around both sending and moving the file on.
            let lock = folder
                .parent()
                .map(|root| root.join(".firing"))
                .unwrap_or_default();
            let until = Instant::now() + deadline;
            let _held = loop {
                if let Some(held) = FolderLock::take(lock.clone(), Duration::from_secs(600)) {
                    break held;
                }
                if Instant::now() >= until {
                    return Err(refused(
                        StatusCode::CONFLICT,
                        "A wake-up is being sent. Try canceling again in a moment.",
                    ));
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            };
            if folder::moving(&folder) || self.roots().away().contains(session) {
                return Err(refused(StatusCode::CONFLICT, MOVING));
            }
            cancel_wake(&folder, &wake.letters);
            self.changed();
            return Ok(());
        }
        let watch = given.get("watch").and_then(Value::as_str);
        if let Some(watch) = watch.filter(|watch| letters(watch))
            && listing.watches.iter().any(|each| each.letters == watch)
        {
            let _ = tokio::fs::remove_dir_all(folder.join("watches").join(watch)).await;
            self.changed();
            return Ok(());
        }
        Err(empty(StatusCode::NOT_FOUND))
    }

    async fn post_now(&self, session: &str, given: Map<String, Value>) -> Result<(), Refusal> {
        let folder = self
            .roots()
            .folder_of(session)
            .ok_or_else(|| empty(StatusCode::NOT_FOUND))?;
        if folder::moving(&folder) {
            return Err(refused(StatusCode::CONFLICT, MOVING));
        }
        if restore_review(&folder, &given) {
            return Err(restore_refusal());
        }
        let wanted = given.get("wake").and_then(Value::as_str);
        let listing = folder::read(&folder, Detail::Line);
        let wake = wanted
            .and_then(|wanted| {
                listing
                    .wakes
                    .into_iter()
                    .find(|wake| wake.id == wanted || wake.letters == wanted)
            })
            .ok_or_else(|| empty(StatusCode::NOT_FOUND))?;
        let key = format!("{}/{}", folder.display(), wake.id);
        if !self
            .inner
            .waking_now
            .lock()
            .expect("waking set")
            .insert(key.clone())
        {
            return Ok(());
        }
        let _sending = Sending(self, key);
        // The whole note, as its time would send it, not the line it's listed by.
        let Some(note) = kuvo_store::files::text_of(&folder.join("wakes").join(&wake.id)) else {
            return Err(empty(StatusCode::NOT_FOUND));
        };
        if text(&folder.join("via")).trim() == "bridge" {
            return Err(refused(
                StatusCode::SERVICE_UNAVAILABLE,
                "The coding agents are starting again. Try again in a moment.",
            ));
        }
        let engine =
            self.inner.engine.wake_engine(true).await.ok_or_else(|| {
                refused(StatusCode::SERVICE_UNAVAILABLE, "The engine isn’t running.")
            })?;
        if let Some(directory) = kuvo_store::files::text_of(&folder.join("directory"))
            && !directory.is_empty()
        {
            engine.wait_for_tools(&directory).await;
        }
        let summary = if letters(&wake.letters) {
            text(&folder.join("summaries").join(&wake.letters))
                .trim()
                .to_owned()
        } else {
            String::new()
        };
        engine
            .deliver(
                session,
                &note,
                Some(summary.as_str()).filter(|summary| !summary.is_empty()),
            )
            .await
            .map_err(|_| {
                refused(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "The wake-up couldn’t be sent.",
                )
            })?;
        if wake.every.is_none() {
            cancel_wake(&folder, &wake.letters);
        }
        self.changed();
        Ok(())
    }
}

/// Wake Up Now's mark on one wake-up, taken off once it's done with.
struct Sending<'a>(&'a Wakes, String);

impl Drop for Sending<'_> {
    fn drop(&mut self) {
        self.0
            .inner
            .waking_now
            .lock()
            .expect("waking set")
            .remove(&self.1);
    }
}

fn restore_review(folder: &Path, given: &Map<String, Value>) -> bool {
    folder::hold(folder).as_deref() == Some("restore-review")
        && given.get("confirmRestoreRetry") != Some(&Value::Bool(true))
}

fn restore_refusal() -> Refusal {
    Refusal::json(
        StatusCode::CONFLICT,
        json!({
            "message": "Check whether this restored work already ran before retrying.",
            "confirmRestoreRetry": true,
        }),
    )
}

/// Whether a client's repeat reads as one (`kuvo-wake` checks it further).
fn repeat_word(every: &str) -> bool {
    every == "weekdays"
        || every.len() >= 2
            && every.len() <= 7
            && every.as_bytes()[0] != b'0'
            && every[..every.len() - 1].bytes().all(|b| b.is_ascii_digit())
            && matches!(every.as_bytes()[every.len() - 1], b'm' | b'h' | b'd' | b'w')
}

/// Takes a wake-up off the schedule by its letters, which a repeating one keeps as its time moves
/// on, with its summary.
fn cancel_wake(folder: &Path, wake_letters: &str) {
    if !letters(wake_letters) {
        return;
    }
    for name in names(&folder.join("wakes")) {
        if folder::WakeName::parse(&name).is_some_and(|wake| wake.letters == wake_letters) {
            let _ = fs::remove_file(folder.join("wakes").join(name));
        }
    }
    let _ = fs::remove_file(folder.join("summaries").join(wake_letters));
}

pub(super) fn register(wakes: Wakes, routes: &mut Registry) {
    let here = wakes.clone();
    routes.register::<list::Spec, _, _>(Access::Any, move |_| {
        let wakes = here.clone();
        async move {
            let sessions = tokio::task::spawn_blocking(move || wakes.sessions())
                .await
                .unwrap_or_default();
            Ok(RouteReply::json::<200>(typed(
                json!({ "sessions": sessions }),
            )?))
        }
    });
    let here = wakes.clone();
    routes.register::<list_events::Spec, _, _>(Access::Any, move |call| {
        let wakes = here.clone();
        async move {
            Ok(RouteReply::ndjson::<200>(followed(
                wakes.clone(),
                call,
                move |wakes| json!({ "sessions": wakes.everything() }),
            )))
        }
    });
    let here = wakes.clone();
    routes.register::<one::Spec, _, _>(Access::Any, move |call| {
        let wakes = here.clone();
        async move {
            let session = session_of(&call)?;
            Ok(RouteReply::json::<200>(typed(wakes.listed(&session))?))
        }
    });
    let here = wakes.clone();
    routes.register::<one_events::Spec, _, _>(Access::Any, move |call| {
        let wakes = here.clone();
        async move {
            let session = session_of(&call)?;
            Ok(RouteReply::ndjson::<200>(followed(
                wakes.clone(),
                call,
                move |wakes| json!(wakes.listed(&session)),
            )))
        }
    });
    let here = wakes.clone();
    routes.register::<add::Spec, _, _>(Access::Any, move |call| {
        let wakes = here.clone();
        async move {
            let session = posted(&wakes, &call)?;
            let listing = wakes.post_add(&session, given(&call)).await?;
            Ok(RouteReply::json::<200>(typed(listing)?))
        }
    });
    let here = wakes.clone();
    routes.register::<watch_route::Spec, _, _>(Access::Any, move |call| {
        let wakes = here.clone();
        async move {
            let session = posted(&wakes, &call)?;
            let listing = wakes.post_watch(&session, given(&call)).await?;
            Ok(RouteReply::json::<200>(typed(listing)?))
        }
    });
    let here = wakes.clone();
    routes.register::<pause::Spec, _, _>(Access::Any, move |call| {
        let wakes = here.clone();
        async move {
            let session = posted(&wakes, &call)?;
            let listing = wakes.post_pause(&session, given(&call)).await?;
            Ok(RouteReply::json::<200>(typed(listing)?))
        }
    });
    let here = wakes.clone();
    routes.register::<cancel::Spec, _, _>(Access::Any, move |call| {
        let wakes = here.clone();
        async move {
            let session = posted(&wakes, &call)?;
            wakes
                .post_cancel(&session, given(&call), Duration::from_secs(2))
                .await?;
            Ok(RouteReply::json::<200>(typed(json!({}))?))
        }
    });
    let here = wakes;
    routes.register::<now_route::Spec, _, _>(Access::Any, move |call| {
        let wakes = here.clone();
        async move {
            let session = posted(&wakes, &call)?;
            wakes.post_now(&session, given(&call)).await?;
            Ok(RouteReply::json::<200>(typed(json!({}))?))
        }
    });
}

/// A session that's posted to: refused while it's on its way to another computer.
fn posted<T>(wakes: &Wakes, call: &RouteCall<T>) -> Result<String, Refusal> {
    let session = session_of(call)?;
    if wakes.roots().away().contains(&session) {
        return Err(refused(StatusCode::CONFLICT, MOVING));
    }
    Ok(session)
}

/// A line now, and again as it changes: made here at once, or beside the box as the folders are
/// looked at again every two seconds.
fn followed<R, T>(
    wakes: Wakes,
    call: RouteCall<T>,
    said: impl Fn(&Wakes) -> Value + Send + Sync + 'static,
) -> impl futures_core::Stream<Item = std::io::Result<JsonLine<R, 200>>> + Send + 'static
where
    R: kuvo_protocol::JsonStatus<200> + 'static,
    <R as kuvo_protocol::JsonStatus<200>>::Body: DeserializeOwned + Send,
    T: Send + 'static,
{
    let said = std::sync::Arc::new(said);
    async_stream::try_stream! {
        let mut changes = wakes.changes();
        let mut previous: Option<Value> = None;
        loop {
            changes.borrow_and_update();
            let (looked, said) = (wakes.clone(), said.clone());
            let value = tokio::task::spawn_blocking(move || said(&looked)).await.map_err(std::io::Error::other)?;
            if previous.as_ref() != Some(&value) {
                let line = JsonLine::new(serde_json::from_value(value.clone())?)?;
                previous = Some(value);
                yield line;
            }
            tokio::select! {
                () = call.cancel.cancelled() => break,
                _ = changes.changed() => {}
                () = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
        }
    }
}
