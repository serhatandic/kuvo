//! Agents writing to each other (PROTOCOL.md, "Agents' tools"): who else there is (list_agents),
//! a message to one (send_message) and one's conversation read (read_conversation), here or on
//! another of your computers, where it goes by mail as a letter left in the outbox (the peers send
//! it, and its answer comes back the same way).

use std::time::Duration;

use http::Method;
use kuvo_engine::{BotEngine, LetterDelivery, LetterPrompt, LetterText, TranscriptScope, folded};
use kuvo_peers::{
    calls::plain,
    directory::{AgentKind, Member},
};
use serde_json::{Map, Value, json};

use super::{Caller, Said, calls::name_of, refused, said, session_id};
use crate::work::{
    Work,
    watch::{encoded, is_archived, tools::under},
};

/// How long a read by mail is waited for, and how much of it is taken.
const READ_WAIT: Duration = Duration::from_secs(660);
const READ_BY_MAIL: usize = 20_000;
/// How much of a letter goes.
const LETTER_TEXT: usize = 32_000;

/// Someone a tool here can name: this computer's own, or another's (`computer`, its name).
#[derive(Clone)]
pub struct Agent {
    pub session: String,
    pub name: String,
    pub bot: bool,
    pub project: Option<String>,
    pub about: Option<String>,
    pub computer: Option<String>,
}

/// Where the agent named is: its conversation here, or the computer it's on.
enum Place {
    Here(Value),
    Away(Member),
}

fn cut(text: &str, most: usize) -> String {
    text.chars().take(most).collect()
}

impl Work {
    /// Everyone, as they are now: this computer's bots, then its latest conversations (not a
    /// subagent's, a seat, nor one put away), then each other computer's it alone lists; and
    /// whether this computer's own could be listed now.
    pub(super) async fn everyone(&self, engine: Option<&BotEngine>) -> (Vec<Agent>, bool) {
        let bots: Vec<_> = crate::wakes::folder::names(&self.bots)
            .into_iter()
            .filter_map(|id| crate::bots::here::BotHere::read(&self.bots, &id))
            .collect();
        let mut agents: Vec<Agent> = bots
            .iter()
            .map(|bot| {
                let text = |field: &str| {
                    bot.described
                        .get(field)
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                        .map(str::to_owned)
                };
                Agent {
                    session: bot.session.clone(),
                    name: text("name").unwrap_or_default(),
                    bot: true,
                    project: None,
                    about: text("job"),
                    computer: None,
                }
            })
            .collect();
        let page = match engine {
            Some(engine) => engine
                .call(
                    Method::GET,
                    "api/session?parentID=null&limit=100&order=desc",
                    None,
                    Duration::from_secs(10),
                )
                .await
                .ok(),
            None => None,
        };
        let fresh = page.is_some();
        let mut sessions: Vec<Value> = page
            .map(|page| {
                page["data"]
                    .as_array()
                    .cloned()
                    .unwrap_or_else(|| page.as_array().cloned().unwrap_or_default())
            })
            .unwrap_or_default();
        let frozen = self.peers.view().local.ownership.frozen.clone();
        sessions.retain(|data| {
            let id = data["id"].as_str().unwrap_or_default();
            data["parentID"].is_null()
                && !bots.iter().any(|bot| bot.session == id)
                && !frozen.contains(id)
                && data["metadata"]["kuvo"]["seat"].is_null()
                && !is_archived(data)
        });
        sessions.sort_by(|a, b| {
            b["time"]["updated"]
                .as_f64()
                .unwrap_or(0.0)
                .total_cmp(&a["time"]["updated"].as_f64().unwrap_or(0.0))
        });
        let projects = kuvo_services::git::read_projects(&self.home).unwrap_or_default();
        let chats = self.home.join("chats");
        for data in sessions.iter().take(40) {
            let folder = data["location"]["directory"]
                .as_str()
                .or(data["directory"].as_str());
            let named = folder
                .and_then(|folder| {
                    projects
                        .iter()
                        .find(|each| each.get("directory").and_then(Value::as_str) == Some(folder))
                })
                .and_then(|project| {
                    project
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                });
            let project = named.or_else(|| {
                let folder = std::path::Path::new(folder?);
                if folder.starts_with(&chats) {
                    Some("Chats".into())
                } else {
                    folder
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                }
            });
            let title = data["title"]
                .as_str()
                .filter(|title| !title.is_empty())
                .unwrap_or("New session");
            agents.push(Agent {
                session: data["id"].as_str().unwrap_or_default().into(),
                name: plain(title, 200),
                bot: false,
                project,
                about: None,
                computer: None,
            });
        }
        // Each other computer's, those it alone lists.
        let view = self.peers.view();
        let members = view.roster.members.clone();
        let told: Vec<_> = members
            .iter()
            .map(|member| (member, view.told_agents(&member.key)))
            .collect();
        for (member, list) in &told {
            for agent in list {
                let listing = told
                    .iter()
                    .filter(|(_, list)| list.iter().any(|each| each.session == agent.session))
                    .count();
                if listing > 1 || agents.iter().any(|each| each.session == agent.session) {
                    continue;
                }
                agents.push(Agent {
                    session: agent.session.clone(),
                    name: agent.name.clone(),
                    bot: agent.kind == AgentKind::Bot,
                    project: None,
                    about: None,
                    computer: Some(name_of(member)),
                });
            }
        }
        (agents, fresh)
    }

