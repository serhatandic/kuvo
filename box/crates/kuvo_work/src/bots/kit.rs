//! What a box makes its bots with and tells them: Kuvo Code's words for bots (`bin/bot-words.json`),
//! New Bot's templates, and the box's own facts (its name, your time zone, the personalities you
//! made); and whether a model runs with the coding agents.

use std::path::Path;

use serde_json::{Map, Value, json};

/// What's said while this box hasn't Kuvo Code's words for bots (an install that's missing them).
pub const NO_KIT: &str =
    "Kuvo Code on this computer is missing its words for bots: update it, then try again.";

/// The coding agents' providers: a bot on one of theirs runs with them.
const AGENT_PROVIDERS: &[&str] = &[
    "claude-code",
    "codex-cli",
    "cursor",
    "grok-cli",
    "antigravity",
];

/// Whether a model runs with the coding agents.
pub fn runs_with_agents(model: Option<&Value>) -> bool {
    let provider = model
        .and_then(|model| model.get("providerID"))
        .map(|provider| match provider {
            Value::String(text) => text.clone(),
            Value::Null => String::new(),
            other => other.to_string(),
        })
        .unwrap_or_default();
    provider == "auto" || AGENT_PROVIDERS.contains(&provider.split('@').next().unwrap_or_default())
}

/// A personality a bot made here can have: one of the kit's, or one you made (`custom`), with
/// what the bot is told.
#[derive(Clone, Debug)]
pub struct Personality {
    pub id: String,
    pub name: String,
    pub description: String,
    pub custom: bool,
    pub instruction: String,
}

/// A bot's job, as it's told: its line, and all of it.
#[derive(Clone, Debug, Default)]
pub struct JobText {
    pub line: String,
    pub section: String,
}

/// The colors, shapes and faces a bot can have, by their names.
#[derive(Clone, Debug)]
pub struct Looks {
    pub colors: Vec<String>,
    pub shapes: Vec<String>,
    pub faces: Vec<String>,
}

impl Looks {
    /// Whether `look` is one a bot can have.
    pub fn fits(&self, look: Option<&Value>) -> bool {
        let Some(look) = look.filter(|look| !is_falsy(look)) else {
            return false;
        };
        let has = |list: &[String], key: &str| {
            look.get(key)
                .and_then(Value::as_str)
                .is_some_and(|value| list.iter().any(|each| each == value))
        };
        has(&self.colors, "color") && has(&self.shapes, "shape") && has(&self.faces, "face")
    }

    fn value(&self) -> Value {
        json!({"colors": self.colors, "shapes": self.shapes, "faces": self.faces})
    }
}

fn is_falsy(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(flag) => !flag,
        Value::Number(number) => number.as_f64() == Some(0.0),
        Value::String(text) => text.is_empty(),
        _ => false,
    }
}

/// What this box makes bots with: its words, filled in for where it runs.
#[derive(Clone, Debug)]
pub struct Kit {
    /// New Bot's templates, as the words have them.
    pub templates: Vec<Map<String, Value>>,
    /// Kuvo Code's own personalities, each with what a bot is told.
    pub personalities: Vec<Personality>,
    pub personality_words: String,
    pub looks: Looks,
    /// What a bot is told, whole, with ⟦name⟧, ⟦folder⟧, ⟦personality⟧, ⟦jobLine⟧ and ⟦jobPart⟧ left.
    pub instructions: String,
    job_none: String,
    job_line: String,
    job_details: String,
    project_note: String,
    identity_note: String,
    pub memory: String,
    pub greeting: String,
}

/// Where a bot runs, as the kit fills its words in.
pub struct Place<'a> {
    pub is_mac: bool,
    /// Whether your own time zone goes in this Mac's bots' folders (one is set).
    pub zone_yours: bool,
    /// This computer as its peers know it (its link's id, or its host name).
    pub machine: &'a str,
    /// Your time zone, or this computer's own clock's.
    pub time_zone: &'a str,
    /// The system it is, as `process.platform` names it ("darwin", "linux", "win32").
    pub system: &'a str,
}

