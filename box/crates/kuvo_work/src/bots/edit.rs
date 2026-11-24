//! A bot here changed or deleted, as Edit Bot and Delete Bot… do: in its description, and in its
//! conversation, which is told as a bot is told.

use std::{collections::BTreeSet, path::Path};

use http::Method;
use serde_json::{Map, Value, json};

use super::{
    AGENTS_DOWN, Bots,
    here::{BotHere, MOVING, change_description, plain_name_of},
    job::{given_job, given_job_fields, job_of, stamp, write_job},
    kit::{NO_KIT, runs_with_agents},
    list::limits_from,
    make::{bot_look, bot_model},
    reach::{self, BOT_STEP, segment},
};

/// What a bot's work may do without asking, when you allow it.
const WORK_ACTIONS: &[&str] = &["makeProject", "cloneProject"];
/// The fields of a change whose times are noted (`edited.json`).
const EDITED_FIELDS: &[&str] = &[
    "name",
    "look",
    "personality",
    "job",
    "jobLine",
    "jobDetails",
    "model",
    "effort",
    "work",
];
/// How long a field's time is kept.
const EDITED_KEPT: i64 = 30 * 86_400_000;
const GONE: &str = "That bot isn’t there any more.";

type Answer = Result<Value, (u16, String)>;

fn no(status: u16, message: impl Into<String>) -> Answer {
    Err((status, message.into()))
}

fn same_model(a: Option<&Value>, b: Option<&Value>) -> bool {
    let field = |model: Option<&Value>, key: &str| {
        model
            .and_then(|model| model.get(key))
            .filter(|value| !value.is_null())
            .cloned()
    };
    ["id", "providerID", "variant"]
        .iter()
        .all(|key| field(a, key) == field(b, key))
}

/// What a change changes (from `before` to `after`), onto the description as it is now.
fn onto(
    now: Option<Map<String, Value>>,
    before: &Map<String, Value>,
    after: &Map<String, Value>,
) -> Map<String, Value> {
    let Some(mut next) = now else {
        return after.clone();
    };
    let keys: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    for key in keys {
        if before.get(key) == after.get(key) {
            continue;
        }
        match after.get(key) {
            Some(value) => {
                next.insert(key.clone(), value.clone());
            }
            None => {
                next.remove(key);
            }
        }
    }
    next
}

/// With what it may do without asking as it's kept now, unless the change names that itself.
fn keep_allowed(
    mut changed: Map<String, Value>,
    now: Option<&Map<String, Value>>,
    names: bool,
) -> Map<String, Value> {
    if names {
        return changed;
    }
    let kept = now
        .and_then(|now| now.get("work"))
        .and_then(Value::as_object)
        .and_then(|work| work.get("allowed"))
        .cloned();
    let mut work = changed
        .get("work")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    match kept {
        Some(kept) => {
            work.insert("allowed".into(), kept);
        }
        None => {
            work.remove("allowed");
        }
    }
    if work.is_empty() {
        changed.remove("work");
    } else {
        changed.insert("work".into(), Value::Object(work));
    }
    changed
}

/// `fields` of a bot (in `folder`) changed now, each unless it was changed later already.
fn note_edited(folder: &Path, fields: &[&str]) {
    if fields.is_empty() {
        return;
    }
    let now = jiff::Timestamp::now().as_millisecond();
    let Ok(_lock) = kuvo_store::locks::OwnedLock::acquire(
        &folder.join(".edited.lock"),
        std::time::Duration::from_secs(10),
    ) else {
        return;
    };
    let file = folder.join("edited.json");
    let mut times: Map<String, Value> = kuvo_store::files::text_of(&file)
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(field, at)| {
            let at = at.as_f64()?;
            ((now as f64) - at < EDITED_KEPT as f64)
                .then(|| (field, json!(at.min(now as f64) as i64)))
        })
        .collect();
    for field in fields {
        let was = times.get(*field).and_then(Value::as_f64).unwrap_or(0.0);
        times.insert((*field).into(), json!((was.max(now as f64)) as i64));
    }
    let text = serde_json::to_string(&Value::Object(times)).expect("JSON");
    let _ = kuvo_store::files::write_secret(&file, text.as_bytes());
}

