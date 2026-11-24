//! Permission rules checked by the resident before any native policy runs.

mod allowance;
mod settings;
mod shell;
#[cfg(test)]
mod tests;
pub(crate) mod turn;

pub use allowance::{covers, remember};
pub use settings::{ConfigLocation, read_blocks};

use crate::session::PermissionAsk;
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    Allow,
    Ask,
    #[serde(other)]
    Deny,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct Rule {
    pub action: String,
    pub resource: String,
    pub effect: Effect,
}

impl Rule {
    pub fn new(action: impl Into<String>, resource: impl Into<String>, effect: Effect) -> Self {
        Self {
            action: action.into(),
            resource: resource.into(),
            effect,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Allowance {
    pub action: String,
    pub patterns: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Policy {
    /// Missing when the resident could not establish which Blocks apply.
    pub blocks: Option<Vec<Rule>>,
    pub agent_rules: Vec<Rule>,
    pub session_rules: Vec<Rule>,
    pub allowed: Vec<Allowance>,
    pub directory: PathBuf,
    pub real_directory: Option<PathBuf>,
    pub person_home: Option<PathBuf>,
    pub own_folders: Vec<PathBuf>,
    pub attachments: BTreeSet<PathBuf>,
    path_types: std::collections::BTreeMap<PathBuf, bool>,
    /// Auto relies on a sandbox. The caller supplies whether one applies to
    /// the process which asked.
    pub auto_without_sandbox: bool,
}

impl Policy {
    pub fn new(directory: PathBuf, blocks: Option<Vec<Rule>>) -> Self {
        Self {
            blocks,
            agent_rules: vec![
                Rule::new("read", "*.env", Effect::Ask),
                Rule::new("read", "*.env.*", Effect::Ask),
                Rule::new("read", "*.env.example", Effect::Allow),
                Rule::new("external_directory", "*", Effect::Ask),
            ],
            session_rules: Vec::new(),
            allowed: Vec::new(),
            directory,
            real_directory: None,
            person_home: None,
            own_folders: Vec::new(),
            attachments: BTreeSet::new(),
            path_types: Default::default(),
            auto_without_sandbox: false,
        }
    }

    /// Capture file information at the resident's I/O boundary before
    /// evaluating a question about paths.
    pub fn observe_paths(&mut self, ask: &PermissionAsk) {
        self.real_directory = self.directory.canonicalize().ok();
        self.path_types = outside_paths(self, ask, |_| false)
            .into_iter()
            .filter_map(|path| Some((path.clone(), path.metadata().ok()?.is_dir())))
            .collect();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    Ask,
    Deny,
}

/// Evaluate the resident's preflight policy for the asking agent. An absent
/// decision lets that agent continue with its own policy.
pub fn check(
    policy: &Policy,
    ask: &PermissionAsk,
    plan: bool,
    owns: impl Fn(&Path) -> bool,
) -> Option<Decision> {
    let action = &ask.action;
    let command = is_shell(action);
    let analyzed = command.then(|| {
        ask.resources
            .iter()
            .map(|line| shell::split(line))
            .collect::<Vec<_>>()
    });
    let parts = match &analyzed {
        Some(commands) => commands
            .iter()
            .flat_map(|command| command.parts().iter())
            .map(String::as_str)
            .collect::<Vec<_>>(),
        None => ask.resources.iter().map(String::as_str).collect(),
    };
    let blocks = policy.blocks.as_deref().unwrap_or_default();
    if strictest(blocks.iter(), action, &parts) == Some(Effect::Deny) || (plan && changes(action)) {
        return Some(Decision::Deny);
    }
    let rules = policy.agent_rules.iter().chain(&policy.session_rules);
    let effect = strictest(rules.clone(), action, &parts);
    if effect == Some(Effect::Deny) {
        return Some(Decision::Deny);
    }
    let outside = outside_paths(policy, ask, owns);
    if !outside.is_empty() {
        let (folders, files) = outside_targets(policy, &outside, action);
        let folders = folders.iter().map(String::as_str).collect::<Vec<_>>();
        let mut refusable = files.iter().map(String::as_str).collect::<Vec<_>>();
        refusable.extend(folders.iter().copied());
        if strictest(blocks.iter(), "external_directory", &refusable) == Some(Effect::Deny)
            || strictest(rules.clone(), "external_directory", &refusable) == Some(Effect::Deny)
        {
            return Some(Decision::Deny);
        }
        let rules_ask = strictest(rules, "external_directory", &folders) == Some(Effect::Ask);
        if outside.iter().any(|file| policy_name(file).contains('\\'))
            || (rules_ask
                && !previously_allowed(
                    &policy.allowed,
                    "external_directory",
                    folders.iter().copied(),
                ))
        {
            return Some(Decision::Ask);
        }
    }
    if policy.blocks.is_none() && !looks(action) {
        return Some(Decision::Ask);
    }
    if analyzed
        .as_ref()
        .is_some_and(|runs| runs.iter().any(|run| !run.known))
        && blocks
            .iter()
            .any(|rule| rule.effect == Effect::Deny && is_shell(&rule.action))
    {
        return Some(Decision::Ask);
    }
    let policy_asks = effect == Some(Effect::Ask) || (command && policy.auto_without_sandbox);
    (policy_asks
        && !previously_allowed(
            &policy.allowed,
            action,
            ask.resources.iter().map(String::as_str),
        ))
    .then_some(Decision::Ask)
}

fn strictest<'a>(
    rules: impl DoubleEndedIterator<Item = &'a Rule> + Clone,
    action: &str,
    names: &[&str],
) -> Option<Effect> {
    let names = if names.is_empty() { &["*"] } else { names };
    names
        .iter()
        .filter_map(|name| {
            rules
                .clone()
                .rev()
                .find(|rule| {
                    matches_glob(&rule.action, action) && matches_glob(&rule.resource, name)
                })
                .map(|rule| rule.effect)
        })
        .max()
}

fn matches_glob(pattern: &str, name: &str) -> bool {
    if pattern == "*" || pattern == name || pattern.strip_suffix(" *") == Some(name) {
        return true;
    }
    let pattern = pattern.chars().collect::<Vec<_>>();
    let name = name.chars().collect::<Vec<_>>();
    let (mut pat, mut at) = (0, 0);
    let mut backtrack = None;
    while at < name.len() {
        match pattern.get(pat) {
            Some('*') => {
                backtrack = Some((pat + 1, at));
                pat += 1;
            }
            Some(ch) if *ch == '?' || *ch == name[at] => {
                pat += 1;
                at += 1;
            }
            _ => match &mut backtrack {
                Some((after, used)) => {
                    *used += 1;
                    at = *used;
                    pat = *after;
                }
                None => return false,
            },
        }
    }
    pattern[pat..].iter().all(|ch| *ch == '*')
}

fn previously_allowed<'a>(
    allowed: &[Allowance],
    action: &str,
    resources: impl Iterator<Item = &'a str> + Clone,
) -> bool {
    allowed.iter().any(|allowed| {
        (allowed.action == action || changes(&allowed.action) && changes(action))
            && resources
                .clone()
                .all(|name| covered(&allowed.patterns, action, name))
    })
}

fn covered(patterns: &[String], action: &str, name: &str) -> bool {
    if patterns.iter().any(|pattern| pattern == name) {
        return true;
    }
    let matches = |name: &str| patterns.iter().any(|pattern| matches_glob(pattern, name));
    if !is_shell(action) {
        return matches(name);
    }
    let found = shell::commands(name);
    found.known
        && if found.parts().is_empty() {
            matches(name.trim())
        } else {
            found.parts().iter().all(|command| {
                command == "cd"
                    || command
                        .strip_prefix("cd")
                        .is_some_and(|rest| rest.starts_with(char::is_whitespace))
                    || matches(command)
            })
        }
}

fn outside_paths(
    policy: &Policy,
    ask: &PermissionAsk,
    owns: impl Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    if !matches!(
        ask.action.as_str(),
        "read" | "edit" | "write" | "patch" | "list" | "grep" | "glob"
    ) {
        return Vec::new();
    }
    ask.resources
        .iter()
        .filter(|resource| !resource.is_empty() && *resource != "*")
        .filter_map(|resource| {
            let original = Path::new(resource);
            if let Some(relative) = resource.strip_prefix("~/") {
                return Some(policy.person_home.as_ref().map_or_else(
                    || original.to_path_buf(),
                    |home| normalized(&home.join(relative)),
                ));
            }
            if !original.is_absolute() && resource.contains(['*', '?', '[']) {
                return None;
            }
            Some(normalized(&policy.directory.join(original)))
        })
        .filter(|path| {
            !path.starts_with(&policy.directory)
                && !policy
                    .real_directory
                    .as_ref()
                    .is_some_and(|real| path.starts_with(real))
                && !policy
                    .own_folders
                    .iter()
                    .any(|folder| path.starts_with(folder))
                && !policy.attachments.contains(path)
                && !owns(path)
        })
        .collect()
}

fn outside_targets(
    policy: &Policy,
    outside: &[PathBuf],
    action: &str,
) -> (Vec<String>, Vec<String>) {
    let mut folders = Vec::new();
    let mut files = Vec::new();
    for path in outside {
        let directory = policy
            .path_types
            .get(path)
            .copied()
            .unwrap_or(matches!(action, "list" | "glob"));
        let parent = if directory {
            path.as_path()
        } else {
            path.parent().unwrap_or(path)
        };
        let pattern = policy_name(&parent.join("*"));
        if !folders.contains(&pattern) {
            folders.push(pattern);
        }
        if !directory {
            files.push(policy_name(path));
        }
    }
    (folders, files)
}

fn normalized(path: &Path) -> PathBuf {
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                clean.pop();
            }
            item => clean.push(item.as_os_str()),
        }
    }
    clean
}

fn policy_name(path: &Path) -> String {
    let name = path.to_string_lossy();
    if cfg!(windows) {
        name.replace('\\', "/")
    } else {
        name.into_owned()
    }
}

fn changes(action: &str) -> bool {
    matches!(action, "edit" | "write" | "patch")
}

fn looks(action: &str) -> bool {
    matches!(
        action,
        "read" | "glob" | "grep" | "list" | "websearch" | "todowrite" | "todoread" | "question"
    )
}

fn is_shell(action: &str) -> bool {
    matches!(action, "shell" | "bash")
}