/// The words for bots in `file`, where they're whole.
fn read_words(file: &Path) -> Option<Map<String, Value>> {
    let words: Value = serde_json::from_str(&kuvo_store::files::text_of(file)?).ok()?;
    let words = words.as_object()?.clone();
    let whole = words.get("kuvo").and_then(Value::as_f64) == Some(1.0)
        && ["templates", "personalities", "instructions"]
            .iter()
            .all(|key| words.get(*key).is_some_and(Value::is_array))
        && words.get("looks").is_some_and(Value::is_object)
        && words
            .get("here")
            .and_then(|here| here.get("mac"))
            .is_some_and(Value::is_object)
        && words
            .get("here")
            .and_then(|here| here.get("machine"))
            .is_some_and(Value::is_object)
        && words.get("job").is_some_and(Value::is_object)
        && [
            "personalityWords",
            "projectNote",
            "identityNote",
            "memory",
            "greeting",
        ]
        .iter()
        .all(|key| words.get(*key).is_some_and(Value::is_string));
    whole.then_some(words)
}

fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|list| list.iter().map(text).collect())
        .unwrap_or_default()
}

/// The process platform's name for the system this box runs on.
pub fn system_here() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

/// What a bot is told, filled in for where it runs.
fn words_here(instructions: &[Value], here: &Map<String, Value>, place: &Place<'_>) -> String {
    let systems = here.get("systems").and_then(Value::as_object);
    let named = systems
        .and_then(|systems| systems.get(place.system).or_else(|| systems.get("other")))
        .map_or_else(|| "machine".to_owned(), text);
    let joined = instructions
        .iter()
        .map(text)
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut out = String::with_capacity(joined.len());
    let mut rest = joined.as_str();
    // (Each of ⟦place⟧, ⟦clock⟧ and ⟦browser⟧ in one pass: what they're filled with isn't looked at again.)
    while let Some(start) = rest.find('⟦') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let key = ["place", "clock", "browser"]
            .into_iter()
            .find(|key| after.starts_with(&format!("⟦{key}⟧")));
        match key {
            Some(key) => {
                out.push_str(&here.get(key).map(text).unwrap_or_default());
                rest = &after[format!("⟦{key}⟧").len()..];
            }
            None => {
                out.push('⟦');
                rest = &after['⟦'.len_utf8()..];
            }
        }
    }
    out.push_str(rest);
    let zone = if place.time_zone.is_empty() {
        "UTC"
    } else {
        place.time_zone
    };
    out.replace("⟦system⟧", &named)
        .replace("⟦machine⟧", place.machine)
        .replace("⟦timeZone⟧", zone)
}

impl Kit {
    /// What this box makes bots with, from the words in its `bin/`; none without them.
    pub fn read(home: &Path, place: &Place<'_>) -> Option<Self> {
        let words = read_words(&home.join("bin/bot-words.json"))?;
        let mut here = if place.is_mac {
            words["here"]["mac"].as_object()?.clone()
        } else {
            words["here"]["machine"].as_object()?.clone()
        };
        // (This Mac's bots are told they go by its clock only while they do.)
        if place.is_mac && place.zone_yours {
            here.insert("clock".into(), words["here"]["machine"]["clock"].clone());
        }
        let personality_words = text(&words["personalityWords"]);
        let personalities = words["personalities"]
            .as_array()?
            .iter()
            .map(|each| {
                let name = text(&each["name"]);
                let description = text(&each["description"]);
                Personality {
                    id: text(&each["id"]),
                    instruction: told(&personality_words, &name, &description),
                    name,
                    description,
                    custom: false,
                }
            })
            .collect();
        let looks = &words["looks"];
        Some(Self {
            templates: words["templates"]
                .as_array()?
                .iter()
                .filter_map(|each| each.as_object().cloned())
                .collect(),
            personalities,
            personality_words,
            looks: Looks {
                colors: strings(&looks["colors"]),
                shapes: strings(&looks["shapes"]),
                faces: strings(&looks["faces"]),
            },
            instructions: words_here(words["instructions"].as_array()?, &here, place),
            job_none: text(&words["job"]["none"]),
            job_line: text(&words["job"]["line"]),
            job_details: text(&words["job"]["details"]),
            project_note: text(&words["projectNote"]),
            identity_note: text(&words["identityNote"]),
            memory: text(&words["memory"]),
            greeting: text(&words["greeting"]),
        })
    }