impl Bots {
    /// `PATCH kuvo/bots/<id>`: a bot here changed as Edit Bot changes it.
    pub(super) async fn change(&self, id: &str, given: Value) -> Answer {
        let Some(mut here) = BotHere::read(&self.inner.bots, id) else {
            return no(404, GONE);
        };
        if here.moving() {
            return no(409, MOVING);
        }
        let Some(kit) = self.kit() else {
            return no(409, NO_KIT);
        };
        let Some(mut given) = given.as_object().cloned() else {
            return no(400, "That isn’t a change to a bot.");
        };
        let described = here.described.clone();
        let mut changed = described.clone();
        if given.contains_key("name") {
            let name = plain_name_of(given.get("name"));
            if name.is_empty() {
                return no(400, "A bot needs a name.");
            }
            changed.insert("name".into(), json!(name));
        }
        if given.contains_key("look") {
            if !kit.looks.fits(given.get("look")) {
                return no(400, "That isn’t a look a bot can have.");
            }
            changed.insert("look".into(), bot_look(&given["look"]));
        }
        if given.contains_key("personality") {
            let wanted = &given["personality"];
            let library = self.inner.setup.personalities();
            if wanted.is_null() {
                changed.remove("personality");
            } else if kit
                .personalities(&library)
                .iter()
                .any(|each| wanted.as_str() == Some(each.id.as_str()))
            {
                changed.insert("personality".into(), wanted.clone());
            } else {
                return no(
                    400,
                    format!(
                        "There’s no personality “{}”.",
                        super::here::js_string(wanted)
                    ),
                );
            }
        }
        let mut job = None;
        if let Some(wanted) = given.get("job") {
            job = Some(given_job(wanted).map_err(|error| (400, error.to_owned()))?);
        }
        let (job_line, job_details) =
            given_job_fields(&given).map_err(|error| (400, error.to_owned()))?;
        if given.contains_key("effort") {
            let names_model = given.contains_key("model");
            let base = if names_model {
                given.get("model").cloned()
            } else {
                described.get("model").cloned()
            };
            let Some(base) = base.as_ref().and_then(bot_model) else {
                return no(
                    400,
                    if names_model {
                        "That isn’t a model.".to_owned()
                    } else {
                        format!("{} has no model to set an effort for.", here.name())
                    },
                );
            };
            let variant = self.effort_of(&base, &given["effort"]).await?;
            let mut model = json!({"id": base["id"], "providerID": base["providerID"]});
            if let Some(variant) = variant {
                model["variant"] = json!(variant);
            }
            given.insert("model".into(), model);
        }
        if given.contains_key("model") {
            let Some(model) = given.get("model").and_then(bot_model) else {
                return no(400, "That isn’t a model.");
            };
            if !same_model(Some(&model), described.get("model")) {
                if runs_with_agents(Some(&model)) {
                    if !here.agents {
                        // (Carrying a conversation to the coding agents is theirs, not in this box yet.)
                        return no(503, AGENTS_DOWN);
                    }
                } else {
                    let Some(engine) = self.inner.engine.bot_engine().await else {
                        return no(503, "The engine isn’t running.");
                    };
                    if let Some(refused) =
                        reach::sign_in_refusal(&engine, &model, self.here()).await
                    {
                        return no(422, refused);
                    }
                }
            }
            changed.insert("model".into(), model);
        }
        let mut limits_changed = false;
        let names_allowed = given
            .get("work")
            .and_then(Value::as_object)
            .is_some_and(|work| work.contains_key("allowed"));
        if let Some(wanted) = given.get("work") {
            if !wanted.is_null() && !wanted.is_object() {
                return no(400, "That isn’t a bot’s work settings.");
            }
            let empty = Map::new();
            let asked = wanted.as_object().unwrap_or(&empty);
            let before = described
                .get("work")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let mut work = before.clone();
            if asked.contains_key("limits") || !asked.contains_key("allowed") {
                match limits_from(asked.get("limits")) {
                    Some(limits) => {
                        work.insert("limits".into(), Value::Object(limits));
                    }
                    None => {
                        work.remove("limits");
                    }
                }
            }
            if let Some(allowed) = asked.get("allowed") {
                let allowed: Vec<&str> = WORK_ACTIONS
                    .iter()
                    .copied()
                    .filter(|action| {
                        allowed
                            .as_array()
                            .is_some_and(|list| list.iter().any(|each| each == action))
                    })
                    .collect();
                if allowed.is_empty() {
                    work.remove("allowed");
                } else {
                    work.insert("allowed".into(), json!(allowed));
                }
            }
            limits_changed =
                serde_json::to_string(&work).ok() != serde_json::to_string(&before).ok();
            if work.is_empty() {
                changed.remove("work");
            } else {
                changed.insert("work".into(), Value::Object(work));
            }
        }
        let folder = here.folder.clone();
        let save = |changed: &Map<String, Value>| {
            let mut saved = None;
            change_description(
                &folder,
                |now| {
                    let next = keep_allowed(
                        onto(now.clone(), &described, changed),
                        now.as_ref(),
                        names_allowed,
                    );
                    saved = Some(next.clone());
                    Ok(Some(next))
                },
                false,
            )
            .map(|()| saved.expect("saved"))
        };
        // Only its work's settings changed: its description kept, nothing to tell it.
        if limits_changed && given.keys().all(|key| key == "work") {
            if let Err(error) = save(&changed) {
                return no(503, format!("The bot couldn’t be changed: {error}"));
            }
            self.changed();
            note_edited(&here.folder, &["work"]);
            return Ok(json!({ "bot": self.listed_one(id) }));
        }
        if here.agents {
            return no(503, AGENTS_DOWN);
        }
        let Some(engine) = self.inner.engine.bot_engine().await else {
            return no(503, "The engine isn’t running.");
        };
        let renamed = changed.get("name") != described.get("name");
        let was = job_of(&here, true);
        if job_line.is_some() || job_details.is_some() {
            let mut whole = job.clone().unwrap_or_else(|| was.clone());
            if let Some(line) = job_line {
                whole.line = line;
            }
            if let Some(details) = job_details {
                whole.details = details;
            }
            job = Some(whole);
        }
        let new_job = job
            .as_ref()
            .is_some_and(|job| job.line != was.line || job.details != was.details);
        let personality_changed = changed.get("personality") != described.get("personality");
        let retold = renamed || personality_changed || new_job;
        let switched = !same_model(changed.get("model"), described.get("model"));
        let looks = ["color", "shape", "face"].iter().any(|part| {
            changed.get("look").and_then(|look| look.get(part))
                != described.get("look").and_then(|look| look.get(part))
        });
        if retold || switched || looks || limits_changed {
            if renamed || switched || looks || personality_changed {
                changed.insert("updated".into(), json!(stamp()));
            }
            if new_job {
                let job = job.as_ref().expect("a new job");
                if let Err(error) = write_job(&here, job) {
                    return no(503, format!("The bot couldn’t be changed: {error}"));
                }
                changed.insert("job".into(), json!(job.line));
            }
            match save(&changed) {
                Ok(saved) => here.described = saved,
                Err(error) => return no(503, format!("The bot couldn’t be changed: {error}")),
            }
            // Its conversation hears of a new name, personality or model, once its details are
            // saved, whatever becomes of the request.
            let session = segment(&here.session);
            if renamed {
                let _ = engine
                    .call(
                        Method::PATCH,
                        &format!("api/session/{session}"),
                        Some(&json!({"title": changed["name"]})),
                        BOT_STEP,
                    )
                    .await;
            }
            if switched && let Some(model) = changed.get("model") {
                let _ = engine
                    .call(
                        Method::POST,
                        &format!("api/session/{session}/model"),
                        Some(&json!({ "model": model })),
                        BOT_STEP,
                    )
                    .await;
            }
            if retold {
                let text = self.told_now(&kit, &here, &changed);
                Self::tell_bot(&engine, &here, &here.session, &text).await;
            }
            self.changed();
        }
        let fields: Vec<&str> = EDITED_FIELDS
            .iter()
            .copied()
            .filter(|field| given.contains_key(*field))
            .collect();
        note_edited(&here.folder, &fields);
        Ok(json!({ "bot": self.listed_one(id) }))
    }

