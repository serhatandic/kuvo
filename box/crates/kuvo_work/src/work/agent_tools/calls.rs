//! The caller's side of work on your other computers (PROTOCOL.md, "Peers", `call`): which
//! computer a tool names, and the call made there for the conversation calling, as this computer
//! knows it. The calls themselves (kept for a computer that's away, answered once) are the peers'.

use std::collections::{BTreeMap, HashSet};

use kuvo_peers::{
    calls::{Request, Tool, known_home, plain},
    directory::{ComputerKey, Member, RecordScope},
    remote_work::Outcome,
    work_rows::{Row, for_lead},
};
use kuvo_wire::keys::encode;
use serde_json::{Map, Value, json};

use super::{Caller, Said, refused, said};
use crate::work::Work;

/// Why a computer isn't answering, when the relay says it isn't there.
pub const AWAY_WORDS: &str = "it may be asleep, off or offline";
const CALLS: &str = "work.elsewhere";
const STARTS: &str = "work.elsewhere.start";
/// How long a computer a lead started work on is asked for its rows, with none told yet.
const STARTED_FOR: i64 = 30 * 86_400_000;

/// A computer's name, as its owner's words have it.
pub fn name_of(member: &Member) -> String {
    let name = plain(&member.name, 100);
    if name.is_empty() {
        "another computer".into()
    } else {
        name
    }
}

fn folded(name: &str) -> String {
    name.trim().trim_start_matches('&').to_lowercase()
}

/// Whether a call that can't be answered now is kept for when its computer is back: not, or kept
/// (telling the caller if it fails there, unless `Quietly`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    No,
    Yes,
    Quietly,
}

/// How a call went, as the caller's side tells it.
pub enum Went {
    Answer(Said),
    Failed(String),
    Away {
        why: &'static str,
        kept: bool,
        sent: bool,
    },
    Offline {
        kept: bool,
    },
    Older,
}

fn verb_of(tool: &str) -> &'static str {
    match tool {
        "stop_session" | "stop_work" => "stop",
        "compact_session" => "summary",
        "answer_ask" => "answer",
        "start_session" => "start",
        _ => "list",
    }
}

pub fn tool_of(name: &str) -> Option<Tool> {
    Some(match name {
        "list_sessions" => Tool::ListSessions,
        "stop_session" => Tool::StopSession,
        "compact_session" => Tool::CompactSession,
        "answer_ask" => Tool::AnswerAsk,
        "start_session" => Tool::StartSession,
        "list_models" => Tool::ListModels,
        "list_projects" => Tool::ListProjects,
        "stop_work" => Tool::StopWork,
        _ => return None,
    })
}

impl Work {
    fn members(&self) -> Vec<Member> {
        self.peers.view().roster.members.clone()
    }

    /// Your other computers, in words.
    fn computers_words(&self) -> String {
        let names: Vec<String> = self.members().iter().map(name_of).collect();
        if names.is_empty() {
            "You have no other computers set up with Kuvo Code.".into()
        } else {
            format!("Your other computers: {}.", names.join(", "))
        }
    }

    /// The computer `given` names (`computer`): here (none, "here", this one's own name), one of
    /// your other computers by its name (a Mac also as "the Mac" where there's one), or why not.
    pub fn computer_of(&self, given: Option<&Value>) -> Result<Option<Member>, String> {
        let wanted = match given {
            None | Some(Value::Null) => return Ok(None),
            Some(Value::String(text)) if text.trim().is_empty() => return Ok(None),
            Some(Value::String(text)) => text.trim(),
            Some(_) => return Err("computer is a computer's name, as list_agents shows it.".into()),
        };
        let folded_wanted = folded(wanted);
        let members = self.members();
        let macs = members
            .iter()
            .filter(|member| member.kind == kuvo_peers::session::ComputerKind::Mac)
            .count();
        let names = |member: &Member| {
            let mut names = vec![
                encode(&member.key),
                plain(&member.name, 100),
                name_of(member),
            ];
            if member.kind == kuvo_peers::session::ComputerKind::Mac && macs == 1 {
                names.extend(["the mac".into(), "mac".into()]);
            }
            names.iter().map(|name| folded(name)).collect::<Vec<_>>()
        };
        let matches: Vec<&Member> = members
            .iter()
            .filter(|member| names(member).contains(&folded_wanted))
            .collect();
        match matches.as_slice() {
            [one] => return Ok(Some((*one).clone())),
            [_, _, ..] => {
                return Err(format!(
                    "More than one of your computers is called {wanted}. Ask the user which one they mean."
                ));
            }
            [] => {}
        }
        let mut own = vec![
            self.peers.view().roster.name.clone(),
            "here".into(),
            "this computer".into(),
            self.settings.here().into(),
        ];
        if self.settings.is_mac() {
            own.extend(["the mac".into(), "this mac".into(), "mac".into()]);
        } else {
            own.push("this machine".into());
        }
        if own
            .iter()
            .any(|name| !name.trim().is_empty() && folded(name) == folded_wanted)
        {
            return Ok(None);
        }
        Err(format!(
            "There's no computer called {wanted}. {}",
            self.computers_words()
        ))
    }

