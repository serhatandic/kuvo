//! A conversation's or a bot's wake-ups and watches, as `kuvo-wake` keeps them in its folder: each
//! wake-up a file named for when it's due, its letters and how often it comes back; each watch a
//! folder of its own; `paused` while you or archiving or a move hold them.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;

/// The most wake-ups a conversation keeps waiting, as the Mac's.
pub const WAKE_LIMIT: usize = 50;
/// The most watches a conversation keeps, as `kuvo-wake watch` allows.
pub const WATCH_LIMIT: usize = 20;

/// Where a home keeps wake-ups: its bots' folders, and the folders of other conversations.
#[derive(Clone, Debug)]
pub struct Roots {
    pub bots: PathBuf,
    pub sessions: PathBuf,
    pub run: PathBuf,
    pub trash: PathBuf,
}

impl Roots {
    pub fn of(home: &Path) -> Self {
        Self {
            bots: home.join("bots"),
            sessions: home.join("wakes"),
            run: home.join("run"),
            trash: home.join("trash"),
        }
    }

    /// The folder keeping a conversation's wake-ups: its bot's, or its own; none for one that
    /// lives on another computer now, or that has neither.
    pub fn folder_of(&self, session: &str) -> Option<PathBuf> {
        if !folder_name(session) || self.away().contains(session) {
            return None;
        }
        for id in names(&self.bots) {
            let folder = self.bots.join(&id);
            if text(&folder.join("session")).trim() == session {
                return Some(folder);
            }
        }
        let own = self.sessions.join(session);
        own.join("session").exists().then_some(own)
    }

    /// The conversations that live on another computer for now.
    pub fn away(&self) -> BTreeSet<String> {
        kuvo_peers::moves::away_sessions(&self.run)
    }

    /// Every folder that may keep wake-ups: the bots' first, then the conversations'.
    pub fn folders(&self) -> impl Iterator<Item = (Kind, PathBuf)> + '_ {
        let bots = names(&self.bots)
            .into_iter()
            .map(|name| (Kind::Bot, self.bots.join(name)));
        let sessions = names(&self.sessions)
            .into_iter()
            .map(|name| (Kind::Session, self.sessions.join(name)));
        bots.chain(sessions)
    }
}

/// Whose a folder is: a bot's (in `bots`), or a conversation's own (in `wakes`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Bot,
    Session,
}