    /// The effort `given` names for `model`, as its variant: none for null or "" (the model's own).
    async fn effort_of(
        &self,
        model: &Value,
        given: &Value,
    ) -> Result<Option<String>, (u16, String)> {
        let effort = match given {
            Value::Null => return Ok(None),
            Value::String(text) if text.is_empty() => return Ok(None),
            Value::String(text) => text.clone(),
            _ => return no(400, "That isn’t an effort.").map(|_| None),
        };
        let here = self.here();
        let id = model["id"].as_str().unwrap_or_default();
        let models = if runs_with_agents(Some(model)) {
            Vec::new()
        } else {
            match self.inner.engine.bot_engine().await {
                Some(engine) => reach::usable_models(&engine).await,
                None => Vec::new(),
            }
        };
        let Some(listed) = reach::model_named(model, &models) else {
            if models.is_empty() {
                return no(
                    503,
                    format!(
                        "The efforts of {id} can’t be read on {here} now. Try again in a moment."
                    ),
                )
                .map(|_| None);
            }
            return no(422, format!("{id} can’t be used on {here} now, so its effort can’t be set: sign in to it in Settings › Accounts, or choose another model.")).map(|_| None);
        };
        if listed.efforts.is_empty() {
            return no(
                400,
                format!("{} has no efforts to choose from.", listed.name),
            )
            .map(|_| None);
        }
        if !listed.efforts.contains(&effort) {
            return no(
                400,
                format!(
                    "{} has no “{effort}” effort: its efforts are {}.",
                    listed.name,
                    listed.efforts.join(", ")
                ),
            )
            .map(|_| None);
        }
        Ok(Some(effort))
    }