    /// What an agent is, for one here to read: "bot, on the Mac", "session, here, in api",
    /// "session, on devbox".
    fn what_agent(&self, agent: &Agent) -> String {
        let place = match &agent.computer {
            Some(computer) => format!("on {computer}"),
            None if self.settings.is_mac() => "on the Mac".into(),
            None => "here".into(),
        };
        let kind = if agent.bot { "bot" } else { "session" };
        let project = agent
            .project
            .as_ref()
            .map(|project| format!(", in {project}"))
            .unwrap_or_default();
        format!("{kind}, {place}{project}")
    }

    /// The list, for an agent to read (not the one asking).
    pub(super) fn describe_agents(&self, agents: &[Agent], caller: Option<&str>) -> String {
        let lines: Vec<String> = agents
            .iter()
            .filter(|agent| Some(agent.session.as_str()) != caller)
            .map(|agent| {
                let about = agent
                    .about
                    .as_ref()
                    .map(|about| format!(": {about}"))
                    .unwrap_or_default();
                format!(
                    "- {} ({}) address: {}{about}",
                    agent.name,
                    self.what_agent(agent),
                    agent.session
                )
            })
            .collect();
        if lines.is_empty() {
            return "No other agents yet.".into();
        }
        format!(
            "Agents you can write to with send_message, or whose conversation you can read with read_conversation (by name or address):\n{}",
            lines.join("\n")
        )
    }