    /// `tool` for `caller` on `peer`, its answer waited for (less once the relay says that
    /// computer isn't there). With `keep`, one that can't be answered now is kept for when it's
    /// back.
    pub async fn call_peer(
        &self,
        peer: &Member,
        tool: &str,
        args: Map<String, Value>,
        caller: &Caller,
        keep: Keep,
        start: Option<Value>,
    ) -> Went {
        let (Some(calls), Some(tool_is)) = (self.calls.get(), tool_of(tool)) else {
            return Went::Older;
        };
        let mut lead = caller.lead(self);
        lead.start = start;
        let request = Request {
            tool: tool_is,
            args,
            lead,
        };
        let outcome = calls
            .call(
                peer.key,
                caller.session.clone(),
                request,
                keep != Keep::No,
                keep == Keep::Quietly,
            )
            .await;
        match outcome {
            Ok(Outcome::Answer(answer)) => Went::Answer(Said {
                text: answer.text,
                refused: answer.refused,
            }),
            Ok(Outcome::Failed(why)) => Went::Failed(why),
            Ok(Outcome::Older) => Went::Older,
            Ok(Outcome::Offline { kept }) => Went::Offline { kept },
            Ok(Outcome::Away { kept, sent }) => {
                let away = self.peers.view().away(&peer.key);
                let why = if away {
                    AWAY_WORDS
                } else {
                    "it didn't answer in time"
                };
                Went::Away { why, kept, sent }
            }
            Err(_) => Went::Away {
                why: AWAY_WORDS,
                kept: false,
                sent: false,
            },
        }
    }

    /// The computer a session not here lives on, as this one knows it: where its rows came from
    /// or where it went, else a computer that lists it.
    fn box_of(&self, session: &str) -> Option<Member> {
        let view = self.peers.view();
        let own = view.roster.here;
        let known = known_home(&self.home.join("run"), session).filter(|key| Some(*key) != own);
        let key = known.or_else(|| {
            view.announcing(RecordScope::Session, session)
                .first()
                .copied()
        })?;
        view.roster.member(&key).cloned()
    }

    /// list_models or list_projects on another of your computers.
    pub async fn read_elsewhere(&self, tool: &str, peer: Member, caller: &Caller) -> Said {
        let name = name_of(&peer);
        let older = format!(
            "{name} runs an older Kuvo Code that can't take work from other computers. Updating Kuvo Code on {name} lets it: ask the user."
        );
        if !self.peers.view().advertises(&peer.key, STARTS) {
            return refused(older);
        }
        match self
            .call_peer(&peer, tool, Map::new(), caller, Keep::No, None)
            .await
        {
            Went::Answer(answer) => answer,
            Went::Failed(why) => refused(format!("On {name}: {why}")),
            Went::Older => refused(older),
            Went::Offline { .. } => refused(self.offline_words(&name)),
            Went::Away { .. } => {
                let doing = if tool == "list_models" {
                    "its models can't be read now"
                } else {
                    "its projects can't be read now"
                };
                refused(asleep(&name, doing))
            }
        }
    }

