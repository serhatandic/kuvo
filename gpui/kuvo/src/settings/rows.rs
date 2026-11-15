//! What Settings lists itself, and the rows of the pages this app still words: your own settings
//! (this app's, on this device), and the computer's sections its box doesn't describe yet
//! (Accounts, Models, Usage). A section the box describes is drawn from its description alone
//! (`described.rs`). Drawing is in `page.rs`; nothing here reaches the box.

use kuvo_box::accounts::{AccountEntry, Accounts, Action, Confirm, Side, State};
use kuvo_box::areas::{Account, Level};
use kuvo_box::models::Offered as OfferedModels;
use kuvo_box::settings::{Config, UsageSettings};
use kuvo_box::settings_pages::{Confirm as Asks, Row as Described};
use kuvo_ui::IconName;
use serde_json::Value;

use crate::areas::Reading;

/// One of Settings' sections this app lists itself.
pub struct Listed {
    pub id: &'static str,
    pub title: &'static str,
    pub icon: IconName,
    /// The heading it's under in a computer's sidebar.
    pub group: &'static str,
    /// What search finds it by besides its name.
    pub keywords: &'static str,
}

const fn listed(id: &'static str, title: &'static str, icon: IconName, group: &'static str, keywords: &'static str) -> Listed {
    Listed { id, title, icon, group, keywords }
}

/// Your settings: this app's own, on this device, kept and drawn by the app.
pub const YOURS: &[Listed] = &[listed("appearance", "Appearance", IconName::Sun, "", "theme light dark colours colors")];

/// Yours that the desktop app hasn't yet: each opens a page saying so.
pub const YOURS_LATER: &[Listed] = &[
    listed("general", "General", IconName::Settings, "", "window swipe open files"),
    listed("notifications", "Notifications", IconName::Bell, "", "sound badge finished waits"),
    listed("voice", "Voice", IconName::Mic, "", "microphone dictation speech"),
    listed("shortcuts", "Keyboard Shortcuts", IconName::Return, "", "keys"),
    listed("panel", "Side Panel", IconName::PanelRight, "", "browser history terminal files"),
    listed("updates", "Updates", IconName::ArrowDown, "", "version beta"),
    listed("privacy", "Privacy", IconName::Info, "", "crash reports"),
];

/// A computer's sections the desktop app still draws itself, from routes it knows one by one.
// BOX-NEEDED: move onto kuvo/settings (#741): Accounts, Models and Usage, each described by the
// box as Permissions and Browser are, so this list and their pages below go.
pub const OWN: &[Listed] = &[
    listed("accounts", "Accounts", IconName::User, "Models", "sign in plan provider api key coding agents"),
    listed("models", "Models", IconName::Sparkle, "Models", "default model subagents titles"),
    listed("usage", "Usage", IconName::Clock, "Models", "limits notices almost out notifications"),
];

/// A computer's sections that are in neither its box's description nor this app yet: each opens
/// a page saying so. One the box describes (by the same name) is drawn from the box instead.
// BOX-NEEDED: move onto kuvo/settings (#741): each of these.
pub const LATER: &[Listed] = &[
    listed("custom-auto", "Custom Auto", IconName::Sliders, "Models", ""),
    listed("agents", "Agents", IconName::Bot, "Agents & Tools", ""),
    listed("commands", "Commands", IconName::Terminal, "Agents & Tools", ""),
    listed("skills", "Skills", IconName::Pencil, "Agents & Tools", ""),
    listed("integrations", "Integrations", IconName::Link, "Agents & Tools", ""),
    listed("vault", "Vault", IconName::Shield, "Agents & Tools", "secrets"),
    listed("mcp", "MCP Servers", IconName::Sliders, "Agents & Tools", ""),
    listed("git", "Git", IconName::Branch, "Projects", ""),
    listed("worktrees", "Worktrees", IconName::Folder, "Projects", ""),
    listed("archived", "Archived Sessions", IconName::Archive, "System", ""),
    listed("engine", "Engine", IconName::Computer, "System", ""),
    listed("storage", "Storage", IconName::Drive, "System", ""),
    listed("phone", "Phone", IconName::Message, "System", "pair iphone"),
];

