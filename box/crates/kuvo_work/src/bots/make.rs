//! New bots made here (`POST kuvo/bots`), as New Bot makes them: the bot's folder, its
//! conversation marked as its and kept with this computer's default approval, what it's told, and
//! the template's job for your composer, or else its greeting, so it writes first.
//!
//! New Bot sent again with the same `request` (a phone's connection dropped and it tried again)
//! is answered as the first was, while it's being made and for an hour after, rather than made
//! twice. Making goes on whatever becomes of the request that asked.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use http::Method;
use kuvo_engine::{BotEngine, NoteEdits};
use serde_json::{Map, Value, json};
use tokio::sync::{Mutex, watch};

use super::{
    AGENTS_DOWN, Bots,
    here::{DESCRIPTION, js_string, plain_name_of},
    job::{first_message_of, stamp},
    kit::{JobText, NO_KIT, runs_with_agents},
    reach::{self, BOT_STEP, segment},
    tell::{digest, note_told, permission},
};
use crate::approval::Rule;

const MADE_FOR: Duration = Duration::from_secs(3600);

/// What `POST kuvo/bots` answers: 201 and the bot made, or a status and why not.
pub type Made = Result<Value, (u16, String)>;

/// The answers New Bot's requests got, by their ids, while they're kept.
#[derive(Default)]
pub struct Requests {
    answers: HashMap<String, (watch::Receiver<Option<Made>>, Instant)>,
}

fn request_id(given: &Value) -> Option<String> {
    let id = given.get("request")?.as_str()?;
    ((8..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    .then(|| id.to_owned())
}

/// A model as a bot keeps it: its id, its provider, and its variant where that's text.
pub fn bot_model(model: &Value) -> Option<Value> {
    let id = model.get("id")?.as_str()?;
    let provider = model.get("providerID")?.as_str()?;
    let mut kept = json!({"id": id, "providerID": provider});
    if let Some(variant) = model.get("variant").and_then(Value::as_str) {
        kept["variant"] = json!(variant);
    }
    Some(kept)
}

/// A look as a bot keeps it: its own three fields.
pub fn bot_look(look: &Value) -> Value {
    json!({"color": look["color"], "shape": look["shape"], "face": look["face"]})
}

/// A new bot's id: `bot_` and ten letters and digits.
pub fn new_id() -> String {
    let mut bytes = [0u8; 10];
    getrandom::fill(&mut bytes).expect("randomness");
    let letters = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let tail: String = bytes
        .iter()
        .map(|byte| letters[usize::from(*byte) % 36] as char)
        .collect();
    format!("bot_{tail}")
}

fn pick(list: &[String]) -> Value {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).expect("randomness");
    let at = u32::from_le_bytes(bytes) as usize % list.len().max(1);
    list.get(at).map_or(Value::Null, |each| json!(each))
}

/// The directories of the box's projects, as New Bot offers them.
pub fn box_projects(home: &Path) -> Vec<(String, String)> {
    let kept: Value = kuvo_store::files::text_of(&home.join("box/projects.json"))
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    kept["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|each| each["removed"] != true)
        .filter_map(|each| {
            let directory = each["directory"].as_str()?;
            Path::new(directory).is_absolute().then_some(())?;
            let name = each["name"]
                .as_str()
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    Path::new(directory)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()
                });
            Some((directory.to_owned(), name))
        })
        .collect()
}

impl Bots {
    /// `kuvo/bots/templates`: what New Bot offers here, the model a bot starts on here (by the
    /// box's rule) and the projects it can work in.
    pub(super) async fn templates(&self) -> Result<Value, kuvo_server::Refusal> {
        let Some(kit) = self.kit() else {
            return Err(kuvo_server::Refusal::new(
                http::StatusCode::CONFLICT,
                NO_KIT,
            ));
        };
        let starting = match self.inner.engine.bot_engine().await {
            Some(engine) => reach::starting_model(&engine).await,
            None => None,
        };
        let machine = self.inner.machine.as_deref();
        let projects = box_projects(&self.inner.home)
            .into_iter()
            .map(|(directory, name)| {
                let mut project = json!({"directory": directory, "name": name});
                if let Some(machine) = machine {
                    project["machine"] = json!(machine);
                }
                project
            })
            .collect();
        Ok(kit.templates_answer(
            &self.inner.setup.personalities(),
            machine,
            self.inner.is_mac,
            starting,
            projects,
        ))
    }