    /// `DELETE kuvo/bots/<id>`: a bot here goes, its folder (its memory, wake-ups and what it
    /// made) to Kuvo Code's trash here, then its conversation.
    pub(super) async fn delete(&self, id: &str) -> Answer {
        let Some(here) = BotHere::read(&self.inner.bots, id) else {
            return no(404, GONE);
        };
        if here.moving() {
            return no(409, MOVING);
        }
        let trashed = (|| {
            if !self.inner.is_mac {
                self.note_deleted(id)?;
            }
            let when = jiff::Zoned::now().strftime("%Y%m%d-%H%M%S").to_string();
            let trash = self
                .inner
                .home
                .join("trash")
                .join(format!("{when}-{}", std::process::id()));
            std::fs::create_dir_all(&trash)?;
            let trashed = trash.join(id);
            std::fs::rename(&here.folder, &trashed)?;
            Ok::<_, std::io::Error>(trashed)
        })();
        if let Err(error) = trashed {
            return no(503, format!("The bot couldn’t be deleted: {error}"));
        }
        // All it has elsewhere, noted as left before any of it is tried: a box that stops meanwhile
        // finishes it as it starts again.
        let mut left = json!({"id": id, "session": here.session, "agents": here.agents, "groups": true, "conversation": true});
        self.note_left(id, Some(&left));
        let still = self.leave_behind(&mut left).await;
        self.note_left(id, still.then_some(&left));
        self.changed();
        Ok(json!({}))
    }

    /// A deleted bot's conversation gone as far as it can be now; its group chats are the coding
    /// agents' (not in this box yet), so they're left noted. Whether anything is still left.
    async fn leave_behind(&self, left: &mut Value) -> bool {
        if left["conversation"] == true
            && left["agents"] != true
            && let Some(engine) = self.inner.engine.bot_engine().await
        {
            let session = left["session"].as_str().unwrap_or_default().to_owned();
            let gone = engine.delete_session(&session).await;
            left["conversation"] = json!(!gone);
        }
        left["groups"] == true || left["conversation"] == true
    }

    fn left_file(&self) -> std::path::PathBuf {
        self.inner.run.join("bots-left.json")
    }

    fn bots_left(&self) -> Vec<Value> {
        let kept: Value = kuvo_store::files::text_of(&self.left_file())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        kept["left"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|each| {
                each["id"].as_str().is_some_and(super::here::bot_id) && each["session"].is_string()
            })
            .cloned()
            .collect()
    }

    fn note_left(&self, id: &str, left: Option<&Value>) {
        let mut bots: Vec<Value> = self
            .bots_left()
            .into_iter()
            .filter(|each| each["id"] != id)
            .collect();
        bots.extend(left.cloned());
        let file = self.left_file();
        let _ = (|| {
            std::fs::create_dir_all(&self.inner.run)?;
            if bots.is_empty() {
                return match std::fs::remove_file(&file) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                    _ => Ok(()),
                };
            }
            let new = file.with_extension("json.new");
            std::fs::write(
                &new,
                serde_json::to_string(&json!({ "left": bots })).expect("JSON"),
            )?;
            std::fs::rename(&new, &file)
        })();
    }

    /// What deleted bots left behind, gone where it can be now: as the box starts.
    pub async fn finish_deletes(&self) {
        for mut left in self.bots_left() {
            let id = left["id"].as_str().unwrap_or_default().to_owned();
            // (Here again, a move brought it back: its conversation and chats are its own.)
            if BotHere::read(&self.inner.bots, &id).is_some() {
                self.note_left(&id, None);
                continue;
            }
            let before = left.clone();
            let still = self.leave_behind(&mut left).await;
            let after = still.then_some(left);
            if after.as_ref() != Some(&before) {
                self.note_left(&id, after.as_ref());
            }
        }
    }

    // COMPAT(beta.1 deleted-bots): `bots/deleted.json`, for a Mac of 0.8.0-beta.1 or earlier.
    fn note_deleted(&self, id: &str) -> std::io::Result<()> {
        let file = self.inner.bots.join("deleted.json");
        let kept: Value = kuvo_store::files::text_of(&file)
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let mut deleted = vec![json!(id)];
        deleted.extend(
            kept["deleted"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|each| each.is_string() && *each != id)
                .cloned(),
        );
        deleted.truncate(500);
        std::fs::create_dir_all(&self.inner.bots)?;
        let new = file.with_extension("json.new");
        std::fs::write(
            &new,
            serde_json::to_string(&json!({ "deleted": deleted })).expect("JSON"),
        )?;
        std::fs::rename(&new, &file)
    }
}