    pub fn offline_words(&self, name: &str) -> String {
        let here = self.settings.here();
        let mut chars = here.chars();
        let here = chars
            .next()
            .map(|first| first.to_uppercase().chain(chars).collect::<String>())
            .unwrap_or_default();
        format!(
            "{here} can't reach your other computers through the relay now, so nothing can be done on {name}. Try again later."
        )
    }

    /// stop_session, compact_session or answer_ask for a session that isn't here: on the computer
    /// it's on, or, where nothing says which, on each that takes calls (kept for those away, in
    /// case it's there).
    pub async fn elsewhere_tool(
        &self,
        tool: &str,
        caller: &Caller,
        args: &Map<String, Value>,
        session: &str,
    ) -> Said {
        let verb = verb_of(tool);
        let here = self.settings.here();
        let offline = format!(
            "Your other computers can't be reached through the relay now: your {verb} goes once {here}'s back on it, and you'll hear how it went as a message."
        );
        let mut args = args.clone();
        args.insert("session".into(), json!(session));
        if let Some(peer) = self.box_of(session) {
            let name = name_of(&peer);
            return match self
                .call_peer(&peer, tool, args, caller, Keep::Yes, None)
                .await
            {
                Went::Answer(answer) => answer,
                Went::Failed(why) => refused(format!("On {name}: {why}")),
                Went::Older => refused(format!(
                    "\"{session}\" is on {name}, which runs an older Kuvo Code: its sessions can't be reached from here until it's updated. Ask the user."
                )),
                Went::Offline { .. } => said(offline),
                Went::Away {
                    why, kept: false, ..
                } => refused(format!("{name} isn't answering ({why}). Try again later.")),
                Went::Away { why, .. } => said(format!(
                    "{name} isn't answering ({why}). Your {verb} is kept and done when it's back: you'll hear how it went as a message."
                )),
            };
        }
        let view = self.peers.view();
        let peers: Vec<Member> = self
            .members()
            .into_iter()
            .filter(|peer| view.advertises(&peer.key, CALLS))
            .collect();
        if peers.is_empty() {
            return refused(format!(
                "There's no session {session} here. Use list_sessions to see yours."
            ));
        }
        let asked = peers
            .iter()
            .map(|peer| self.call_peer(peer, tool, args.clone(), caller, Keep::Quietly, None));
        let outcomes = futures_util::future::join_all(asked).await;
        let mut away = Vec::new();
        let mut offline_any = false;
        for (peer, went) in peers.iter().zip(outcomes) {
            match went {
                Went::Answer(answer) => return answer,
                Went::Offline { .. } => offline_any = true,
                Went::Away { .. } => away.push(name_of(peer)),
                _ => {}
            }
        }
        if offline_any {
            return said(offline);
        }
        if !away.is_empty() {
            let (are, them) = if away.len() > 1 {
                ("aren't", "them")
            } else {
                ("isn't", "it")
            };
            return said(format!(
                "There's no session {session} here, or on your other computers that answered. {} {are} answering ({AWAY_WORDS}): your {verb} is kept for {them}, in case it's there, and you'll hear if it was.",
                away.join(" and ")
            ));
        }
        refused(format!(
            "There's no session {session} here or on your other computers. Use list_sessions to see yours."
        ))
    }

    /// A bot's Stop All reaching your other computers: each that takes it is told to stop the
    /// work under the bot's conversation there, kept for one that's away until it's back (only
    /// what was started before now: work the bot starts after it's back is its own).
    pub fn stop_work_elsewhere(&self, lead: &str, name: &str) {
        let Some(calls) = self.calls.get().cloned() else {
            return;
        };
        let view = self.peers.view();
        let before = jiff::Timestamp::now().as_millisecond();
        let name = plain(name, 100);
        let name = if name.is_empty() {
            "a bot".to_owned()
        } else {
            name
        };
        for peer in self
            .members()
            .into_iter()
            .filter(|peer| view.advertises(&peer.key, STARTS))
        {
            let request = Request {
                tool: Tool::StopWork,
                args: [("before".to_owned(), json!(before))].into_iter().collect(),
                lead: kuvo_peers::calls::Lead {
                    name: name.clone(),
                    free: false,
                    rules: Vec::new(),
                    bot: None,
                    start: None,
                },
            };
            let (calls, lead) = (calls.clone(), lead.to_owned());
            tokio::spawn(async move { calls.call(peer.key, lead, request, true, true).await });
        }
    }

