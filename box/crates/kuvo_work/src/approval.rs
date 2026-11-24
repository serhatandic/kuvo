//! How much a conversation's agent does on its own (PROTOCOL.md, "Approval"): Ask First, Accept
//! Edits, Auto and Full Access, kept with the conversation, and the engine's rules made for each
//! with this computer's Settings › Permissions, so a Don't Allow there holds in every one.
//!
//! Auto needs the sandbox; this box runs none yet, so a conversation set to Auto asks first, and
//! says why. Full Access is had once you confirmed it here (`box/approval.json`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// How much a conversation's agent does on its own, least first.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Approval {
    AskFirst,
    AcceptEdits,
    Auto,
    FullAccess,
}

impl Approval {
    pub fn of(value: &Value) -> Option<Self> {
        Self::deserialize(value).ok()
    }

    pub fn word(self) -> &'static str {
        match self {
            Self::AskFirst => "askFirst",
            Self::AcceptEdits => "acceptEdits",
            Self::Auto => "auto",
            Self::FullAccess => "fullAccess",
        }
    }

    /// Whether rules made for it ask before commands (Ask First and Accept Edits do).
    pub fn asks_before_commands(self) -> bool {
        matches!(self, Self::AskFirst | Self::AcceptEdits)
    }
}

/// One of the engine's rules: what it's for, where, and what it does (`allow`, `ask`, `deny`, or
/// one a newer engine has, held back as a deny is, and kept as it is).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Rule {
    pub action: String,
    pub resource: String,
    pub effect: String,
}

impl Rule {
    pub fn new(action: &str, resource: &str, effect: &str) -> Self {
        Self {
            action: action.into(),
            resource: resource.into(),
            effect: effect.into(),
        }
    }

    /// A rule as the engine has it, or none for what isn't one.
    pub fn of(value: &Value) -> Option<Self> {
        let rule = Self::deserialize(value).ok()?;
        (!rule.effect.is_empty()).then_some(rule)
    }

    /// Every one of `values` a rule, or none.
    pub fn all(values: &[Value]) -> Option<Vec<Self>> {
        values.iter().map(Self::of).collect()
    }

    /// What it holds back: Don't Allow, or one this box doesn't know.
    pub fn holds_back(&self) -> bool {
        self.effect != "allow" && self.effect != "ask"
    }

    /// A folder a conversation may work in, of its own.
    pub fn is_particular(&self) -> bool {
        self.action == "external_directory" && self.effect == "allow" && self.resource != "*"
    }

    fn is_outside(&self) -> bool {
        self.action == "external_directory" && self.resource == "*"
    }

    fn with_effect(&self, effect: &str) -> Self {
        Self::new(&self.action, &self.resource, effect)
    }
}

/// What Ask First asks about.
const ASK_FIRST: &[&str] = &[
    "edit",
    "write",
    "patch",
    "shell",
    "webfetch",
    "external_directory",
    "kuvo-browser_browser_save_pdf",
    "kuvo-browser_browser_upload",
    "kuvo-browser_watch_for_change",
];
const EDITS: &[&str] = &["edit", "write", "patch"];

/// What Auto and Full Access never hold the agent for: reading a `.env` and carrying on after the
/// same call again and again.
fn never_held() -> [Rule; 3] {
    [
        Rule::new("read", "*.env", "allow"),
        Rule::new("read", "*.env.*", "allow"),
        Rule::new("doom_loop", "*", "allow"),
    ]
}

fn asks_for(rules: &[Rule], action: &str) -> bool {
    rules
        .iter()
        .any(|rule| rule.action == action && rule.resource == "*" && rule.effect == "ask")
}

/// A conversation's rules made again from its approval (it asks first where it asks before
/// commands) and Settings › Permissions (`settings`) as they are now, with its own folders after.
pub fn refreshed_rules(current: &[Rule], settings: &[Rule]) -> Vec<Rule> {
    let denied = settings.iter().filter(|rule| rule.holds_back()).cloned();
    let mut made: Vec<Rule> = if asks_for(current, "shell") {
        let everywhere = settings
            .iter()
            .rev()
            .find(|rule| rule.is_outside())
            .is_some_and(|rule| rule.effect == "allow");
        ASK_FIRST
            .iter()
            .filter(|action| everywhere || **action != "external_directory")
            .map(|action| Rule::new(action, "*", "ask"))
            .chain(
                settings
                    .iter()
                    .filter(|rule| {
                        !rule.holds_back()
                            && rule.resource != "*"
                            && rule.action != "external_directory"
                    })
                    .cloned(),
            )
            .chain(denied)
            .collect()
    } else {
        settings
            .iter()
            .filter(|rule| rule.effect == "ask")
            .map(|rule| rule.with_effect("allow"))
            .chain(denied)
            .collect()
    };
    // (Its own folders are those Settings says nothing of.)
    made.extend(current.iter().filter(|rule| {
        rule.is_particular()
            && !settings
                .iter()
                .any(|set| set.action == "external_directory" && set.resource == rule.resource)
    }).cloned());
    made
}