    /// `POST kuvo/bots`.
    pub(super) async fn make(&self, given: Value) -> Made {
        self.make_with(given, None).await
    }

    /// A bot made, with what another part of this box gives it (`setup`); the same `request`
    /// gets the same answer for an hour (one with a setup, its own).
    pub async fn make_with(&self, given: Value, setup: Option<Setup>) -> Made {
        let Some(request) = request_id(&given) else {
            return self.make_once(given, setup).await;
        };
        let request = if setup.is_some() {
            format!("marketplace:{request}")
        } else {
            request
        };
        let (answer, mut waiting) = watch::channel(None);
        {
            let mut requests = self.inner.requests.lock().await;
            requests
                .answers
                .retain(|_, (_, at)| at.elapsed() <= MADE_FOR);
            if let Some((before, _)) = requests.answers.get(&request) {
                waiting = before.clone();
            } else {
                requests
                    .answers
                    .insert(request.clone(), (waiting.clone(), Instant::now()));
                // (Made whatever becomes of the request that asked.)
                let bots = self.clone();
                tokio::spawn(async move {
                    let made = bots.make_once(given, setup).await;
                    let mut requests = bots.inner.requests.lock().await;
                    if made.is_ok() {
                        if let Some(kept) = requests.answers.get_mut(&request) {
                            kept.1 = Instant::now();
                        }
                    } else {
                        requests.answers.remove(&request);
                    }
                    let _ = answer.send(Some(made));
                });
            }
        }
        match waiting.wait_for(Option::is_some).await {
            Ok(made) => made.clone().expect("answered"),
            Err(_) => Err((503, "The bot couldn’t be made.".into())),
        }
    }

