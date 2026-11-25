//! A bot's conversation told what it is: the kit's words, with its job as its memory has it now;
//! and its own rules put right for its folder here and this computer's Settings › Permissions.
//! What it was last told is kept in its folder (`told.json`), so a restart doesn't tell every bot
//! again: it's told again only when what it would be told changes.

use std::path::Path;

use kuvo_engine::{BotEngine, NoteEdits, SessionPermission};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::{
    Bots,
    here::BotHere,
    job::job_of,
    kit::{JobText, Kit},
    own_keys, reach,
};
use crate::approval::{Rule, refreshed_rules, without_former_asks};

const TOLD_FILE: &str = "told.json";

/// What's kept of what a bot was told: a digest.
pub fn digest(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn told_of(folder: &Path) -> Map<String, Value> {
    kuvo_store::files::text_of(&folder.join(TOLD_FILE))
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

/// What it was told, noted with what it was told before.
pub fn note_told(folder: &Path, changes: &[(&str, &str)]) {
    let mut told = told_of(folder);
    for (key, value) in changes {
        told.insert((*key).into(), json!(value));
    }
    let file = folder.join(TOLD_FILE);
    let new = folder.join(format!("{TOLD_FILE}.new"));
    let text = serde_json::to_string(&Value::Object(told)).expect("JSON");
    if std::fs::write(&new, text).is_ok() {
        let _ = std::fs::rename(&new, &file);
    }
}

/// What the engine keeps of a rule.
pub fn permission(rule: &Rule) -> SessionPermission {
    SessionPermission::new(&rule.action, &rule.resource, &rule.effect)
}

impl Bots {
    /// What a bot here is told now: the kit's words with its name, folder, personality and
    /// project, and its job as its memory has it (`described`, its description as it's changed).
    pub(super) fn told_now(
        &self,
        kit: &Kit,
        here: &BotHere,
        described: &Map<String, Value>,
    ) -> String {
        let library = self.inner.setup.personalities();
        let personality = described
            .get("personality")
            .and_then(Value::as_str)
            .and_then(|id| {
                kit.personalities(&library)
                    .into_iter()
                    .find(|each| each.id == id)
            })
            .map(|each| each.instruction);
        let job = job_of(here, true);
        let name = described
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let project = described.get("project").and_then(Value::as_str);
        kit.instructions_for(
            name,
            &here.folder.display().to_string(),
            personality.as_deref(),
            project,
            Some(&JobText {
                line: job.line,
                section: job.details,
            }),
        ) + &own_keys::keys_gone_note(&here.folder, &self.config(), &self.inner.computer)
    }

    /// Tells a bot's conversation `text`, and notes it. Whether it got there.
    pub(super) async fn tell_bot(
        engine: &BotEngine,
        here: &BotHere,
        session: &str,
        text: &str,
    ) -> bool {
        let told = reach::tell(engine, session, text).await;
        if told {
            note_told(
                &here.folder,
                &[("session", session), ("instructions", &digest(text))],
            );
        }
        told
    }

    /// What a bot's folder says of where it is, as this box has it: the folder it works in, the
    /// time zone its wake-ups go by, and its memory file (for one made before it was made with it).
    pub(super) fn settle_folder(&self, here: &BotHere, kit: Option<&Kit>) {
        let write = |name: &str, data: &[u8]| {
            let file = here.folder.join(name);
            if std::fs::read(&file).ok().as_deref() != Some(data) {
                std::fs::write(&file, data)?;
            }
            Ok::<_, std::io::Error>(())
        };
        let directory = here
            .described
            .get("project")
            .and_then(Value::as_str)
            .map_or_else(
                || here.folder.join("workspace").display().to_string(),
                str::to_owned,
            );
        let _ = (|| {
            write("directory", directory.as_bytes())?;
            self.zone_files().write(&here.folder)?;
            if let Some(kit) = kit
                && !here.folder.join("MEMORY.md").exists()
            {
                write("MEMORY.md", kit.memory_for(here.name()).as_bytes())?;
            }
            Ok::<_, std::io::Error>(())
        })();
    }

    /// A bot's own rules as they're made now: its approval with this computer's Settings ›
    /// Permissions, then its folder here. Whether they're right now.
    pub(super) async fn repair_rules(
        &self,
        engine: &BotEngine,
        here: &BotHere,
        session: &str,
    ) -> bool {
        let id = here.id.clone();
        let own = Rule::new(
            "external_directory",
            &here.folder.join("*").display().to_string(),
            "allow",
        );
        let wake = self.inner.home.join("bin/kuvo-wake").display().to_string();
        let commands = [
            format!("kuvo-wake {id} "),
            format!("\"{wake}\" {id} "),
            format!("\"$HOME/.kuvo-code/bin/kuvo-wake\" {id} "),
        ];
        let old = |rule: &Value| {
            let resource = match rule.get("resource") {
                Some(Value::String(text)) => text.clone(),
                Some(Value::Null) | None => String::new(),
                Some(other) => other.to_string(),
            };
            (rule.get("action").and_then(Value::as_str) == Some("shell")
                && commands.iter().any(|command| resource.starts_with(command)))
                || (rule.get("action").and_then(Value::as_str) == Some("external_directory")
                    && resource.ends_with(&format!("/bots/{id}/*")))
        };
        let approvals = &self.inner.approvals;
        let mut readable = true;
        let done = engine
            .rewrite(session, |data| {
                let Some(rules) = data.get("permissions").and_then(Value::as_array) else {
                    readable = false;
                    return None;
                };
                let kept: Vec<Value> = rules.iter().filter(|rule| !old(rule)).cloned().collect();
                let typed = Rule::all(&kept);
                let settings = typed.as_ref().and(approvals.permissions_here());
                let made = match (&typed, &settings) {
                    (Some(kept), Some(settings)) => {
                        let approval =
                            approvals.of_bot(kept, data.get("metadata").filter(|m| m.is_object()));
                        let made = if approval.stored {
                            approvals.rules(approval.effective, kept, &[])
                        } else {
                            refreshed_rules(kept, settings)
                        };
                        without_former_asks(&made, settings)
                            .iter()
                            .map(|rule| json!(rule))
                            .collect::<Vec<_>>()
                    }
                    // (Its rules as they are, where they aren't all ones this box reads or its
                    // settings aren't its own: without the asks a box before put in a bot's.)
                    _ => kept
                        .into_iter()
                        .filter(|rule| {
                            Rule::of(rule)
                                .is_none_or(|rule| !without_former_asks(&[rule], &[]).is_empty())
                        })
                        .collect(),
                };
                let mut repaired = made;
                repaired.push(json!(own));
                if &repaired == rules {
                    return None;
                }
                let permissions = repaired
                    .iter()
                    .filter_map(Rule::of)
                    .map(|rule| permission(&rule))
                    .collect();
                Some(NoteEdits {
                    permissions: Some(permissions),
                    ..NoteEdits::default()
                })
            })
            .await
            .is_ok();
        done && readable
    }

    /// A bot's run on the engine: what it's told brought up to date, its rules put right for its
    /// folder, and its memory file made. One bot at a time.
    pub(super) async fn freshen(&self, engine: &BotEngine, here: &BotHere) {
        let _one = self.one_at_a_time(&here.session).await;
        let Some(here) = BotHere::read(&self.inner.bots, &here.id).filter(|here| !here.moving())
        else {
            return;
        };
        // (Its approval kept first, for one from before there were approvals.)
        self.inner
            .conversations
            .settle_session(engine, &here.session)
            .await;
        let kit = self.kit();
        self.settle_folder(&here, kit.as_ref());
        let Some(kit) = kit else {
            return;
        };
        let told = told_of(&here.folder);
        own_keys::forget_connected_keys(&here.folder, &self.config());
        let text = self.told_now(&kit, &here, &here.described);
        let stale = match reach::told_there(engine, &here.session).await {
            None => {
                told.get("session").and_then(Value::as_str) != Some(&here.session)
                    || told.get("instructions").and_then(Value::as_str) != Some(&digest(&text))
            }
            Some(there) => there.as_deref() != Some(text.as_str()),
        };
        if stale {
            Self::tell_bot(engine, &here, &here.session, &text).await;
        }
        let folder = here.folder.display().to_string();
        if told.get("rules").and_then(Value::as_str) != Some(&folder)
            && self.repair_rules(engine, &here, &here.session).await
        {
            note_told(&here.folder, &[("rules", &folder)]);
        }
    }

    /// Every bot whose conversation is on the engine, told what it is now where that changed, and
    /// its rules put right: as the box finds its engine, and as its app asks after a change to its
    /// words. Those with the coding agents are left to their bridge.
    pub async fn freshen_all(&self) {
        let ids = crate::wakes::folder::names(&self.inner.bots);
        let mut engine = None;
        for id in ids {
            let Some(here) = BotHere::read(&self.inner.bots, &id).filter(|here| !here.moving())
            else {
                continue;
            };
            if here.agents {
                continue;
            }
            if engine.is_none() {
                engine = self.inner.engine.running_bot_engine().await;
            }
            let Some(engine) = engine.as_ref() else {
                return;
            };
            if reach::told_there(engine, &here.session).await.is_none()
                && !engine.has_session(&here.session).await
            {
                continue;
            }
            self.freshen(engine, &here).await;
        }
    }

    /// `POST kuvo/bots/<id>/arrived`: a bot just brought here from another computer, made this
    /// box's: its folder, its rules here, and what it's told here.
    pub(super) async fn settle(&self, id: &str) -> Result<Value, (u16, String)> {
        let gone = || (404, "That bot isn’t there any more.".to_owned());
        let here = BotHere::read(&self.inner.bots, id).ok_or_else(gone)?;
        let _one = self.one_at_a_time(&here.session).await;
        let kit = self.kit();
        self.settle_folder(&here, kit.as_ref());
        self.changed();
        let kit = kit.ok_or_else(|| (409, super::kit::NO_KIT.to_owned()))?;
        if here.agents {
            return Err((503, super::AGENTS_DOWN.to_owned()));
        }
        let engine = self
            .inner
            .engine
            .bot_engine()
            .await
            .ok_or_else(|| (503, "The engine isn’t running.".to_owned()))?;
        let text = self.told_now(&kit, &here, &here.described);
        let there = reach::told_there(&engine, &here.session).await;
        let already = there
            .as_ref()
            .is_some_and(|there| there.as_deref() == Some(text.as_str()));
        if already {
            note_told(
                &here.folder,
                &[("session", &here.session), ("instructions", &digest(&text))],
            );
        }
        let told = already || Self::tell_bot(&engine, &here, &here.session, &text).await;
        let ruled = self.repair_rules(&engine, &here, &here.session).await;
        if ruled {
            note_told(
                &here.folder,
                &[("rules", &here.folder.display().to_string())],
            );
        }
        if !told || !ruled {
            return Err((
                503,
                "Its conversation here didn’t take what it is now. It’s tried again as its next run starts."
                    .into(),
            ));
        }
        Ok(json!({ "bot": self.listed_one(id) }))
    }
}