/// The icon a box names for a section (PROTOCOL.md: a set every client has); one this app
/// hasn't, its own.
pub fn icon_named(name: &str) -> IconName {
    match name {
        "settings" => IconName::Settings,
        "drive" => IconName::Drive,
        "archive" => IconName::Archive,
        "shield" => IconName::Shield,
        "globe" => IconName::Globe,
        "user" => IconName::User,
        "clock" => IconName::Clock,
        "sparkle" => IconName::Sparkle,
        "bot" => IconName::Bot,
        "terminal" => IconName::Terminal,
        "link" => IconName::Link,
        "computer" => IconName::Computer,
        "branch" => IconName::Branch,
        "folder" => IconName::Folder,
        "bell" => IconName::Bell,
        "file" => IconName::File,
        "message" => IconName::Message,
        "pencil" => IconName::Pencil,
        "mic" => IconName::Mic,
        _ => IconName::Sliders,
    }
}

/// What a change asks of the box (or, for the theme, of this window).
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    SignIn { side: Side, account: String, name: String, sheet_title: String },
    Install { account: String, name: String },
    /// Disconnect, asked first in the box's words where it says to.
    Disconnect { account: String, confirm: Option<Confirm> },
    SignOut { account: String },
    /// An added account removed, asked first in the box's words where it says to.
    RemoveAgent { account: String, confirm: Option<Confirm> },
    /// A model setting at a path in the engine's settings, from what was read there.
    SetModel { path: &'static [&'static str], model: Option<String>, was: Option<String> },
    SetNotice(Notice, bool),
    SetDark(bool),
    /// A row the box describes, changed to `value` from the revision it was read at (`asked`:
    /// its confirm, where it has one, was answered).
    Change { at: At, value: Value, asked: bool },
    /// What a described row offers (`action`: its id), done by the box, asked first where the
    /// box says to.
    Do { at: At, action: String, confirm: Option<Asks>, asked: bool },
    /// A field's text, saved as its row's value (`number`: read as one first).
    Save { at: At, number: bool },
    /// A secret's field shown, to replace the one set (or put away again).
    Filter,
    Folder { at: At, words: kuvo_box::settings_pages::Folder },
    Replacing { row: String, on: bool },
    /// Opens the address the box named: a web page, or (from its own computer's box alone) what
    /// only an app on that computer can open.
    Open(String),
    /// Opens another section of the computer shown.
    Section(String),
    /// Goes to a place in Settings (`mod.rs`, its keys): another section, or the other side.
    Go(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    AlmostOut,
    CuttingItClose,
    WillRunOut,
}

/// Where a change to a row the box describes goes: its section, its id, the revision it was read
/// at, and what's asked first. (Only these: a row's choices are never copied into each choice.)
#[derive(Clone, Debug, PartialEq)]
pub struct At {
    pub section: String,
    pub row: String,
    pub revision: String,
    pub title: String,
    pub confirm: Option<Asks>,
    pub confirm_below: Option<kuvo_box::settings_pages::ConfirmBelow>,
    pub unit: Option<kuvo_box::settings_pages::Unit>,
    pub original: Option<Value>,
    pub multiline: bool,
}

impl At {
    pub fn asks_for(&self, value: &Value) -> Option<Asks> {
        self.confirm_below.as_ref().filter(|below| value.as_f64().is_some_and(|number| number < below.value)).map(|below| below.confirm.clone()).or_else(|| self.confirm.clone())
    }

    pub fn below(&self, value: &Value) -> bool {
        self.confirm_below.as_ref().is_some_and(|below| value.as_f64().is_some_and(|value| value < below.value))
    }

    pub fn field_value(&self, text: &str, number: bool) -> Option<Value> {
        if !number { return Some(Value::String(if self.multiline { text.to_owned() } else { text.trim().to_owned() })); }
        if let (Some(unit), Some(original)) = (&self.unit, &self.original)
            && original.as_f64().is_some_and(|value| unit.number(value) == text.trim())
        {
            return Some(original.clone());
        }
        let number = text.trim().parse::<f64>().ok()?;
        serde_json::Number::from_f64(self.unit.as_ref().map_or(number, |unit| (number * unit.factor).round())).map(Value::Number)
    }

    pub fn of(section: &str, row: &Described) -> At {
        At { section: section.to_owned(), row: row.id.clone(), revision: row.revision.clone(), title: row.title.clone(), confirm: row.confirm.clone(), confirm_below: row.confirm_below.clone(), unit: row.unit.clone(), original: row.value.clone(), multiline: row.lines.is_some() }
    }
}

/// A button in a row.
#[derive(Clone, Debug, PartialEq)]
pub struct Push {
    pub label: String,
    /// None: drawn, and not pressed (its row says why).
    pub act: Option<Act>,
}