    /// The agent `wanted` names (its name, with "&" or not, or its address), and where it is; or
    /// why it can't be written to (`reading`: read).
    async fn find_agent(
        &self,
        engine: Option<&BotEngine>,
        wanted: &str,
        caller: Option<&str>,
        (agents, fresh): &(Vec<Agent>, bool),
        reading: bool,
    ) -> Result<(Agent, Place), Said> {
        let mut name = wanted.trim();
        if name.starts_with('&') && !agents.iter().any(|agent| agent.name == name) {
            name = &name[1..];
        }
        let address = session_id(name);
        let mut matches: Vec<&Agent> = agents
            .iter()
            .filter(|agent| agent.session == name)
            .collect();
        if matches.is_empty() && !address {
            matches = agents
                .iter()
                .filter(|agent| folded(&agent.name) == folded(name))
                .collect();
        }
        if matches.len() > 1 {
            let listed: Vec<Agent> = matches.into_iter().cloned().collect();
            return Err(refused(format!(
                "More than one agent is called {name}: use one's address.\n{}",
                self.describe_agents(&listed, caller)
            )));
        }
        if matches.is_empty() && !address {
            if !fresh {
                return Err(refused(format!(
                    "Couldn't look up {name}: this computer's agents didn't finish listing their conversations. Try again, or use the conversation's address."
                )));
            }
            // Part of a name ("Release" for "Release notes"): those it could be.
            let close: Vec<Agent> = agents
                .iter()
                .filter(|agent| folded(&agent.name).contains(&folded(name)))
                .take(8)
                .cloned()
                .collect();
            let hint = if close.is_empty() {
                "Use list_agents to see who there is.".to_owned()
            } else {
                format!(
                    "Did you mean one of these?\n{}",
                    self.describe_agents(&close, caller)
                )
            };
            return Err(refused(format!("There's no agent called {name}. {hint}")));
        }
        let listed = matches.first().map(|agent| (*agent).clone());
        let session = listed
            .as_ref()
            .map_or(name, |agent| agent.session.as_str())
            .to_owned();
        if Some(session.as_str()) == caller {
            return Err(refused(if reading {
                "That's this conversation: you have it already."
            } else {
                "That's you."
            }));
        }
        let title = listed
            .as_ref()
            .map_or(session.clone(), |agent| agent.name.clone());
        let denied = |why: &str| {
            refused(format!(
                "Couldn't {} {title}: {why}",
                if reading { "read" } else { "write to" }
            ))
        };
        if !reading && self.peers.view().local.ownership.frozen.contains(&session) {
            return Err(refused(format!(
                "{title} is moving to another computer. Try again in a minute."
            )));
        }
        let data = match engine {
            Some(engine) => engine
                .call(
                    Method::GET,
                    &format!("api/session/{}", encoded(&session)),
                    None,
                    Duration::from_secs(10),
                )
                .await
                .ok()
                .filter(|data| data["id"] == session.as_str()),
            None => None,
        };
        let Some(data) = data else {
            let Some(peer) = self.box_of(&session) else {
                return Err(denied(
                    "No computer has confirmed where this conversation is. Check its address with list_agents.",
                ));
            };
            let agent = listed.unwrap_or(Agent {
                session: session.clone(),
                name: "New session".into(),
                bot: false,
                project: None,
                about: None,
                computer: None,
            });
            return Ok((
                Agent {
                    computer: Some(agent.computer.clone().unwrap_or_else(|| name_of(&peer))),
                    ..agent
                },
                Place::Away(peer),
            ));
        };
        if !data["parentID"].is_null() {
            return Err(denied("It's a subagent's conversation."));
        }
        if !data["metadata"]["kuvo"]["seat"].is_null() {
            return Err(denied("It's a seat in a group chat."));
        }
        if is_archived(&data) {
            // (One of the caller's own sessions it may still read.)
            let all = match (reading, caller, engine) {
                (true, Some(_), Some(engine)) => {
                    super::super::watch::tools::all_work_archived(engine)
                        .await
                        .unwrap_or_default()
                }
                _ => Vec::new(),
            };
            if !caller.is_some_and(|caller| {
                under(caller, &all)
                    .iter()
                    .any(|each| each["id"] == session.as_str())
            }) {
                return Err(denied("It's archived."));
            }
        }
        let bot = crate::wakes::folder::names(&self.bots).iter().any(|id| {
            crate::bots::here::BotHere::read(&self.bots, id)
                .is_some_and(|bot| bot.session == session)
        });
        let title = data["title"]
            .as_str()
            .filter(|title| !title.is_empty())
            .unwrap_or("New session");
        let agent = listed.unwrap_or(Agent {
            session: session.clone(),
            name: title.into(),
            bot,
            project: None,
            about: None,
            computer: None,
        });
        Ok((agent, Place::Here(data)))
    }

    /// Who a conversation is to the others: its name in the list, else where it is.
    fn sender_name(&self, caller: Option<&Caller>) -> String {
        match caller {
            Some(caller) => caller.name.clone(),
            None if self.settings.is_mac() => "another agent".into(),
            None => format!("an agent on {}", self.own_name()),
        }
    }