/// A conversation's id as a folder may be named for it.
pub fn folder_name(session: &str) -> bool {
    (1..=100).contains(&session.len())
        && session
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// A conversation's id the engine gives (`ses_…`).
pub fn session_id(session: &str) -> bool {
    session
        .strip_prefix("ses_")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// Letters that name a wake-up or watch for good.
pub fn letters(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// A folder's entries by name, sorted, dot-files left out; none when it isn't there.
pub fn names(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = kuvo_store::files::list_of(folder)
        .into_iter()
        .filter_map(|name| name.into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort();
    names
}

/// A file's text, empty when it can't be read.
pub fn text(file: &Path) -> String {
    kuvo_store::files::text_of(file).unwrap_or_default()
}

/// How often a wake-up comes back, as its name says (the app's `Wakes.Repeat`): 30m, 2h, 1d, 1w or
/// weekdays, within a year; none for anything else.
pub fn repeat_text(text: &str) -> Option<&str> {
    if text == "weekdays" {
        return Some(text);
    }
    let (count, unit) = text.split_at(text.len().checked_sub(1)?);
    if count.is_empty()
        || count.len() > 6
        || count.starts_with('0')
        || !count.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let count: u32 = count.parse().ok()?;
    let fits = match unit {
        "m" => (5..=525_600).contains(&count),
        "h" => count <= 8_760,
        "d" => count <= 366,
        "w" => count <= 52,
        _ => false,
    };
    fits.then_some(text)
}

/// A wake-up as its file's name has it: when it's due (seconds since 1970), the letters that name
/// it for good, and how often it comes back as the name keeps it (`1d@0900`, `1w@Mon0900`).
#[derive(Clone, Debug, PartialEq)]
pub struct WakeName {
    pub name: String,
    pub due: f64,
    pub letters: String,
    pub repeat: Option<String>,
}

impl WakeName {
    pub fn parse(name: &str) -> Option<Self> {
        let mut parts = name.split('-');
        let due = parts.next()?;
        let (whole, fraction) = due.split_once('.').unwrap_or((due, "0"));
        if whole.is_empty()
            || fraction.is_empty()
            || !whole
                .bytes()
                .chain(fraction.bytes())
                .all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let letters = parts.next().unwrap_or_default().to_owned();
        let repeat = parts.collect::<Vec<_>>().join("-");
        Some(Self {
            name: name.to_owned(),
            due: due.parse().ok()?,
            letters,
            repeat: (!repeat.is_empty()).then_some(repeat),
        })
    }

    /// How often it comes back, as a client is told (none for a repeat it can't read).
    pub fn every(&self) -> Option<&str> {
        let repeat = self.repeat.as_deref()?;
        repeat_text(repeat.split('@').next().unwrap_or_default())
    }

    /// Whole seconds it's due at, as `kuvo-wake fire` reads a name (none for a fraction).
    pub fn due_seconds(&self) -> Option<i64> {
        let due = self.name.split('-').next()?;
        due.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| due.parse().ok())
            .flatten()
    }
}

/// A wake-up as a client takes it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Wake {
    pub id: String,
    pub due: f64,
    pub note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub every: Option<String>,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel_key: Option<String>,
    #[serde(skip)]
    pub letters: String,
}

/// A watch as a client takes it.
#[derive(Clone, Debug, Serialize)]
pub struct Watch {
    pub letters: String,
    pub every: String,
    pub check: String,
    pub note: String,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<u64>,
}

/// A folder's wake-ups (the soonest first), its watches and whether they're held.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Listing {
    pub wakes: Vec<Wake>,
    pub watches: Vec<Watch>,
    pub paused: bool,
}

/// Notes and checks on a line each (as lists show them), or whole.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Detail {
    Line,
    Whole,
}

fn shown(text: &str, limit: usize, detail: Detail) -> String {
    match detail {
        Detail::Whole => text.to_owned(),
        Detail::Line => one_line(text, limit),
    }
}

/// Text on one line, at most `limit` characters (tabs and line ends as spaces).
pub fn one_line(text: &str, limit: usize) -> String {
    text.chars()
        .take(limit)
        .map(|ch| {
            if matches!(ch, '\t' | '\r' | '\n') {
                ' '
            } else {
                ch
            }
        })
        .collect()
}

/// A folder's wake-ups, watches and pause.
pub fn read(folder: &Path, detail: Detail) -> Listing {
    let mut wakes: Vec<Wake> = names(&folder.join("wakes"))
        .into_iter()
        .filter_map(|name| {
            let parsed = WakeName::parse(&name)?;
            let note = text(&folder.join("wakes").join(&name));
            let cancel_key = letters(&parsed.letters).then(|| parsed.letters.clone());
            let given = cancel_key
                .as_ref()
                .and_then(|key| kuvo_store::files::text_of(&folder.join("summaries").join(key)));
            Some(Wake {
                id: name,
                due: parsed.due,
                summary: wake_summary(&note, given.as_deref()),
                note: shown(&note, 400, detail),
                every: parsed.every().map(str::to_owned),
                cancel_key,
                letters: parsed.letters,
            })
        })
        .collect();
    wakes.sort_by(|a, b| a.due.total_cmp(&b.due));
    let watches = names(&folder.join("watches"))
        .into_iter()
        .filter_map(|letters| {
            let at = folder.join("watches").join(&letters);
            let check = text(&at.join("check"));
            if check.is_empty() {
                return None;
            }
            let note = text(&at.join("note"));
            let next = text(&at.join("next"));
            let next = next.trim();
            let next = (!next.is_empty() && next.bytes().all(|b| b.is_ascii_digit()))
                .then(|| next.parse::<u64>().ok())
                .flatten()
                .filter(|due| *due > 0);
            Some(Watch {
                every: text(&at.join("every")).trim().to_owned(),
                check: shown(&check, 300, detail),
                summary: wake_summary(
                    &note,
                    kuvo_store::files::text_of(&at.join("summary")).as_deref(),
                ),
                note: shown(&note, 400, detail),
                next,
                letters,
            })
        })
        .collect();
    Listing {
        wakes,
        watches,
        paused: folder.join("paused").exists(),
    }
}