/// Why one can't be had here now, as a word a client maps and in words.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Unavailable {
    pub reason: &'static str,
    pub why: &'static str,
}

const SANDBOX_OFF: &str = "Auto needs the sandbox, which isn't running on this computer.";
const WINDOWS: &str = "Coming soon on Windows.";
const FULL_ACCESS_OFF: &str = "Full Access isn't confirmed on this computer yet: confirm it first.";

/// A conversation's approval as it's kept: the one kept with it (`stored`), what its rules are
/// made for (`applied`), and what it comes to here now (`effective`, `held` why that's less).
#[derive(Clone, Debug)]
pub struct Kept {
    pub approval: Approval,
    pub stored: bool,
    pub applied: Option<Approval>,
    pub effective: Approval,
    pub held: Option<Unavailable>,
}

/// This computer's approvals: what you confirmed and picked last (`box/approval.json`), and its
/// Settings › Permissions.
#[derive(Clone)]
pub struct Approvals {
    file: PathBuf,
    settings: PathBuf,
    /// On a machine, its settings are its own only once their file is there.
    owns_settings_by_file: bool,
    windows: bool,
}

impl Approvals {
    pub fn new(home: &Path, is_mac: bool) -> Self {
        Self {
            file: home.join("box/approval.json"),
            settings: if is_mac {
                home.join("config/opencode/opencode.json")
            } else {
                home.join("box/opencode.json")
            },
            owns_settings_by_file: !is_mac,
            windows: kuvo_os::platform::Platform::current().is_windows_host(),
        }
    }