    /// The template `id`, where the kit has one.
    pub fn template(&self, id: &Value) -> Option<&Map<String, Value>> {
        self.templates
            .iter()
            .find(|each| each.get("id") == Some(id))
    }

    /// The personalities a bot made here can have: Kuvo Code's own, then yours (`library`).
    pub fn personalities(&self, library: &[Value]) -> Vec<Personality> {
        let mut all = self.personalities.clone();
        all.extend(library.iter().map(|each| {
            let name = text(&each["name"]);
            let description = text(&each["description"]);
            Personality {
                id: text(&each["id"]),
                instruction: told(&self.personality_words, &name, &description),
                name,
                description,
                custom: true,
            }
        }));
        all
    }

    /// A bot's memory file as it's made, for its name.
    pub fn memory_for(&self, name: &str) -> String {
        self.memory.replace("⟦name⟧", name)
    }

    /// What a bot is told: the kit's words with its own name, folder, personality and project,
    /// and its job, for one that has one.
    pub fn instructions_for(
        &self,
        name: &str,
        folder: &str,
        personality: Option<&str>,
        project: Option<&str>,
        job: Option<&JobText>,
    ) -> String {
        let working = job.is_some_and(|job| !job.section.is_empty() || !job.line.is_empty());
        let mut text = self
            .instructions
            .replace("⟦name⟧", name)
            .replace("⟦folder⟧", folder);
        text = match personality.filter(|personality| !personality.is_empty()) {
            Some(personality) => text.replacen("⟦personality⟧", personality, 1),
            None => text.replacen("\n⟦personality⟧\n", "", 1),
        };
        if project.is_some_and(|project| !project.is_empty()) {
            text = text.replacen(
                &format!("\n\n{}", self.identity_note),
                &format!("\n\n{}\n\n{}", self.project_note, self.identity_note),
                1,
            );
        }
        let line = job
            .filter(|job| !job.line.is_empty())
            .map(|job| self.job_line.replacen("⟦job⟧", &job.line, 1))
            .unwrap_or_default();
        text = text.replacen("⟦jobLine⟧", &line, 1);
        let part = match job.filter(|_| working) {
            Some(job) => self.job_details.replacen(
                "⟦jobDetails⟧",
                if job.section.is_empty() {
                    &job.line
                } else {
                    &job.section
                },
                1,
            ),
            None => self.job_none.clone(),
        };
        if !part.is_empty() {
            text.replacen("⟦jobPart⟧", &part, 1)
        } else if let Some(at) = text.find("\n\n⟦jobPart⟧") {
            format!("{}{}", &text[..at], &text[at + "\n\n⟦jobPart⟧".len()..])
        } else {
            text.replacen("⟦jobPart⟧", "", 1)
        }
    }

    /// `kuvo/bots/templates`: what New Bot offers, the model a bot starts on here (by `mac`, or
    /// this machine's name), and its projects.
    pub fn templates_answer(
        &self,
        library: &[Value],
        machine: Option<&str>,
        is_mac: bool,
        starting: Option<Value>,
        projects: Vec<Value>,
    ) -> Value {
        let templates: Vec<Value> = self
            .templates
            .iter()
            .map(|template| {
                let mut template = template.clone();
                template.remove("firstMessage");
                Value::Object(template)
            })
            .collect();
        let personalities: Vec<Value> = self
            .personalities(library)
            .into_iter()
            .map(|each| {
                json!({"id": each.id, "name": each.name, "description": each.description, "custom": each.custom})
            })
            .collect();
        let place = if is_mac {
            "mac".to_owned()
        } else {
            machine
                .filter(|name| !name.is_empty())
                .unwrap_or("machine")
                .to_owned()
        };
        json!({
            "templates": templates,
            "personalities": personalities,
            "looks": self.looks.value(),
            "startingModel": { place: starting },
            "projects": projects,
        })
    }
}