/// The first sentence of a note's first line, as a wake-up's card shows one without a summary.
fn first_sentence(note: &str) -> String {
    let line = note
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default();
    let plain = line.replace(['*', '`'], "");
    let plain = plain
        .trim_start_matches(|ch: char| ch.is_whitespace() || matches!(ch, '#' | '>' | '-'))
        .trim();
    let mut chars = plain.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if matches!(ch, '.' | '!' | '?' | '…')
            && chars.peek().is_none_or(|(_, next)| next.is_whitespace())
        {
            return plain[..index + ch.len_utf8()].to_owned();
        }
    }
    plain.to_owned()
}

/// A wake-up's or watch's summary: the one its agent gave it, or its note's first sentence, at
/// most 300 characters (as JavaScript counts them).
pub fn wake_summary(note: &str, given: Option<&str>) -> String {
    let given = given
        .map(|given| given.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|given| !given.is_empty());
    let summary = given
        .or_else(|| Some(first_sentence(note)).filter(|first| !first.is_empty()))
        .unwrap_or_else(|| "Wake-up".to_owned());
    let units: Vec<u16> = summary.encode_utf16().collect();
    if units.len() > 300 {
        format!("{}…", String::from_utf16_lossy(&units[..299]).trim_end())
    } else {
        summary
    }
}

/// Whether a folder's wake-ups are held while it moves to another computer.
pub fn moving(folder: &Path) -> bool {
    let marker = text(&folder.join("paused"));
    let marker = marker.trim();
    marker == "moving"
        || marker.strip_prefix("moving:mov_").is_some_and(|id| {
            (1..=40).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric())
        })
}

/// What's in `paused`, when it's there.
pub fn hold(folder: &Path) -> Option<String> {
    fs::read(folder.join("paused"))
        .ok()
        .map(|raw| String::from_utf8_lossy(&raw).into_owned())
}

/// A conversation archived (or brought back): its own wake-ups wait while it's archived and come
/// back with it. A hold of your own stays; a move's becomes archiving's.
pub fn hold_archived(sessions: &Path, session: &str, archived: bool) {
    if !session_id(session) {
        return;
    }
    let folder = sessions.join(session);
    let file = folder.join("paused");
    let held = hold(&folder);
    if archived {
        if folder.exists() && held.as_deref().is_none_or(|held| held == "moving") {
            let _ = std::fs::write(&file, "archived");
        }
    } else if matches!(held.as_deref(), Some("archived" | "")) {
        let _ = std::fs::remove_file(&file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_repeats_read_as_the_app_reads_them() {
        let wake = WakeName::parse("1800000000-Wk1-1d@0900").unwrap();
        assert_eq!(wake.due, 1_800_000_000.0);
        assert_eq!(wake.letters, "Wk1");
        assert_eq!(wake.every(), Some("1d"));
        assert_eq!(WakeName::parse("1800000000-Wk2").unwrap().every(), None);
        assert_eq!(WakeName::parse("17.5-ab").unwrap().due, 17.5);
        assert!(WakeName::parse("soon-ab").is_none());
        assert!(WakeName::parse(".5-ab").is_none());
        for good in ["5m", "525600m", "8760h", "366d", "52w", "weekdays"] {
            assert_eq!(repeat_text(good), Some(good));
        }
        for bad in ["4m", "08h", "1y", "8761h", "53w", "1234567m", "", "m"] {
            assert_eq!(repeat_text(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_summary_is_the_agents_or_the_first_sentence() {
        assert_eq!(wake_summary("Check it. Then more", None), "Check it.");
        assert_eq!(
            wake_summary("\n## **Look** at v1.2 now", None),
            "Look at v1.2 now"
        );
        assert_eq!(wake_summary("", None), "Wake-up");
        assert_eq!(wake_summary("Note", Some("  A   line \n")), "A line");
        let long = "x".repeat(400);
        assert_eq!(wake_summary(&long, None).chars().count(), 300);
    }
}
