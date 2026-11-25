//! The bots of your other computers, as each last told this box (`elsewhere` in `kuvo/bots`,
//! PROTOCOL.md, `bots.elsewhere`): what this box already hears from its peers (their agents), as a
//! list a client can draw.

use std::{collections::BTreeMap, path::Path};

use kuvo_peers::{
    directory::Look,
    lists::{BotActivity, BotsTold},
    reach::{Millis, Standing},
    session::ComputerKind,
    setup_lists::View,
};
use kuvo_wire::keys::encode;
use serde_json::{Map, Value, json};

use super::{
    here::BotHere,
    list::{Deletes, delete_lines},
};

/// A bot one of your other computers lists, as that computer last told this box.
pub struct Elsewhere {
    pub id: String,
    pub session: String,
    pub name: String,
    pub look: Option<Look>,
    pub job: Option<String>,
    /// The computer it lives on: its key, its name as a client shows it, and its kind.
    pub key: String,
    pub computer: String,
    pub kind: ComputerKind,
    pub deletes: Vec<Deletes>,
    pub standing: Standing,
    /// What it's doing, as its computer's lists last said, where they said it of this conversation.
    pub activity: Option<BotActivity>,
}

/// Where a deleted bot's folder ends up on `computer`, as it said: the Finder's Trash, or Kuvo
/// Code's own trash folder there; one that hasn't (from before it did) is said both ways.
fn folder_goes(computer: &str, trash: Option<&str>) -> String {
    match trash {
        Some("trash") => format!("goes to the Trash on {computer}"),
        Some(_) => format!("goes to Kuvo Code’s trash folder on {computer}"),
        None => format!("goes to the Trash on {computer}, or to Kuvo Code’s trash folder there"),
    }
}

/// The bots your other computers list, each once: one two of them list (it's moving from one to
/// the other) is the one's that told this box last; one that lives here is this box's own,
/// whatever another still says; one whose move away from here is finishing is the computer's it
/// went to. By computer, as this box knows them, then as each listed its own.
pub fn bots_elsewhere(view: &View, run: &Path, bots: &Path, now: Millis) -> Vec<Elsewhere> {
    let told = view.bots_told(now);
    let mut latest: BTreeMap<&str, (usize, Millis)> = BTreeMap::new();
    for (at, computer) in told.iter().enumerate() {
        for bot in &computer.bots {
            let id = bot.id.as_deref().unwrap_or_default();
            if latest
                .get(id)
                .is_none_or(|(_, heard)| computer.heard > *heard)
            {
                latest.insert(id, (at, computer.heard));
            }
        }
    }
    let gone_to = kuvo_peers::moves::bots_gone_to(run);
    let away = kuvo_peers::moves::away_sessions(run);
    let mut listed = Vec::new();
    for (at, computer) in told.iter().enumerate() {
        let BotsTold {
            computer: member,
            standing,
            trash,
            activity,
            ..
        } = computer;
        let key = encode(&member.key);
        let name = &member.name;
        for bot in &computer.bots {
            let id = bot.id.as_deref().unwrap_or_default();
            let its_own = match gone_to.get(id) {
                Some(to) => *to == key,
                None => {
                    latest.get(id).map(|(latest, _)| *latest) == Some(at)
                        && BotHere::read(bots, id).is_none_or(|here| away.contains(&here.session))
                }
            };
            if !its_own {
                continue;
            }
            // (Listed once by each computer; one that names a bot twice is told once.)
            latest.remove(id);
            listed.push(Elsewhere {
                id: id.to_owned(),
                session: bot.session.clone(),
                name: bot.name.clone(),
                look: bot.look.clone(),
                job: bot.about.clone(),
                key: key.clone(),
                computer: name.clone(),
                kind: member.kind,
                // (Its wake-ups aren't told between computers: said without a count.)
                deletes: delete_lines(name, folder_goes(name, *trash), None),
                standing: *standing,
                activity: activity
                    .and_then(|all| all.get(id))
                    .filter(|activity| activity.session == bot.session)
                    .cloned(),
            });
        }
    }
    listed
}

impl Elsewhere {
    /// As `kuvo/bots` says it: away with when it was last heard from, or switching to a new
    /// release; and what it's doing as its computer said.
    pub fn said(&self) -> Value {
        let mut said = Map::new();
        said.insert("id".into(), json!(self.id));
        said.insert("session".into(), json!(self.session));
        said.insert("name".into(), json!(self.name));
        if let Some(look) = &self.look {
            said.insert("look".into(), json!(look));
        }
        if let Some(job) = &self.job {
            said.insert("job".into(), json!(job));
        }
        said.insert("box".into(), json!(self.key));
        said.insert("computer".into(), json!(self.computer));
        said.insert("kind".into(), json!(self.kind));
        said.insert("deletes".into(), json!(self.deletes));
        match self.standing {
            Standing::Away { last_seen } => {
                said.insert("away".into(), json!(true));
                if let Some(seen) = last_seen {
                    said.insert("lastSeen".into(), json!(seen));
                }
            }
            Standing::Updating => {
                said.insert("updating".into(), json!(true));
            }
            Standing::Live => {}
        }
        if let Some(activity) = &self.activity {
            said.insert("now".into(), json!(activity.now));
            if let Some(paused) = activity.paused {
                said.insert("paused".into(), json!(paused));
            }
            if let Some(wakes) = &activity.wakes {
                said.insert("wakes".into(), json!(wakes));
            }
        }
        Value::Object(said)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleting_a_bot_elsewhere_says_where_its_folder_goes_as_its_computer_told() {
        let folder = |trash| {
            delete_lines("Studio", folder_goes("Studio", trash), None)
                .into_iter()
                .map(|line| (line.what, line.words))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            folder(Some("trash"))[1].1,
            "Its folder, with its memory and what it made there, goes to the Trash on Studio."
        );
        assert_eq!(
            folder(Some("kuvo"))[1].1,
            "Its folder, with its memory and what it made there, goes to Kuvo Code’s trash folder on Studio."
        );
        let unsaid = folder(None);
        assert_eq!(
            unsaid[1].1,
            "Its folder, with its memory and what it made there, goes to the Trash on Studio, or to Kuvo Code’s trash folder there."
        );
        // (Its wake-ups aren't told between computers: said without a count.)
        assert_eq!(
            unsaid.iter().map(|(what, _)| *what).collect::<Vec<_>>(),
            ["conversation", "folder", "wakes", "groups", "sessions"]
        );
        assert_eq!(unsaid[2].1, "Its wake-ups and watches are cancelled.");
    }
}