    async fn make_once(&self, given: Value, setup: Option<Setup>) -> Made {
        let no = |status: u16, message: &str| Err((status, message.to_owned()));
        let Some(kit) = self.kit() else {
            return no(409, NO_KIT);
        };
        let Some(given) = given.as_object().filter(|_| !given.is_null()) else {
            return no(400, "That isn’t a bot to make.");
        };
        let name = plain_name_of(given.get("name"));
        if name.is_empty() {
            return no(400, "A bot needs a name.");
        }
        if self.inner.is_mac
            && given
                .get("machine")
                .is_some_and(|machine| !machine.is_null() && machine != "mac")
        {
            return no(
                400,
                &format!(
                    "This Mac makes only bots that live on it: make one for “{}” there.",
                    plain_name_of(given.get("machine"))
                ),
            );
        }
        let template = match given.get("template").filter(|template| !template.is_null()) {
            Some(wanted) => match kit.template(wanted) {
                Some(template) => Some(template.clone()),
                None => {
                    return no(
                        400,
                        &format!("There’s no template “{}”.", js_string(wanted)),
                    );
                }
            },
            None => None,
        };
        let shared = given
            .get("job")
            .and_then(Value::as_object)
            .filter(|job| {
                job.get("line").is_some_and(Value::is_string)
                    || job.get("details").is_some_and(Value::is_string)
            })
            .map(|job| {
                first_message_of(
                    job.get("line").and_then(Value::as_str).unwrap_or_default(),
                    job.get("details")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )
            })
            .filter(|message| !message.is_empty());
        let personality_id = given
            .get("personality")
            .filter(|personality| !personality.is_null())
            .or_else(|| template.as_ref()?.get("personality"))
            .cloned()
            .unwrap_or(Value::Null);
        let personality = kit
            .personalities(&self.inner.setup.personalities())
            .into_iter()
            .find(|each| personality_id.as_str() == Some(each.id.as_str()));
        if truthy(&personality_id) && personality.is_none() {
            return no(
                400,
                &format!("There’s no personality “{}”.", js_string(&personality_id)),
            );
        }
        let project = given.get("project").filter(|project| !project.is_null());
        let project = match project {
            Some(project) => {
                let known = project.as_str().filter(|project| {
                    box_projects(&self.inner.home)
                        .iter()
                        .any(|(directory, _)| directory == project)
                });
                match known {
                    Some(project) => Some(project.to_owned()),
                    None => return no(400, "That project isn’t in Kuvo Code."),
                }
            }
            None => None,
        };
        let look = if kit.looks.fits(given.get("look")) {
            bot_look(&given["look"])
        } else if let Some(look) = template.as_ref().and_then(|template| template.get("look")) {
            look.clone()
        } else {
            json!({
                "color": pick(&kit.looks.colors),
                "shape": pick(&kit.looks.shapes),
                "face": pick(&kit.looks.faces),
            })
        };
        let given_model = given.get("model").and_then(bot_model);
        let agents = runs_with_agents(given_model.as_ref());
        // (The engine for the model it starts on and for its conversation; the coding agents' bridge
        // isn't in this box yet.)
        let engine = if agents {
            None
        } else {
            self.inner.engine.bot_engine().await
        };
        let model = match given_model {
            Some(model) => model,
            None => match &engine {
                Some(engine) => match reach::starting_model(engine).await {
                    Some(model) => model,
                    None => {
                        return no(
                            422,
                            &format!(
                                "There’s no model to start it on here: sign in to an account on {} first.",
                                self.here()
                            ),
                        );
                    }
                },
                None => {
                    return no(
                        422,
                        &format!(
                            "There’s no model to start it on here: sign in to an account on {} first.",
                            self.here()
                        ),
                    );
                }
            },
        };
        let Some(engine) = engine else {
            return no(
                503,
                if agents {
                    AGENTS_DOWN
                } else {
                    "The engine isn’t running."
                },
            );
        };
        if let Some(refused) = reach::sign_in_refusal(&engine, &model, self.here()).await {
            return no(422, &refused);
        }

        let id = new_id();
        let folder = self.inner.bots.join(&id);
        let directory = project
            .clone()
            .unwrap_or_else(|| folder.join("workspace").display().to_string());
        let mut session: Option<String> = None;
        let made_for = setup.is_some();
        let made = self
            .make_folder(
                &engine,
                &kit,
                Making {
                    id: &id,
                    name: &name,
                    folder: &folder,
                    directory: &directory,
                    project: project.as_deref(),
                    model: &model,
                    look: &look,
                    personality: personality
                        .as_ref()
                        .map(|each| (each.id.as_str(), each.instruction.as_str())),
                    setup,
                },
                &mut session,
            )
            .await;
        if let Err(error) = made {
            if let Some(session) = &session {
                let _ = engine
                    .call(
                        Method::DELETE,
                        &format!("api/session/{}", segment(session)),
                        None,
                        BOT_STEP,
                    )
                    .await;
            }
            let _ = std::fs::remove_dir_all(&folder);
            return no(503, &format!("The bot couldn’t be made: {error}"));
        }
        let session = session.expect("made");
        // Without a template's or a shared bot's job for you to send, it writes first.
        let first = template
            .as_ref()
            .map(|template| {
                first_message_of(
                    template
                        .get("job")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    template
                        .get("instructions")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )
            })
            .filter(|message| !message.is_empty())
            .or(shared);
        if first.is_none() && !made_for {
            let _ = engine
                .call(
                    Method::POST,
                    &format!("api/session/{}/synthetic", segment(&session)),
                    Some(
                        &json!({"text": kit.greeting, "description": "A new bot's first message"}),
                    ),
                    BOT_STEP,
                )
                .await;
        }
        self.changed();
        Ok(json!({ "bot": self.listed_one(&id), "firstMessage": first }))
    }