    /// The computers `lead` started work on lately, by key.
    pub fn started_on(&self, lead: &str) -> Vec<String> {
        let file = self.home.join("run/work/started-elsewhere.json");
        let kept: BTreeMap<String, BTreeMap<String, i64>> = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let now = jiff::Timestamp::now().as_millisecond();
        let keys = kept.get(lead).into_iter().flatten();
        keys.filter(|(_, at)| now - **at <= STARTED_FOR)
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Notes that `lead` started work on the computer `key`, so its sessions there are asked for
    /// before their first rows come.
    pub fn note_started_on(&self, lead: &str, key: &ComputerKey) {
        let file = self.home.join("run/work/started-elsewhere.json");
        let mut kept: BTreeMap<String, BTreeMap<String, i64>> = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let now = jiff::Timestamp::now().as_millisecond();
        for keys in kept.values_mut() {
            keys.retain(|_, at| now - *at <= STARTED_FOR);
        }
        kept.retain(|_, keys| !keys.is_empty());
        kept.entry(lead.to_owned())
            .or_default()
            .insert(encode(key), now);
        if let Ok(text) = serde_json::to_vec(&kept) {
            let _ = kuvo_store::files::write_secret(&file, &text);
        }
    }

    /// What each other computer has of the caller's sessions (list_sessions): asked live of each
    /// that told it has some, or that the caller started one on; one that doesn't answer gives
    /// what it last told, as of then.
    pub async fn rows_everywhere(&self, caller: &Caller) -> Vec<RowsThere> {
        let kept = for_lead(&self.peers, &caller.session)
            .await
            .unwrap_or_default();
        let started: HashSet<String> = self.started_on(&caller.session).into_iter().collect();
        let view = self.peers.view();
        let asked: Vec<Member> = self
            .members()
            .into_iter()
            .filter(|peer| {
                (kept.iter().any(|each| each.computer == peer.key)
                    || started.contains(&encode(&peer.key)))
                    && view.advertises(&peer.key, CALLS)
            })
            .collect();
        let calls = asked
            .iter()
            .map(|peer| self.call_peer(peer, "list_sessions", Map::new(), caller, Keep::No, None));
        let outcomes = futures_util::future::join_all(calls).await;
        let mut there = Vec::new();
        for (peer, went) in asked.iter().zip(outcomes) {
            let name = name_of(peer);
            if let Went::Answer(answer) = &went
                && !answer.refused
                && let Ok(given) = serde_json::from_str::<Value>(&answer.text)
                && let Some(rows) = given.get("rows").and_then(Value::as_array)
            {
                let rows = rows
                    .iter()
                    .filter(|row| row["session"].is_string())
                    .cloned()
                    .collect();
                there.push(RowsThere {
                    name,
                    rows,
                    as_of: None,
                    away: None,
                });
                continue;
            }
            let Some(before) = kept.iter().find(|each| each.computer == peer.key) else {
                continue;
            };
            let away = match went {
                Went::Away { why, .. } => why.to_owned(),
                Went::Offline { .. } => "this computer is off the relay".into(),
                Went::Answer(_) => "its answer didn't come whole".into(),
                _ => AWAY_WORDS.into(),
            };
            let rows = before.rows.iter().map(row_value).collect();
            there.push(RowsThere {
                name,
                rows,
                as_of: Some(before.at),
                away: Some(away),
            });
        }
        there
    }
}

fn row_value(row: &Row) -> Value {
    serde_json::to_value(row).unwrap_or_default()
}

/// A lead's sessions on another computer, as list_sessions says them: by that computer's name,
/// live, or as of when it last told (`as_of`) when it isn't answering (`away`, why).
pub struct RowsThere {
    pub name: String,
    pub rows: Vec<Value>,
    pub as_of: Option<i64>,
    pub away: Option<String>,
}

pub fn asleep(name: &str, doing: &str) -> String {
    format!(
        "{name} isn't answering: it may be asleep, off or offline, so {doing}. Nothing can wake it from here: try again once it's back (a wake-up can remind you), or do it on another computer."
    )
}
