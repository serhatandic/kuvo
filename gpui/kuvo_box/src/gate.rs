//! The update gate (PROTOCOL.md, "The update gate", #679): capabilities, not versions. Every
//! computer is used for what it says it has. [`REQUIRED`] names what this client draws (the same
//! list as KuvoClient's `BoxGate.required`): a box lacking some of it is only *behind*, used as it
//! is, each feature drawn where it has it and drawn without where it hasn't, and offered its
//! update. A box is *held* only where this client can't speak to it at all: lacking the
//! [`BASELINE`] (or answering no `kuvo/info`). A held box is sent nothing but what tells or brings
//! about its update, what keeps work already under way there going, and your computers' list
//! (which never depends on one computer's version).

mod required;
pub use required::REQUIRED;

/// What this client can't speak to a box without at all. A box lacking one of these is held;
/// every other name in [`REQUIRED`] is a feature, drawn where a box has it and drawn without where
/// it hasn't. A name goes here when a request this client sends *means something else* to a box
/// without it, never for what's only drawn:
/// - `sessions.list`, `messages.send`: its session list, and a message sent as fields (a box with
///   `messages.send` has what came before it too, PROTOCOL.md);
/// - `messages.commands`: a command's message, read as typed, by its id;
/// - `sessions.start.empty`: a session started by the box, never by the client itself;
/// - `clients.kind`: the desktop app says what it is (without it, its messages are taken as a phone's);
/// - `queue.stop.arriving`: Stop holds what's still on its way (without it, a queued message still
///   reaches the agent after Stop).
pub const BASELINE: &[&str] = &["clients.kind", "messages.commands", "messages.send", "queue.stop.arriving", "sessions.list", "sessions.start.empty"];

/// What `features` lack of [`REQUIRED`], in its order: none when the box is up to date.
pub fn missing<S: AsRef<str>>(features: &[S]) -> Vec<String> {
    REQUIRED
        .iter()
        .filter(|name| !features.iter().any(|has| has.as_ref() == **name))
        .map(|name| (*name).to_owned())
        .collect()
}

/// What `features` lack of [`BASELINE`]: none when this client can speak to the box.
pub fn missing_baseline<S: AsRef<str>>(features: &[S]) -> Vec<String> {
    BASELINE.iter().filter(|name| !features.iter().any(|has| has.as_ref() == **name)).map(|name| (*name).to_owned()).collect()
}

/// Whether a held box is still asked `path`: what it is (`kuvo/info`), its own update
/// (`kuvo/update`, `kuvo/box/update`), its Mac's app registering with it (`kuvo/box/app`) and the
/// keys it holds for that app (`kuvo/box/proof`, as KuvoClient's gate), what's at work there
/// (`kuvo/box/at-work`, as the menu bar asks), your computers' list
/// (`kuvo/computers`, followed at `kuvo/computers/events`, and this app's pairing through it,
/// `kuvo/devices/pair`: every other computer stays yours whatever this one's version), and what keeps work under way going: its engine (`api/…`), its
/// coding agents' bridge (`kuvo/agents/…`), its agents' tool calls (`kuvo/agent-tools`), its asks
/// and Auto's decisions.
pub fn is_allowed(path: &str) -> bool {
    let route = path.split('?').next().unwrap_or(path);
    if ["kuvo/info", "kuvo/update", "kuvo/box/update", "kuvo/box/app", "kuvo/box/proof", "kuvo/box/at-work", "kuvo/agent-tools", "kuvo/computers", "kuvo/computers/events", "kuvo/devices/pair"].contains(&route) {
        return true;
    }
    if route.starts_with("api/") || route.starts_with("kuvo/agents/") {
        return true;
    }
    for asks in ["kuvo/asks", "kuvo/sandbox/asks", "kuvo/vault/asks", "kuvo/work/asks"] {
        if route == asks || route.strip_prefix(asks).is_some_and(|rest| rest.starts_with('/')) {
            return true;
        }
    }
    let parts: Vec<&str> = route.split('/').filter(|part| !part.is_empty()).collect();
    // A conversation's decisions, and its asks (`…/asks`, `…/asks/events`, `…/asks/<ask>`).
    let session = parts.len() >= 4 && parts[0] == "kuvo" && parts[1] == "sessions";
    (session && parts.len() == 4 && parts[3] == "decisions") || (session && parts[3] == "asks" && parts.len() <= 5)
}

/// Where a box stands with this client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// Not asked yet: the first request reads `kuvo/info` before anything else.
    Unknown,
    /// It has everything this client draws.
    Open,
    /// It's older than this client, lacking these features: used as it is (what it lacks isn't
    /// drawn), and offered its update.
    Behind { missing: Vec<String> },
    /// This client can't speak to it at all (it lacks the [`BASELINE`]; `missing`: all it lacks):
    /// it waits for its update.
    Held { missing: Vec<String> },
}

impl Gate {
    /// The gate for a box whose `kuvo/info` names `features` (none: a box from before it said any,
    /// or one that answers no `kuvo/info` at all, lacks them all).
    pub fn for_features<S: AsRef<str>>(features: Option<&[S]>) -> Gate {
        let Some(features) = features else {
            return Gate::Held { missing: REQUIRED.iter().map(|name| (*name).to_owned()).collect() };
        };
        let missing = missing(features);
        if missing.is_empty() {
            Gate::Open
        } else if missing_baseline(features).is_empty() {
            Gate::Behind { missing }
        } else {
            Gate::Held { missing }
        }
    }

    /// Whether the box has `feature` (one of [`REQUIRED`]): drawn where it has, drawn without where
    /// it hasn't; nothing while it's unknown or held.
    pub fn has(&self, feature: &str) -> bool {
        match self {
            Gate::Open => true,
            Gate::Behind { missing } => !missing.iter().any(|name| name == feature),
            Gate::Unknown | Gate::Held { .. } => false,
        }
    }

    /// This client can't speak to it: only its update, and what keeps work there going.
    pub fn is_held(&self) -> bool {
        matches!(self, Gate::Held { .. })
    }

    /// It's older than this client (held, or only behind): its update is offered.
    pub fn is_older(&self) -> bool {
        matches!(self, Gate::Held { .. } | Gate::Behind { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_but(left_out: &[&str]) -> Vec<String> {
        REQUIRED.iter().filter(|name| !left_out.contains(name)).map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn a_box_lacking_a_feature_is_behind_and_used_one_lacking_the_baseline_is_held() {
        // (beta.30's bot states, on a box from before them: drawn without, never held.)
        let behind = Gate::for_features(Some(&all_but(&["bots.state", "settings.vault"])));
        assert_eq!(behind, Gate::Behind { missing: vec!["bots.state".into(), "settings.vault".into()] });
        assert!(!behind.is_held() && behind.is_older());
        assert!(Gate::for_features(Some(&all_but(&["messages.send"]))).is_held());
        // (Without it the desktop app's messages would be taken as a phone's: never sent so.)
        assert!(Gate::for_features(Some(&all_but(&["clients.kind"]))).is_held());
        assert!(Gate::for_features::<String>(None).is_held());
        assert_eq!(Gate::for_features(Some(&all_but(&[]))), Gate::Open);
    }

    #[test]
    fn a_held_box_still_lists_your_computers_and_pairs_this_app() {
        for route in ["kuvo/computers", "kuvo/computers/events", "kuvo/devices/pair", "kuvo/box/at-work", "kuvo/update"] {
            assert!(is_allowed(route), "{route}");
        }
        assert!(!is_allowed("kuvo/bots"));
    }
}