    fn kept(&self) -> Map<String, Value> {
        kuvo_store::files::text_of(&self.file)
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| match value {
                Value::Object(kept) => Some(kept),
                _ => None,
            })
            .unwrap_or_default()
    }

    fn keep(&self, change: Map<String, Value>) {
        let mut kept = self.kept();
        kept.extend(change);
        let text = serde_json::to_string_pretty(&Value::Object(kept)).expect("JSON");
        let _ = kuvo_store::files::write_secret(&self.file, text.as_bytes());
    }

    /// Whether you confirmed Full Access on this computer.
    pub fn full_access_allowed(&self) -> bool {
        self.kept().get("fullAccess") == Some(&Value::Bool(true))
    }

    /// The approval a new conversation here gets where nothing says: the last you picked here,
    /// Auto the first time; on Windows, Ask First in place of Auto.
    pub fn default_approval(&self) -> Approval {
        let kept = self.kept();
        let wanted = kept
            .get("last")
            .and_then(Approval::of)
            .filter(|last| {
                *last != Approval::FullAccess || kept.get("fullAccess") == Some(&Value::Bool(true))
            })
            .unwrap_or(Approval::Auto);
        if wanted == Approval::Auto && self.windows {
            Approval::AskFirst
        } else {
            wanted
        }
    }

    /// When this box first kept approvals for new conversations (kept the first time it's asked).
    pub fn since(&self) -> f64 {
        if let Some(since) = self.kept().get("since").and_then(Value::as_f64) {
            return since;
        }
        let now = jiff::Timestamp::now().as_millisecond();
        self.keep(Map::from_iter([("since".into(), json!(now))]));
        now as f64
    }

    /// Whether `approval` can be had here now.
    pub fn unavailable(&self, approval: Approval) -> Option<Unavailable> {
        match approval {
            Approval::FullAccess if !self.full_access_allowed() => Some(Unavailable {
                reason: "fullAccessOff",
                why: FULL_ACCESS_OFF,
            }),
            Approval::Auto if self.windows => Some(Unavailable {
                reason: "windows",
                why: WINDOWS,
            }),
            Approval::Auto => Some(Unavailable {
                reason: "sandbox",
                why: SANDBOX_OFF,
            }),
            _ => None,
        }
    }

    /// What `wanted` comes to here now, and why it's less.
    pub fn effective(&self, wanted: Approval) -> (Approval, Option<Unavailable>) {
        let mut approval = wanted;
        let mut held = None;
        if approval == Approval::FullAccess
            && let Some(not) = self.unavailable(approval)
        {
            held = Some(not);
            approval = Approval::Auto;
        }
        if approval == Approval::Auto
            && let Some(not) = self.unavailable(approval)
        {
            held = Some(not);
            approval = Approval::AskFirst;
        }
        (approval, held)
    }

    /// This box's Settings › Permissions, or none where they can't be read or aren't its own yet.
    pub fn permissions_here(&self) -> Option<Vec<Rule>> {
        if self.owns_settings_by_file && !self.settings.exists() {
            return None;
        }
        let settings = match std::fs::read_to_string(&self.settings) {
            Ok(text) if text.trim().is_empty() => json!({}),
            Ok(text) => serde_json::from_str::<Value>(&text)
                .ok()
                .filter(Value::is_object)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if std::fs::symlink_metadata(&self.settings).is_ok() {
                    return None;
                }
                json!({})
            }
            Err(_) => return None,
        };
        let rules = settings["permissions"].as_array().cloned().unwrap_or_default();
        Some(Rule::all(&rules).unwrap_or_default())
    }

    /// The rules a conversation runs with for `approval`: made with Settings › Permissions as they
    /// are (or, where they aren't its own, the Don't Allows it has), its own folders kept, and the
    /// Don't Allows it brought from its lead's computer (`held`) last.
    pub fn rules(&self, approval: Approval, current: &[Rule], held: &[Rule]) -> Vec<Rule> {
        let made = self.made(approval, current);
        let more: Vec<Rule> = held
            .iter()
            .filter(|rule| rule.holds_back() && !made.contains(rule))
            .cloned()
            .collect();
        made.into_iter().chain(more).collect()
    }

    fn made(&self, approval: Approval, current: &[Rule]) -> Vec<Rule> {
        let settings = self
            .permissions_here()
            .unwrap_or_else(|| current.iter().filter(|rule| rule.holds_back()).cloned().collect());
        let start: Vec<Rule> = approval
            .asks_before_commands()
            .then(|| Rule::new("shell", "*", "ask"))
            .into_iter()
            .chain(current.iter().filter(|rule| rule.is_particular()).cloned())
            .collect();
        let mut made = refreshed_rules(&start, &settings);
        if approval == Approval::AcceptEdits {
            for rule in &mut made {
                if EDITS.contains(&rule.action.as_str()) && rule.resource == "*" && rule.effect == "ask"
                {
                    rule.effect = "allow".into();
                }
            }
        }
        if approval == Approval::Auto
            && settings.iter().rev().find(|rule| rule.is_outside()).is_some_and(|rule| rule.effect == "ask")
        {
            for rule in &mut made {
                if rule.is_outside() && rule.effect == "allow" {
                    rule.effect = "ask".into();
                }
            }
        }
        if approval == Approval::FullAccess {
            made.insert(0, Rule::new("external_directory", "*", "allow"));
        }
        if matches!(approval, Approval::Auto | Approval::FullAccess) {
            made.splice(0..0, never_held());
        }
        made
    }

    /// A bot's conversation's approval (`approvalOf` for a bot): the one kept with it while its
    /// rules are still those made for it; else one asked for alone; else Ask First where its rules
    /// ask before commands, and this computer's default otherwise.
    pub fn of_bot(&self, rules: &[Rule], metadata: Option<&Value>) -> Kept {
        let own = metadata.and_then(|metadata| metadata.get("kuvo"));
        let kept = own.and_then(|own| own.get("approval"));
        let applied = own.and_then(|own| own.get("applied"));
        // (As TypeScript reads it on every call: kept the first time.)
        let _ = self.since();
        if let (Some(kept), Some(applied)) = (kept.and_then(Approval::of), applied.and_then(Approval::of))
            && made_for(rules, applied)
        {
            let (effective, held) = self.effective(kept);
            return Kept {
                approval: kept,
                stored: true,
                applied: Some(applied),
                effective,
                held,
            };
        }
        let asked = kept.and_then(Approval::of).filter(|_| applied.is_none());
        let guessed = asked.unwrap_or_else(|| {
            if asks_for(rules, "shell") {
                Approval::AskFirst
            } else {
                self.default_approval()
            }
        });
        let (effective, held) = self.effective(guessed);
        Kept {
            approval: guessed,
            stored: false,
            applied: None,
            effective,
            held,
        }
    }
}

/// Whether rules are still the ones the box made for `applied`.
fn made_for(rules: &[Rule], applied: Approval) -> bool {
    applied.asks_before_commands() == asks_for(rules, "shell")
        && !(applied == Approval::AcceptEdits && asks_for(rules, "edit"))
}

mod former;
pub use former::{has_former_asks, without_former_asks};

#[cfg(test)]
mod tests;
