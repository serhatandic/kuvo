//! The names known to this resident, and the addresses written into a prompt.

use std::collections::{BTreeMap, HashSet};

use serde::Deserialize;
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};
use unicode_segmentation::UnicodeSegmentation;

use crate::{Engine, disk, parts, session_list::LocalSessions};

#[derive(Clone, Deserialize)]
pub(crate) struct Name {
    pub session: String,
    pub name: String,
    pub kind: Kind,
    pub project: Option<String>,
    pub machine: Option<String>,
    pub about: Option<String>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    Session,
    Bot,
}

#[derive(Default, Deserialize)]
pub(crate) struct Naming {
    #[serde(rename = "machine")]
    pub here: Option<String>,
    #[serde(default)]
    pub agents: Vec<Name>,
}

impl Naming {
    pub(crate) async fn local(engine: &Engine, list: &LocalSessions) -> Self {
        // Keep accepting the directory written by the TypeScript resident.
        let mut known = disk::read::<Self>(&engine.home.join("run/agents.json"))
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        let mut existing = known
            .agents
            .iter()
            .map(|agent| agent.session.clone())
            .collect::<HashSet<_>>();
        known.agents.extend(
            list.names(known.here.as_deref())
                .into_iter()
                .filter(|agent| existing.insert(agent.session.clone())),
        );
        known
            .agents
            .sort_by_key(|agent| !matches!(agent.kind, Kind::Bot));
        known
    }

    pub(crate) fn names<'a>(
        &'a self,
        text: &str,
        excluding: &str,
        picked: &[&str],
    ) -> Result<Vec<&'a Name>, &'static str> {
        if !text.contains('&') {
            return Ok(Vec::new());
        }
        if picked
            .iter()
            .any(|id| !self.agents.iter().any(|agent| agent.session == *id))
        {
            return Err(
                "An agent you named with & isn't known to this computer yet. Try again in a moment.",
            );
        }
        let mut candidates = BTreeMap::new();
        for agent in self
            .agents
            .iter()
            .filter(|agent| agent.session != excluding && !agent.name.is_empty())
        {
            let key = folded(&agent.name);
            if picked.contains(&agent.session.as_str()) {
                candidates.insert(key, agent);
            } else {
                candidates.entry(key).or_insert(agent);
            }
        }
        let mut candidates = candidates.into_iter().collect::<Vec<_>>();
        candidates.sort_by_key(|(key, _)| std::cmp::Reverse(key.graphemes(true).count()));

        // Each folded grapheme can span several letters. Match at whole grapheme boundaries
        // so &Straße can match &Strasse without an interior part of ß matching separately.
        let mut boundaries = BTreeMap::new();
        let mut normalized = String::new();
        let mut previous_word = false;
        for grapheme in text.graphemes(true) {
            let word = grapheme.chars().next().is_some_and(char::is_alphanumeric);
            boundaries.insert(normalized.len(), (previous_word, word));
            normalized.push_str(&folded(grapheme));
            previous_word = word;
        }
        boundaries.insert(normalized.len(), (previous_word, false));
        let mut found: Vec<(usize, usize, &Name)> = Vec::new();
        for (key, agent) in candidates.into_iter().filter(|(key, _)| !key.is_empty()) {
            let pattern = format!("&{key}");
            for (start, _) in normalized.match_indices(&pattern) {
                let end = start + pattern.len();
                let open = boundaries.get(&start).is_some_and(|(before, _)| !before);
                let close = boundaries.get(&end).is_some_and(|(_, after)| !after);
                if open && close && !found.iter().any(|(from, to, _)| start < *to && *from < end) {
                    found.push((start, end, agent));
                }
            }
        }
        found.sort_by_key(|(start, _, _)| *start);
        let mut seen = HashSet::new();
        Ok(found
            .into_iter()
            .filter_map(|(_, _, agent)| seen.insert(&agent.session).then_some(agent))
            .collect())
    }

    pub(crate) fn line(&self, agent: &Name) -> String {
        let kind = match agent.kind {
            Kind::Session => "session",
            Kind::Bot => "bot",
        };
        let location = match agent.machine.as_deref() {
            None => "on the Mac".to_owned(),
            current if current == self.here.as_deref() => "here".to_owned(),
            Some(machine) => format!("on {machine}"),
        };
        let mut line = format!("- &{} ({}): {kind}, {location}", agent.name, agent.session);
        if let Some(project) = &agent.project {
            line.push_str(", in ");
            line.push_str(project);
        }
        if let Some(about) = agent.about.as_deref().filter(|about| !about.is_empty()) {
            line.push_str(": ");
            line.push_str(about);
        }
        line
    }

    pub(crate) fn parts<'a>(&self, named: &[&'a Name]) -> Vec<parts::Agent<'a>> {
        named
            .iter()
            .map(|agent| parts::Agent {
                name: &agent.name,
                session: &agent.session,
            })
            .collect()
    }
}

fn folded(text: &str) -> String {
    let expanded = text
        .chars()
        .flat_map(|code| {
            if ('\u{fb00}'..='\u{fb06}').contains(&code) {
                code.to_string().nfkd().collect::<Vec<_>>()
            } else {
                vec![code]
            }
        })
        .collect::<String>();
    let mut result = String::new();
    for letter in expanded
        .nfd()
        .filter(|letter| !is_combining_mark(*letter))
        .flat_map(char::to_lowercase)
    {
        match letter {
            'ς' => result.push('σ'),
            'ß' => result.push_str("ss"),
            letter => result.push(letter),
        }
    }
    result
}