    async fn make_folder(
        &self,
        engine: &BotEngine,
        kit: &super::kit::Kit,
        making: Making<'_>,
        session: &mut Option<String>,
    ) -> Result<(), String> {
        let Making {
            id,
            name,
            folder,
            directory,
            project,
            model,
            look,
            personality,
            setup,
        } = making;
        let fail = |error: std::io::Error| error.to_string();
        std::fs::create_dir_all(folder.join("workspace")).map_err(fail)?;
        std::fs::create_dir_all(folder.join("wakes")).map_err(fail)?;
        // (What Claude Code remembers for it, in its own folder, so it goes wherever the bot goes;
        // not for a bot in a project: there it's the project's.)
        if project.is_none() {
            std::fs::create_dir_all(folder.join("memory")).map_err(fail)?;
        }
        std::fs::write(folder.join("MEMORY.md"), kit.memory_for(name)).map_err(fail)?;
        let job = match setup {
            Some(Setup { job, prepare }) => {
                self.zone_files().write(folder).map_err(fail)?;
                prepare(folder, Path::new(directory)).map_err(fail)?;
                super::job::project_job(folder, &job).map_err(fail)?;
                Some(job)
            }
            None => None,
        };
        let own = Rule::new(
            "external_directory",
            &folder.join("*").display().to_string(),
            "allow",
        );
        let rules = vec![own];
        let made = engine
            .call(
                Method::POST,
                "api/session",
                Some(&json!({
                    "title": name,
                    "location": {"directory": directory},
                    "model": model,
                    "permissions": rules,
                    "metadata": {"kuvo": {"bot": id}},
                })),
                BOT_STEP,
            )
            .await
            .map_err(|error| error.to_string())?;
        let made = made["id"]
            .as_str()
            .filter(|made| !made.is_empty())
            .ok_or("No conversation was made.")?
            .to_owned();
        *session = Some(made.clone());
        // Marked as a bot's, and kept with the approval its rules say (this computer's default),
        // its rules made for what that comes to here.
        let approvals = &self.inner.approvals;
        let kept = approvals.of_bot(&rules, None);
        let made_rules = approvals.rules(kept.effective, &rules, &[]);
        engine
            .rewrite(&made, |_| {
                Some(NoteEdits {
                    kuvo: [
                        ("bot".to_owned(), Some(json!(id))),
                        ("approval".to_owned(), Some(json!(kept.approval.word()))),
                        ("applied".to_owned(), Some(json!(kept.effective.word()))),
                    ]
                    .into(),
                    permissions: Some(made_rules.iter().map(permission).collect()),
                    ..NoteEdits::default()
                })
            })
            .await
            .map_err(|_| "The conversation it made went.".to_owned())?;
        let told = kit.instructions_for(
            name,
            &folder.display().to_string(),
            personality.map(|(_, instruction)| instruction),
            project,
            job.as_ref()
                .map(|job| JobText {
                    line: job.line.clone(),
                    section: job.details.clone(),
                })
                .as_ref(),
        );
        if !reach::tell(engine, &made, &told).await {
            return Err("Its conversation wasn’t told what it is.".into());
        }
        std::fs::write(folder.join("session"), &made).map_err(fail)?;
        note_told(
            folder,
            &[
                ("session", &made),
                ("instructions", &digest(&told)),
                ("rules", &folder.display().to_string()),
            ],
        );
        std::fs::write(folder.join("directory"), directory).map_err(fail)?;
        // Its wake-ups go by your time zone.
        self.zone_files().write(folder).map_err(fail)?;
        let now = stamp();
        let mut described = Map::new();
        described.insert("created".into(), json!(now));
        described.insert("id".into(), json!(id));
        described.insert(
            "job".into(),
            json!(job.as_ref().map_or("", |job| job.line.as_str())),
        );
        described.insert("kuvo".into(), json!(DESCRIPTION));
        described.insert("look".into(), look.clone());
        described.insert("model".into(), model.clone());
        described.insert("name".into(), json!(name));
        if let Some((personality, _)) = personality {
            described.insert("personality".into(), json!(personality));
        }
        if let Some(project) = project {
            described.insert("project".into(), json!(project));
        }
        described.insert("updated".into(), json!(now));
        let text = serde_json::to_string_pretty(&Value::Object(described)).expect("JSON");
        std::fs::write(folder.join("bot.json"), text).map_err(fail)?;
        Ok(())
    }
}

/// What a bot is made with, once it's checked.
struct Making<'a> {
    id: &'a str,
    name: &'a str,
    folder: &'a PathBuf,
    directory: &'a str,
    project: Option<&'a str>,
    model: &'a Value,
    look: &'a Value,
    personality: Option<(&'a str, &'a str)>,
    setup: Option<Setup>,
}

/// Whether a value counts as given, as JavaScript tests it.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(text) => !text.is_empty(),
        _ => true,
    }
}

/// What a bot made for another part of this box comes with (the Marketplace's Add Agent): its job,
/// and what's put in its folder before its conversation is made (`folder`, then the folder it
/// works in).
pub struct Setup {
    pub job: super::job::Job,
    pub prepare: kuvo_services::marketplace::bots::Prepare,
}

/// Requests' answers, shared by every copy of the bots.
pub type SharedRequests = Arc<Mutex<Requests>>;