fn told(words: &str, name: &str, description: &str) -> String {
    words
        .replace("⟦name⟧", name)
        .replace("⟦description⟧", description)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kit(is_mac: bool, zone_yours: bool) -> Kit {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("bin")).unwrap();
        // Built in, so the test runs on a computer without this checkout.
        std::fs::write(
            home.path().join("bin/bot-words.json"),
            include_str!("../../../../src/shared/bot-words.json"),
        )
        .unwrap();
        Kit::read(
            home.path(),
            &Place {
                is_mac,
                zone_yours,
                machine: "lab",
                time_zone: "Asia/Tokyo",
                system: "linux",
            },
        )
        .expect("the words")
    }

    #[test]
    fn incomplete_bot_words_without_here_are_not_loaded() {
        let home = tempfile::tempdir().unwrap();
        let folder = home.path().join("bin");
        std::fs::create_dir_all(&folder).unwrap();
        let mut words: Value =
            serde_json::from_str(include_str!("../../../../src/shared/bot-words.json")).unwrap();
        words.as_object_mut().unwrap().remove("here");
        std::fs::write(
            folder.join("bot-words.json"),
            serde_json::to_vec(&words).unwrap(),
        )
        .unwrap();
        assert!(
            Kit::read(
                home.path(),
                &Place {
                    is_mac: false,
                    zone_yours: false,
                    machine: "lab",
                    time_zone: "Asia/Tokyo",
                    system: "linux",
                }
            )
            .is_none(),
            "incomplete words must be refused, not panic"
        );
    }

    #[test]
    fn a_machines_bot_is_told_where_it_runs_and_its_clock() {
        let told = kit(false, false).instructions_for("Ada", "/bots/bot_a", None, None, None);
        assert!(told.contains("You are Ada, one of the user's bots in Kuvo Code"));
        assert!(told.contains("the Linux machine \"lab\""), "{told}");
        assert!(told.contains("in the user's time zone, Asia/Tokyo"));
        assert!(told.contains("You have a web browser: one on this machine"));
        assert!(!told.contains('⟦'), "{told}");
        assert!(told.contains("You don't have a job yet"));
    }

    #[test]
    fn a_macs_bot_goes_by_its_clock_until_you_set_a_zone() {
        let told = kit(true, false).instructions_for("Ada", "/bots/bot_a", None, None, None);
        assert!(told.contains("You run on this Mac."), "{told}");
        assert!(told.contains("on this Mac's clock"));
        let told = kit(true, true).instructions_for("Ada", "/bots/bot_a", None, None, None);
        assert!(told.contains("in the user's time zone, Asia/Tokyo"));
    }

    #[test]
    fn its_personality_project_and_job_are_told() {
        let kit = kit(false, false);
        let straight = kit
            .personalities(&[])
            .into_iter()
            .find(|each| each.id == "preset.straight")
            .unwrap();
        let job = JobText {
            line: "Watches the build".into(),
            section: String::new(),
        };
        let told = kit.instructions_for(
            "Ada",
            "/bots/bot_a",
            Some(&straight.instruction),
            Some("/work/app"),
            Some(&job),
        );
        assert!(told.contains("Your personality: Straight Shooter."));
        assert!(told.contains(": Watches the build"));
        assert!(told.contains("Your job:\nWatches the build"));
        assert!(!told.contains("You don't have a job yet"));
        assert!(!told.contains('⟦'), "{told}");
    }

    #[test]
    fn models_on_the_coding_agents() {
        assert!(runs_with_agents(Some(
            &json!({"providerID": "claude-code@2"})
        )));
        assert!(runs_with_agents(Some(&json!({"providerID": "auto"}))));
        assert!(!runs_with_agents(Some(&json!({"providerID": "opencode"}))));
        assert!(!runs_with_agents(None));
    }
}