impl Push {
    pub fn new(label: impl Into<String>, act: Act) -> Push {
        Push { label: label.into(), act: Some(act) }
    }
}

/// One of a meter's windows, as drawn: its name, what's said of it, its share (0 to 1), how
/// close it is, and when it starts over.
#[derive(Clone, Debug, PartialEq)]
pub struct Meter {
    pub title: String,
    pub said: String,
    pub share: f32,
    pub level: Level,
    pub resets_at: Option<f64>,
}

/// A row's control, on its right.
#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    None,
    /// Plain words (a state that's read, not set here).
    Text(String),
    /// A switch, and what flipping it does (none: it can't be flipped here, or not until another
    /// change in its group is answered).
    Switch { on: bool, act: Option<Act> },
    /// A menu: what it shows now (none: a menu of things to do, under "…"), and its choices
    /// (words, a quieter line, what picking one does, or none where it can't be picked from here,
    /// and whether it's it).
    Menu { shown: Option<String>, choices: Vec<(String, Option<String>, Option<Act>, bool)> },
    /// Buttons, in order.
    Buttons(Vec<Push>),
    /// A field (kept by the view under the row's key), and the buttons beside it.
    Field { unit: Option<String>, lines: Option<(usize, usize)>, masked: bool, value: String, placeholder: String, revision: String, save: Act, more: Vec<Push> },
    /// It opens something: a section, a page.
    Opens(Act),
    /// A change on its way.
    Busy(String),
    /// Windows of use, each with its share and when it starts over.
    Windows(Vec<Meter>),
}

/// How a row's line reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Quiet,
    Warning,
}

/// What a row shows before its name.
#[derive(Clone, Debug, PartialEq)]
pub enum Leading {
    None,
    /// A service's mark, by the box's service id.
    Mark(String),
    /// The name's first letter, in a round tile.
    Initial(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// Its debug selector, unique on the page.
    pub key: String,
    pub leading: Leading,
    pub title: String,
    pub detail: String,
    pub tone: Tone,
    /// Why it can't be changed from here, in the box's words.
    pub why: Option<String>,
    pub control: Control,
}

impl Row {
    pub fn new(key: impl Into<String>, title: impl Into<String>, detail: impl Into<String>, control: Control) -> Row {
        Row { key: key.into(), leading: Leading::None, title: title.into(), detail: detail.into(), tone: Tone::Quiet, why: None, control }
    }

    fn leading(mut self, leading: Leading) -> Row {
        self.leading = leading;
        self
    }

    pub fn warning(mut self, warning: bool) -> Row {
        if warning {
            self.tone = Tone::Warning;
        }
        self
    }

    /// Whether `query` (lowercase) is in its name or its line.
    pub fn matches(&self, query: &str) -> bool {
        self.title.to_lowercase().contains(query) || self.detail.to_lowercase().contains(query)
    }

    pub fn why(mut self, why: Option<String>) -> Row {
        self.why = why;
        self
    }
}

/// A page's piece: a group of rows in one card, or a line of its own.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    /// A quiet label over a card (with words under it, where there are any), and the card's rows.
    Group(String, String, Vec<Row>),
    /// A quiet line on the page: loading, a reason, what isn't here yet.
    Note { key: String, words: String, busy: bool, warning: bool },
}

pub fn note(key: &str, words: impl Into<String>, busy: bool) -> Block {
    Block::Note { key: key.into(), words: words.into(), busy, warning: false }
}

/// Everything one of this app's own pages is drawn from, as read from the box.
pub struct Facts<'a> {
    pub accounts: &'a Reading<Accounts>,
    pub limits: &'a Reading<Vec<Account>>,
    pub config: &'a Reading<Config>,
    pub offered: &'a Reading<OfferedModels>,
    pub usage: &'a Reading<UsageSettings>,
    /// Rows with a change on its way, by key, and what they say meanwhile.
    pub busy: &'a [(String, String)],
    /// The computer's name.
    pub this_box: &'a str,
    /// Whether the box is on the computer this app runs on.
    pub here: bool,
}

impl Facts<'_> {
    fn busy(&self, key: &str) -> Option<Control> {
        self.busy.iter().find(|(busy, _)| busy == key).map(|(_, words)| Control::Busy(words.clone()))
    }

    /// Whether a change to a row whose key starts with `group` is under way.
    fn group_busy(&self, group: &str) -> bool {
        self.busy.iter().any(|(busy, _)| busy.starts_with(group))
    }
}