    /// `letter` left in the outbox for the peers to send (`run/outbox/sending`, the TypeScript
    /// box's too): its id.
    fn leave_letter(&self, letter: Value) -> std::io::Result<String> {
        let mut bytes = [0u8; 16];
        let _ = getrandom::fill(&mut bytes);
        let id = format!("ltr_{}", kuvo_wire::keys::encode(&bytes));
        let folder = self.home.join("run/outbox/sending");
        kuvo_store::files::create_private_dirs(&folder)?;
        let mut letter = letter;
        letter["at"] = json!(jiff::Timestamp::now().as_millisecond());
        kuvo_store::files::write_secret(
            &folder.join(format!("{id}.json")),
            &serde_json::to_vec(&letter)?,
        )?;
        Ok(id)
    }

    /// send_message: a message from `caller` to the agent `args.to` names, wherever it runs. One
    /// cut short (`operation`) goes again under the id it had, so it's never there twice.
    pub(super) async fn send_message_tool(
        &self,
        engine: Option<&BotEngine>,
        caller: Option<&Caller>,
        args: &Map<String, Value>,
        operation: Option<&super::receipts::Operation>,
    ) -> Result<Said, String> {
        let text = |field: &str| {
            args.get(field)
                .and_then(Value::as_str)
                .filter(|text| !text.trim().is_empty())
        };
        let Some(to) = text("to") else {
            return Ok(refused("Say who to write to (to)."));
        };
        let Some(words) = text("text") else {
            return Ok(refused("Say what to write (text)."));
        };
        let all = self.everyone(engine).await;
        let (target, place) = match self
            .find_agent(
                engine,
                to,
                caller.map(|caller| caller.session.as_str()),
                &all,
                false,
            )
            .await
        {
            Ok(found) => found,
            Err(said) => return Ok(said),
        };
        // A few at a time: two agents shouldn't keep writing to each other unattended. Taken back
        // if it doesn't go, so one tried again isn't counted twice.
        let key = caller.map_or("outside", |caller| caller.session.as_str());
        // (One cut short and sent again was counted the first time.)
        let got = operation.map(super::receipts::Operation::progress);
        let got = got.unwrap_or_default();
        let again = got["to"] == target.session.as_str();
        let counted = if again {
            Some(0)
        } else {
            self.count_message(key)
        };
        let Some(counted) = counted else {
            return Ok(refused(
                "You've sent a lot of messages in the last few minutes. Wait a while, or ask the user how to go on.",
            ));
        };
        let failed = |why: String| {
            self.message_back(key, counted);
            Ok(refused(why))
        };
        let from = caller.map_or("unknown", |caller| caller.session.as_str());
        let sender = self.sender_name(caller);
        let answer = match place {
            Place::Here(data) => {
                let hint = if from == "unknown" {
                    "It can't be answered: who sent it isn't known.".to_owned()
                } else {
                    format!("To answer, use send_message to {from}.")
                };
                let letter = format!("[Message from {sender} · {from}] ({hint})\n\n{words}");
                let parts = kuvo_engine::parts_metadata(&letter);
                let id = match got["message"].as_str().filter(|_| again) {
                    Some(id) => Some(id.to_owned()),
                    None => self.new_message_id().await,
                };
                let (Some(id), Ok(mover)) = (id, self.list.engine().session_move().await) else {
                    return failed(format!(
                        "Delivery to {} couldn't be confirmed: the engine isn't running.",
                        target.name
                    ));
                };
                if let Some(operation) = operation {
                    operation.keep(&json!({ "to": target.session, "message": id }));
                }
                let prompt = LetterPrompt {
                    id,
                    immediate: LetterText {
                        text: letter,
                        metadata: Some(parts),
                    },
                    when_queued: None,
                    from: sender.clone(),
                    words: words.to_owned(),
                    steer: true,
                };
                let went = match mover.deliver_letter(&target.session, prompt).await {
                    Ok(went) => went,
                    // (Its operation's to be sent again, and goes under the same id.)
                    Err(_) if operation.is_some() => {
                        let name = &target.name;
                        return Err(format!(
                            "Waiting for {name}'s conversation to confirm delivery."
                        ));
                    }
                    Err(refusal) => {
                        return failed(format!(
                            "Delivery to {} couldn't be confirmed: {}",
                            target.name,
                            super::super::watch::tools::refusal_words(refusal).await
                        ));
                    }
                };
                if let Some(caller) = caller {
                    let mut data = data;
                    data["id"] = json!(target.session);
                    self.note_told(&caller.session, &data);
                }
                match went {
                    LetterDelivery::Accepted => format!(
                        "Sent to {}. Its answer, if it writes one, comes to you as a message.",
                        target.name
                    ),
                    LetterDelivery::Queued(reason) => named_reason(&reason, &target.name),
                }
            }
            Place::Away(peer) => {
                let letter = json!({ "from": caller.map(|caller| caller.session.clone()), "to": target.session, "text": cut(words, LETTER_TEXT), "peer": kuvo_wire::keys::encode(&peer.key), "wait": false });
                if !(again && got["left"] == true) && self.leave_letter(letter).is_err() {
                    return failed(format!("It didn't reach {}.", target.name));
                }
                if let Some(operation) = operation {
                    operation.keep(&json!({ "to": target.session, "left": true }));
                }
                if !self.peers.view().relay_connected {
                    format!(
                        "Your other computers can't be reached through the relay now: it goes once {}'s back on it, and its answer comes to you as a message.",
                        self.settings.here()
                    )
                } else {
                    let on = target
                        .computer
                        .as_ref()
                        .map(|computer| format!(" on {computer}"))
                        .unwrap_or_default();
                    format!(
                        "On its way to {}{on}. Its answer, if it writes one, comes to you as a message.",
                        target.name
                    )
                }
            }
        };
        if let Some(caller) = caller {
            self.note_sent(&caller.session, &target.session, words);
        }
        Ok(said(answer))
    }