/// A reading's state as a note: loading, or why it couldn't be read.
fn unread<T>(reading: &Reading<T>, key: &str, what: &str) -> Option<Block> {
    match reading {
        Reading::NotYet => Some(note(key, format!("Reading {what}…"), true)),
        Reading::Failed(why) => Some(note(key, format!("{} couldn’t be read: {why}", capitalized(what)), false)),
        Reading::Ready(_) => None,
    }
}

fn capitalized(words: &str) -> String {
    let mut chars = words.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// One of your own settings' pages, as blocks.
pub fn yours(section: &str, dark: bool) -> Vec<Block> {
    match section {
        "appearance" => {
            let theme = Row::new(
                "row-theme",
                "Theme",
                "Light or dark colours. ⌘⇧L switches them too.",
                Control::Menu { shown: Some(if dark { "Dark".into() } else { "Light".into() }), choices: vec![("Light".into(), None, Some(Act::SetDark(false)), !dark), ("Dark".into(), None, Some(Act::SetDark(true)), dark)] },
            );
            vec![Block::Group("Theme".into(), String::new(), vec![theme])]
        }
        _ => Vec::new(),
    }
}

/// One of a computer's pages this app still draws itself, as blocks.
pub fn page(section: &str, facts: &Facts) -> Vec<Block> {
    match section {
        "accounts" => accounts(facts),
        "usage" => usage(facts),
        "models" => models(facts),
        _ => Vec::new(),
    }
}

// MARK: Accounts

/// What one of the box's actions does here, where this app has a control for it; none for one it
/// hasn't yet (a program's update, renaming an account, another account beside the one in use).
pub(super) fn act_of(entry: &AccountEntry, action: &Action) -> Option<Act> {
    let (account, name) = (entry.integration.id.clone(), entry.integration.name.clone());
    Some(match action.id.as_str() {
        "signIn" | "signInAgain" => Act::SignIn { side: entry.side, account, name, sheet_title: action.sheet_title.clone()? },
        "install" => Act::Install { account, name },
        "disconnect" => Act::Disconnect { account, confirm: action.confirm.clone() },
        "signOut" => Act::SignOut { account },
        "remove" => Act::RemoveAgent { account, confirm: action.confirm.clone() },
        _ => return None,
    })
}

/// An account's row, all of it as the box says: its line, a warning where its state needs you,
/// and its actions with the box's words. Only the layout is this app's: signed in, they're in a
/// menu (but a lone one that asks first, which stays a button); otherwise buttons, in the box's
/// order. An action that only works on the box's computer is drawn only there.
fn account_row(entry: &AccountEntry, facts: &Facts) -> Row {
    let name = entry.integration.name.clone();
    let key = format!("account-{}", entry.integration.id);
    let control = facts.busy(&key).unwrap_or_else(|| {
        let offered: Vec<(&Action, Act)> = entry.actions.iter().filter(|action| action.offered(facts.here)).filter_map(|action| Some((action, act_of(entry, action)?))).collect();
        match offered.as_slice() {
            [] => Control::None,
            [(action, _)] if entry.state == State::SignedIn && action.confirm.is_none() => menu(&offered),
            [_, _, ..] if entry.state == State::SignedIn => menu(&offered),
            _ => Control::Buttons(offered.into_iter().map(|(action, act)| Push::new(action.label.clone(), act)).collect()),
        }
    });
    Row::new(key, name.clone(), entry.line.clone(), control).leading(mark(entry, name)).warning(entry.state.needs_you())
}

/// Actions in a menu: one that opens something first (the box's confirm, or this app's sign-in
/// sheet) ends in an ellipsis, as menus mark them. The mark is drawn here, never read in a label.
fn menu(offered: &[(&Action, Act)]) -> Control {
    let choices = offered.iter().map(|(action, act)| (if action.confirm.is_some() || action.signs_in() { format!("{}…", action.label) } else { action.label.clone() }, None, Some(act.clone()), false)).collect();
    Control::Menu { shown: None, choices }
}

/// An account's mark: the box's `service`; its name's first letter where the box names none.
fn mark(entry: &AccountEntry, name: String) -> Leading {
    entry.service.clone().map_or(Leading::Initial(name), Leading::Mark)
}

fn accounts(facts: &Facts) -> Vec<Block> {
    if let Some(note) = unread(facts.accounts, "note-accounts", "accounts") {
        return vec![note];
    }
    let Reading::Ready(accounts) = facts.accounts else { return Vec::new() };
    let mut blocks = Vec::new();
    if let Some(why) = &accounts.agents_down {
        blocks.push(note("note-agents-down", format!("The coding agents aren’t running on {}: {why}", facts.this_box), false));
    } else if !accounts.agents.is_empty() {
        blocks.push(Block::Group("Coding Agents".into(), String::new(), accounts.agents.iter().map(|entry| account_row(entry, facts)).collect()));
    }
    if let Some(why) = &accounts.engine_down {
        blocks.push(note("note-engine-down", format!("The engine isn’t running on {}: {why}", facts.this_box), false));
    }
    // By the box's state: those signed in (or once signed in, now ended), then the rest to add,
    // each in the engine's order.
    let (connected, others): (Vec<&AccountEntry>, Vec<&AccountEntry>) = accounts.engine.iter().partition(|entry| entry.state != State::SignedOut);
    if !connected.is_empty() {
        blocks.push(Block::Group("Providers".into(), String::new(), connected.into_iter().map(|entry| account_row(entry, facts)).collect()));
    }
    if !others.is_empty() {
        blocks.push(Block::Group("Add a Provider".into(), String::new(), others.into_iter().map(|entry| account_row(entry, facts)).collect()));
    }
    blocks
}

// MARK: Usage

/// An account's windows as meters: its share, its colour by the box's level, and when it starts
/// over; one that started over and wasn't read since says so (PROTOCOL.md).
fn meters(account: &Account) -> Vec<Meter> {
    account
        .windows
        .iter()
        .map(|window| Meter {
            title: window.title.clone(),
            said: if window.expired { "Not read since it started over".to_owned() } else { format!("{}% used", (window.used * 100.).round() as u32) },
            share: if window.expired { 0. } else { window.used.clamp(0., 1.) },
            level: window.level,
            resets_at: window.resets_at.filter(|_| !window.expired),
        })
        .collect()
}

fn usage(facts: &Facts) -> Vec<Block> {
    let mut blocks = Vec::new();
    match facts.limits {
        Reading::Ready(limits) if limits.is_empty() => blocks.push(note("note-usage-none", "No account’s limits to show yet. Sign in under Accounts, and each plan’s limits show here.", false)),
        Reading::Ready(limits) => {
            let rows = limits
                .iter()
                .enumerate()
                .map(|(ix, account)| {
                    let detail = account.plan.clone().map(|plan| format!("{plan} plan")).unwrap_or_default();
                    Row::new(format!("usage-{ix}"), account.title.clone(), detail, Control::Windows(meters(account))).leading(Leading::Mark(account.service.clone()))
                })
                .collect();
            blocks.push(Block::Group("Limits".into(), String::new(), rows));
        }
        reading => blocks.extend(unread(reading, "note-usage", "your accounts’ limits")),
    }
    // The notices this computer's box sends about them.
    match facts.usage {
        Reading::Ready(usage) => {
            let notices = usage.notices;
            let row = |key: &str, title: &str, detail: &str, on: bool, act: fn(bool) -> Act| {
                // (Each is sent whole, from what the box last said: two at once would undo one.)
                Row::new(key, title, detail, facts.busy(key).unwrap_or(Control::Switch { on, act: (!facts.group_busy("notice-")).then(|| act(!on)) }))
            };
            let rows = vec![
                row("notice-almost-out", "Almost Out", "When a limit drops below 10% left.", notices.under_ten_percent, |on| Act::SetNotice(Notice::AlmostOut, on)),
                row("notice-close", "Cutting It Close", "When a limit is on pace to end with little left.", notices.healthy_to_close, |on| Act::SetNotice(Notice::CuttingItClose, on)),
                row("notice-run-out", "Will Run Out", "When a limit is on pace to run out before it starts over.", notices.close_to_running_out, |on| Act::SetNotice(Notice::WillRunOut, on)),
            ];
            blocks.push(Block::Group("Notices".into(), format!("Sent by {}, to every app that connects to it.", facts.this_box), rows));
            for (ix, unavailable) in usage.unavailable.iter().enumerate() {
                blocks.push(note(&format!("note-unavailable-{ix}"), unavailable.message.clone(), false));
            }
        }
        reading => blocks.extend(unread(reading, "note-usage-settings", "the usage notices")),
    }
    blocks
}

// MARK: Models

/// A model setting: where it is, its name and line, and what none means.
const MODEL_SETTINGS: &[(&[&str], &str, &str, &str)] = &[
    (&["model"], "Model for new sessions", "New sessions use the model you picked last; this one is for when there’s none.", "The engine’s choice"),
    (&["agents", "title", "model"], "Model for session titles", "A small, fast model is enough for titles.", "The session’s model"),
    (&["subagent_model"], "Subagents use", "For the parts of a task an agent hands off, in every session here.", "Same as the conversation"),
];

fn models(facts: &Facts) -> Vec<Block> {
    if let Some(note) = unread(facts.config, "note-config", "this computer’s settings") {
        return vec![note];
    }
    let Reading::Ready(config) = facts.config else { return Vec::new() };
    // The engine's settings name an engine model as `provider/model`: its models offered are the
    // ones these can be.
    let offered: Vec<(String, String)> = match facts.offered {
        Reading::Ready(offered) => offered.models.iter().filter(|model| model.side == kuvo_box::models::Side::Engine && model.is_offered()).map(|model| (format!("{}/{}", model.provider_id, model.id), model.name.clone())).collect(),
        _ => Vec::new(),
    };
    let rows = MODEL_SETTINGS
        .iter()
        .enumerate()
        .map(|(ix, (path, title, detail, none))| {
            let key = format!("model-{ix}");
            let now = config.model_at(path);
            let control = facts.busy(&key).unwrap_or_else(|| {
                let shown = now.as_ref().map(|now| offered.iter().find(|(id, _)| id == now).map_or_else(|| now.clone(), |(_, name)| name.clone())).unwrap_or_else(|| (*none).to_owned());
                let mut choices = vec![((*none).to_owned(), None, Some(Act::SetModel { path, model: None, was: now.clone() }), now.is_none())];
                choices.extend(offered.iter().map(|(id, name)| (name.clone(), None, Some(Act::SetModel { path, model: Some(id.clone()), was: now.clone() }), now.as_deref() == Some(id))));
                Control::Menu { shown: Some(shown), choices }
            });
            Row::new(key, *title, *detail, control)
        })
        .collect();
    let mut blocks = vec![Block::Group("Defaults".into(), String::new(), rows)];
    if let Reading::Failed(why) = facts.offered {
        blocks.push(note("note-offered", format!("The models offered couldn’t be read: {why}"), false));
    }
    blocks
}

/// A page's blocks with only the rows `query` (lowercase) finds; all of them where `whole` (its
/// section's own name or words matched), and none where nothing does.
pub fn found(whole: bool, blocks: Vec<Block>, query: &str) -> Vec<Block> {
    if query.is_empty() || whole {
        return blocks;
    }
    blocks
        .into_iter()
        .filter_map(|block| match block {
            Block::Group(label, note, rows) if label.to_lowercase().contains(query) => Some(Block::Group(label, note, rows)),
            Block::Group(label, note, rows) => {
                let rows: Vec<Row> = rows.into_iter().filter(|row| row.matches(query)).collect();
                (!rows.is_empty()).then_some(Block::Group(label, note, rows))
            }
            Block::Note { .. } => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_keeps_the_rows_it_finds_and_whole_sections_it_names() {
        let blocks = vec![Block::Group("Browser".into(), String::new(), vec![Row::new("a", "Search engine", "For the address field", Control::None), Row::new("b", "Offer to save sign-ins", "", Control::None)]), note("n", "words", false)];
        let found_rows = found(false, blocks.clone(), "engine");
        assert_eq!(found_rows.len(), 1);
        let Block::Group(_, _, rows) = &found_rows[0] else { panic!() };
        assert_eq!(rows.len(), 1);
        assert_eq!(found(true, blocks.clone(), "browser").len(), 2, "the section's name keeps it whole");
        assert!(found(false, blocks, "nothing like it").is_empty());
    }
}

#[cfg(test)]
mod storage_unit_tests {
    use super::*;

    #[test]
    fn decimal_default_saved_untouched_keeps_exact_bytes() {
        let row: Described = serde_json::from_value(serde_json::json!({
            "id": "budget", "title": "Budget", "kind": "number", "value": 1073741824,
            "unit": {"name": "GB", "factor": 1000000000, "decimals": 2, "rounding": "nearest"}
        })).unwrap();
        let at = At::of("storage", &row);
        assert_eq!(at.unit.as_ref().unwrap().display(1073741824.0), "1.07 GB");
        assert_eq!(at.field_value("1.07", true), Some(serde_json::json!(1073741824)));
    }
}