    /// read_conversation: the conversation of the agent `args.of` names, as you'd read it (a long
    /// one as its start and latest part; with `query`, the parts that mention it). One on another
    /// computer comes by mail from one that shares them (`letters-read`).
    pub(super) async fn read_conversation_tool(
        &self,
        engine: Option<&BotEngine>,
        caller: Option<&Caller>,
        args: &Map<String, Value>,
    ) -> Said {
        let of = args
            .get("of")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        if of.is_empty() {
            return refused("Say whose conversation to read (of).");
        }
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|query| !query.is_empty());
        let all = self.everyone(engine).await;
        let (target, place) = match self
            .find_agent(
                engine,
                of,
                caller.map(|caller| caller.session.as_str()),
                &all,
                true,
            )
            .await
        {
            Ok(found) => found,
            Err(said) => return said,
        };
        let name = &target.name;
        let intro = format!(
            "{name}'s conversation ({}, address {}), oldest first. What's under **User** is the user's; under **{name}**, its agent's.\n\n",
            self.what_agent(&target),
            target.session
        );
        let unreadable = |why: &str| refused(format!("Couldn't read {name}'s conversation: {why}"));
        match place {
            Place::Here(data) => {
                let cancel = tokio_util::sync::CancellationToken::new();
                match self
                    .list
                    .engine()
                    .transcript(
                        &target.session,
                        name,
                        query,
                        TranscriptScope::Local,
                        &cancel,
                    )
                    .await
                {
                    Ok(words) => {
                        if let Some(caller) = caller {
                            self.note_read(&caller.session, &data);
                        }
                        said(format!("{intro}{words}"))
                    }
                    Err(error) => unreadable(&error.to_string()),
                }
            }
            Place::Away(peer) => {
                let Some(caller) = caller else {
                    return unreadable(
                        "the other computer needs a conversation to send its answer to.",
                    );
                };
                let view = self.peers.view();
                if !view.relay_connected {
                    return unreadable("this computer isn't connected to the relay.");
                }
                if !view.advertises(&peer.key, "letters-read") {
                    return refused(format!(
                        "{name}'s computer doesn't share its conversations yet (it runs an older Kuvo Code). Ask it what you need with send_message."
                    ));
                }
                // Its computer isn't there: said at once, not kept (a read wanted now is no use next week).
                let asleep = format!(
                    "{} isn't answering (it may be asleep, off or offline), so {name}'s conversation can't be read now. Try again later.",
                    name_of(&peer)
                );
                if view.away(&peer.key) {
                    return refused(asleep);
                }
                let read = query.map_or_else(|| json!({}), |query| json!({ "query": query }));
                let letter = json!({ "from": caller.session, "to": target.session, "text": query.unwrap_or("read"), "peer": kuvo_wire::keys::encode(&peer.key), "wait": true, "read": read });
                let Ok(id) = self.leave_letter(letter) else {
                    return unreadable("this computer couldn't save the request to send it.");
                };
                match self.letter_answer(&id, &peer).await {
                    Some(Ok(answer)) => said(format!("{intro}{}", cut(&answer, READ_BY_MAIL))),
                    Some(Err(why)) => unreadable(&why),
                    None if self.peers.view().away(&peer.key) => refused(asleep),
                    None => unreadable(&format!("{} didn't send it back in time.", name_of(&peer))),
                }
            }
        }
    }

    /// The answer to letter `id` (`run/replies/<id>.json`), its words or why none will come; none
    /// once it's waited long enough or `peer` isn't there. Then it's given up on, and one that
    /// comes later reaches the conversation as a message.
    async fn letter_answer(&self, id: &str, peer: &Member) -> Option<Result<String, String>> {
        let replies = self.home.join("run/replies");
        let _ = kuvo_store::files::create_private_dirs(&replies);
        let (file, alive) = (
            replies.join(format!("{id}.json")),
            replies.join(format!("{id}.alive")),
        );
        let began = tokio::time::Instant::now();
        let mut answer = None;
        while began.elapsed() < READ_WAIT {
            let _ = std::fs::write(&alive, "");
            tokio::time::sleep(Duration::from_secs(1)).await;
            if let Some(reply) = std::fs::read(&file)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            {
                let _ = std::fs::remove_file(&file);
                answer = Some(
                    match (
                        reply["text"].as_str(),
                        reply["failed"].as_str().or(reply["note"].as_str()),
                    ) {
                        (Some(text), _) => Ok(text.to_owned()),
                        (None, why) => Err(why.unwrap_or("it couldn't be read").to_owned()),
                    },
                );
                break;
            }
            if self.peers.view().away(&peer.key) {
                break;
            }
        }
        if answer.is_none() {
            let _ = std::fs::write(replies.join(format!("{id}.gaveup")), "");
        }
        let _ = std::fs::remove_file(&alive);
        answer
    }

    async fn new_message_id(&self) -> Option<String> {
        self.list
            .engine()
            .wake_engine(false)
            .await?
            .message_id()
            .ok()
    }

    /// A bot's conversation telling or reading one of its sessions, for clients to show it
    /// looking after it.
    fn note_told(&self, by: &str, data: &Value) {
        self.note_looked(by, data, "told");
    }

    fn note_read(&self, by: &str, data: &Value) {
        self.note_looked(by, data, "read");
    }

    fn note_looked(&self, by: &str, data: &Value, kind: &str) {
        if let (Some(bot), Some(id)) = (super::super::watch::works_for(data), data["id"].as_str())
            && self
                .bot_of_conversation(by)
                .is_some_and(|here| here.id == bot)
        {
            self.records.note_oversight(bot, id, kind);
        }
    }
}

/// Why a message waits, naming the conversation it waits in.
fn named_reason(reason: &str, name: &str) -> String {
    if reason.starts_with("That conversation is stopped: the user paused it.") {
        return reason.replacen(
            "That conversation is stopped: the user paused it.",
            &format!("{name} is stopped: the user paused its conversation."),
            1,
        );
    }
    reason
        .replacen("that conversation", name, 1)
        .replacen("That conversation", name, 1)
}
