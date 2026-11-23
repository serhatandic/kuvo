//! What Settings lists itself, and the rows of the pages this app still words: your own settings
//! (this app's, on this device), and the computer's sections its box doesn't describe yet
//! (Accounts, Models, Usage). A section the box describes is drawn from its description alone
//! (`described.rs`). Drawing is in `page.rs`; nothing here reaches the box.

use kuvo_box::accounts::{AccountEntry, AccountSection, Accounts, Action, Confirm, ProgramUpdate, Side, State};
use kuvo_box::areas::Level;
use kuvo_box::models::Offered as OfferedModels;
use kuvo_box::settings::{Config, UsageSettings};
use kuvo_box::settings_pages::{BadgeTone, Confirm as Asks, FlowState, Form, Row as Described};
use kuvo_box::{Refusal, error::Offered};
use kuvo_ui::IconName;
use serde_json::Value;

use crate::areas::Reading;
use crate::updates::{Can, Facts as UpdateFacts};

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

/// Your settings: this app's own, on this device, kept (`crate::yours`) and drawn by the app.
/// Adding one: `.claude/notes/yours-settings-pattern.md`.
pub const YOURS: &[Listed] = &[
    listed("general", "General", IconName::Settings, "", "open files editor code text"),
    listed("appearance", "Appearance", IconName::Sun, "", "theme light dark system auto colours colors text size font bigger smaller larger zoom"),
    listed("notifications", "Notifications", IconName::Bell, "", "sound badge dock finished waits asks alerts"),
    listed("shortcuts", "Keyboard Shortcuts", IconName::Return, "", "keys"),
    listed("panel", "Side Panel", IconName::PanelRight, "", "browser history terminal files"),
    listed("privacy", "Privacy", IconName::Info, "", "crash reports"),
    listed("updates", "Updates", IconName::ArrowDown, "", "version beta check new release automatic download install sparkle"),
];

/// Yours that the desktop app hasn't yet: each opens a page saying so.
pub const YOURS_LATER: &[Listed] = &[
    listed("voice", "Voice", IconName::Mic, "", "microphone dictation speech"),
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
    AddAgent { account: String, name: String, sheet_title: String },
    Install { account: String, name: String },
    UpdateAgent { account: String },
    RenameAgent { account: String, name: String, label: String },
    /// Disconnect, asked first in the box's words where it says to.
    Disconnect { account: String, confirm: Option<Confirm> },
    /// An agent's own sign-in signed out, asked first in the box's words where it says to.
    SignOut { account: String, confirm: Option<Confirm> },
    /// An added account removed, asked first in the box's words where it says to.
    RemoveAgent { account: String, confirm: Option<Confirm> },
    /// A model setting at a path in the engine's settings, from what was read there.
    SetModel { path: &'static [&'static str], model: Option<String>, was: Option<String> },
    SetNotice(Notice, bool),
    /// One of your own settings changed, kept on this device.
    Yours(crate::yours::Change),
    /// The app's own updates (`crate::updates`).
    Update(UpdateAct),
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
    /// Opens or closes what a row keeps folded (a meter's breakdown), by its key.
    Expand(String),
    /// Goes to a place in Settings (`mod.rs`, its keys): another section, or the other side.
    Go(String),
    /// What a described row offers that asks for values first: its form, as a sheet.
    Form { at: At, action: String, form: Box<Form> },
    /// A flow's pasted address, sent with the action that takes it (`code`'s).
    Code { at: At, action: String },
    /// What the box offered beside the coding agents' state (Try Again), then the page read again.
    TakeAction(Offered),
    /// Check for Updates for Kuvo on your computers: the window's box asks your main computer.
    CheckComputers,
}

/// How Check for Updates for Kuvo on your computers stands: not asked, on its way, or what the
/// box answered (in its words, or the app's for the main checking).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ComputersCheck {
    #[default]
    Idle,
    Checking,
    Said(String),
}

/// What Settings › Updates asks of the app's updater.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateAct {
    Check,
    Checks(bool),
    Downloads(bool),
    Betas(bool),
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
    /// A model, from the model picker (the engine's models, which the engine's settings name as
    /// `provider/model`): what's shown, the model now, what leaving it unset means, and where it's
    /// set.
    Model { shown: String, current: Option<kuvo_box::typed::ModelRef>, unset: String, path: &'static [&'static str], was: Option<String> },
    /// Buttons, in order.
    Buttons(Vec<Push>),
    /// A field (kept by the view under the row's key), saved as Return is pressed or it's left
    /// (`save`), never with a button of its own; in it, where the row has them, a folder to
    /// choose (`browse`) and common values to pick (`picks`); a change on its way (`busy`) is a
    /// spinner in it, the field kept; and buttons beside it (`more`: a secret's Cancel).
    Field { unit: Option<String>, lines: Option<(usize, usize)>, masked: bool, value: String, placeholder: String, revision: String, save: Act, more: Vec<Push>, browse: Option<Act>, picks: Vec<Push>, busy: Option<String> },
    /// A short choice of two or three, side by side: each one's words, what picking it does
    /// (none where it can't be picked from here), and whether it's it.
    Segments(Vec<(String, Option<Act>, bool)>),
    /// A whole by its parts, as one bar with a legend: the row's actions (Refresh) over it, a
    /// change on its way, and whether its breakdown is open (`open`, toggled by `expand`).
    Parts { parts: Vec<Part>, actions: Vec<Push>, busy: Option<String>, open: bool, expand: Act },
    /// It opens something: a section, a page.
    Opens(Act),
    /// A change on its way.
    Busy(String),
    /// Windows of use, each with its share and when it starts over.
    Windows(Vec<Meter>),
    /// A thing in a list: what it offers now (buttons), what asks first (a "…" menu), and its
    /// switch, last, at the edge.
    Thing { buttons: Vec<Push>, menu: Vec<Push>, switch: Option<(bool, Option<Act>)>, busy: Option<String> },
}

/// One part of a whole, as drawn: its name, its words, its share of the whole (0 to 1), and what
/// it's made of (each one's name and words).
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub title: String,
    pub words: String,
    pub share: f32,
    pub parts: Vec<(String, String)>,
}

/// Where a row's control goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Layout {
    /// Its name and line on the left, the control on the right, in its group's card.
    #[default]
    Beside,
    /// On the page itself, full width, no card: a group's one multi-line field (under the group's
    /// title and words), or a whole by its parts.
    Bare,
}

/// A flow the box leads for a row, as drawn under it.
#[derive(Clone, Debug, PartialEq)]
pub struct FlowDrawn {
    pub state: FlowState,
    pub title: String,
    pub line: String,
    pub buttons: Vec<Push>,
    /// Where an address is pasted back: its placeholder, and what sends it.
    pub code: Option<(String, String, Act)>,
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
    /// How it stands, beside its name.
    pub badge: Option<(String, BadgeTone)>,
    /// One more line under it, set apart.
    pub notice: Option<(String, Tone)>,
    pub flow: Option<FlowDrawn>,
    pub layout: Layout,
    /// Words before its control (an `actions` row's size), quieter.
    pub aside: Option<String>,
}

impl Row {
    pub fn new(key: impl Into<String>, title: impl Into<String>, detail: impl Into<String>, control: Control) -> Row {
        Row { key: key.into(), leading: Leading::None, title: title.into(), detail: detail.into(), tone: Tone::Quiet, why: None, control, badge: None, notice: None, flow: None, layout: Layout::Beside, aside: None }
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
        let contains = |words: &str| words.to_lowercase().contains(query);
        contains(&self.title) || contains(&self.detail) || self.why.as_deref().is_some_and(contains)
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
    pub usage_agents: Option<&'a Refusal>,
    pub update_progress_unavailable: bool,
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

/// What your own settings' pages are drawn from: your settings now, and what this device has.
pub struct YoursFacts<'a> {
    pub yours: &'a crate::yours::Yours,
    /// The editors on this Mac General offers.
    pub editors: &'a [crate::yours::editors::Editor],
    pub shortcuts: &'a [super::shortcuts::Shortcut],
    pub shortcut_filter: &'a str,
    /// Why the last change couldn't be kept, where it couldn't.
    pub problem: Option<&'a str>,
    /// Where System Settings shows this app's notifications.
    pub notification_settings: &'a str,
    /// How the app stands with updating itself (none: this app has no updater, as in tests that
    /// set none). Sparkle keeps these settings itself, so they aren't in `yours`.
    pub updates: Option<&'a UpdateFacts>,
    /// The window's computer is this one, and its name ("This Mac").
    pub here: Option<&'a str>,
    /// Check for Updates for Kuvo on your computers.
    pub computers_check: &'a ComputersCheck,
    pub now: f64,
}

/// A switch of your own settings: on, and the change flipping it makes.
fn yours_switch(on: bool, change: fn(bool) -> crate::yours::Change) -> Control {
    Control::Switch { on, act: Some(Act::Yours(change(!on))) }
}

/// One of your own settings' pages, as blocks.
pub fn yours(section: &str, facts: &YoursFacts) -> Vec<Block> {
    use crate::yours::Change;
    let mut blocks = match section {
        "general" => {
            let picked = facts.yours.general.editor.as_deref().and_then(|id| facts.editors.iter().find(|editor| editor.id == id));
            let default = "Default text editor";
            let mut choices = vec![(default.to_owned(), None, Some(Act::Yours(Change::Editor(None))), picked.is_none())];
            choices.extend(facts.editors.iter().map(|editor| (editor.name.clone(), None, Some(Act::Yours(Change::Editor(Some(editor.id.clone())))), picked == Some(editor))));
            let editor = Row::new("row-editor", "Open code and text in", "From Files, for a file on this Mac.", Control::Menu { shown: Some(picked.map_or(default, |editor| &editor.name).to_owned()), choices });
            vec![Block::Group("Files".into(), String::new(), vec![editor])]
        }
        "appearance" => {
            use crate::yours::{TextSize, Theme};
            let now = &facts.yours.appearance;
            let picked = now.theme.unwrap_or(Theme::System);
            let themes = [(Theme::Light, "Light"), (Theme::Dark, "Dark"), (Theme::System, "System")];
            let theme = Row::new(
                "row-theme",
                "Theme",
                "System follows this Mac’s light or dark as it changes. ⌘⇧L switches between light and dark.",
                Control::Menu {
                    shown: themes.iter().find(|(theme, _)| *theme == picked).map(|(_, label)| (*label).to_owned()),
                    choices: themes.iter().map(|(theme, label)| ((*label).to_owned(), None, Some(Act::Yours(Change::Theme(*theme))), *theme == picked)).collect(),
                },
            );
            let size = Row::new(
                "row-text-size",
                "Text size",
                "",
                Control::Menu {
                    shown: Some(now.text_size.ui().label().to_owned()),
                    choices: TextSize::ALL.iter().map(|size| (size.ui().label().to_owned(), None, Some(Act::Yours(Change::TextSize(*size))), *size == now.text_size)).collect(),
                },
            );
            vec![Block::Group(String::new(), String::new(), vec![theme, size])]
        }
        "panel" => {
            use crate::yours::PanelStart;
            let picked = facts.yours.panel.starts_with;
            vec![Block::Group(String::new(), String::new(), vec![Row::new(
                "row-panel-start", "Start each panel with", "When the side panel opens.",
                Control::Menu {
                    shown: Some(picked.title().into()),
                    choices: PanelStart::ALL.iter().map(|start| (start.title().into(), None, Some(Act::Yours(Change::PanelStart(*start))), *start == picked)).collect(),
                },
            )])]
        }
        "shortcuts" => shortcuts(facts),
        "updates" => updates(facts),
        "privacy" => {
            let mut rows = vec![
                Row::new("row-share-usage", "Share anonymous usage", "Nothing is sent yet. This choice will apply when Kuvo adds anonymous usage reporting.", yours_switch(facts.yours.privacy.share_anonymous_usage, Change::ShareAnonymousUsage)),
                Row::new("row-privacy-policy", "Privacy", "What Kuvo keeps and shares.", Control::Buttons(vec![Push::new("Read", Act::Open("https://www.kuvo.run/code/docs/privacy/".into()))])),
            ];
            for (id, title) in [("storage", "Storage"), ("browser", "Browser"), ("vault", "Vault")] {
                rows.push(Row::new(format!("row-privacy-{id}"), title, "On the computer selected in Settings.", Control::Opens(Act::Section(id.into()))));
            }
            vec![Block::Group(String::new(), String::new(), rows)]
        }
        "notifications" => {
            let now = &facts.yours.notifications;
            let rows = vec![
                Row::new("row-notify-sessions", "Sessions", "When one finishes or asks you something, and bots’ and group chats’ news. Only while Kuvo isn’t in front.", yours_switch(now.sessions, Change::Sessions)),
                Row::new("row-notify-sound", "Play a sound", "", yours_switch(now.sound, Change::Sound)),
                Row::new("row-notify-dock", "Count on the Dock icon", "What waits for you, on all your computers.", yours_switch(now.dock, Change::Dock)),
                Row::new("row-notify-system", "System notification settings", "How they look, or to allow them again.", Control::Buttons(vec![Push::new("Open", Act::Open(facts.notification_settings.to_owned()))])),
            ];
            vec![Block::Group(String::new(), String::new(), rows)]
        }
        _ => Vec::new(),
    };
    if let Some(problem) = facts.problem.filter(|_| !blocks.is_empty()) {
        blocks.push(Block::Note { key: "note-yours-problem".into(), words: format!("Your change couldn’t be kept for next time: {problem}"), busy: false, warning: true });
    }
    blocks
}

// MARK: Updates

/// The app's version and its updates. Kuvo on this computer isn't updated here: it updates itself,
/// taking the release each new app brings, and its version is under its own settings.
fn updates(yours: &YoursFacts) -> Vec<Block> {
    let without = UpdateFacts::without(crate::updates::version(), crate::updates::NOT_THIS_BUILD);
    let facts = yours.updates.unwrap_or(&without);
    let detail = match &facts.can {
        Can::Not(why) => why.clone(),
        // (A check, a download, or Sparkle's window waiting on you: its window says which.)
        Can::Update if facts.busy => "An update is under way.".to_owned(),
        // (Sparkle couldn't start: it said why once, in its own window.)
        Can::Update if !facts.can_check => "Kuvo can’t check for updates now.".to_owned(),
        Can::Update => crate::time::last_checked(facts.checked, yours.now),
    };
    let control = if facts.updates() { Control::Buttons(vec![Push { label: "Check Now".into(), act: (facts.can_check || facts.busy).then_some(Act::Update(UpdateAct::Check)) }]) } else { Control::None };
    let mut version = Row::new("row-version", format!("Kuvo {}", facts.version), detail, control);
    if crate::updates::is_beta(&facts.version) {
        version.badge = Some(("Beta".into(), BadgeTone::Quiet));
    }
    let mut rows = vec![version];
    // (Kuvo on your computers updates together, when your main computer has a new version: Check
    // for Updates asks it to look now, through this computer's box, which says what happens.)
    if let Some(name) = yours.here {
        let check = || Control::Buttons(vec![Push { label: "Check for Updates".into(), act: Some(Act::CheckComputers) }]);
        let (detail, control) = match yours.computers_check {
            ComputersCheck::Idle => ("Updates by itself, together with your other computers.".to_owned(), check()),
            ComputersCheck::Checking => ("Asking your main computer to check for updates.".to_owned(), Control::Busy("Checking…".into())),
            ComputersCheck::Said(words) => (words.clone(), check()),
        };
        rows.push(Row::new("row-update-computer", format!("Kuvo on {name}"), detail, control));
    }
    let mut blocks = vec![Block::Group("Version".into(), String::new(), rows)];
    if facts.updates() {
        let switch = |on: bool, act: fn(bool) -> UpdateAct| Control::Switch { on, act: Some(Act::Update(act(!on))) };
        let mut rows = vec![Row::new("row-update-checks", "Check for updates automatically", "Kuvo looks once a day, and tells you when there’s a new version.", switch(facts.checks, UpdateAct::Checks))];
        if let Some(downloads) = facts.downloads {
            let line = "New versions download in the background and install the next time Kuvo quits.";
            rows.push(if facts.checks {
                Row::new("row-update-downloads", "Download and install automatically", line, switch(downloads, UpdateAct::Downloads))
            } else {
                Row::new("row-update-downloads", "Download and install automatically", line, Control::Switch { on: false, act: None }).why(Some("Turn on automatic checks first.".into()))
            });
        }
        rows.push(Row::new("row-update-betas", "Get beta updates", "Try new features before they’re released. Betas can have bugs.", switch(facts.betas, UpdateAct::Betas)));
        blocks.push(Block::Group("Getting Updates".into(), "Signed updates from Kuvo’s releases.".into(), rows));
    }
    blocks
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

/// The control for an account action, if this version of the app understands it.
pub(super) fn act_of(entry: &AccountEntry, action: &Action) -> Option<Act> {
    let (account, name) = (entry.integration.id.clone(), entry.integration.name.clone());
    Some(match action.id.as_str() {
        "addAnother" if entry.side == Side::Agents => Act::AddAgent { account, name, sheet_title: action.sheet_title.clone()? },
        "signIn" | "signInAgain" | "addAnother" => Act::SignIn { side: entry.side, account, name, sheet_title: action.sheet_title.clone()? },
        "install" => Act::Install { account, name },
        "update" => Act::UpdateAgent { account },
        "rename" => Act::RenameAgent { account, name, label: entry.account_label().to_owned() },
        "disconnect" => Act::Disconnect { account, confirm: action.confirm.clone() },
        "signOut" => Act::SignOut { account, confirm: action.confirm.clone() },
        "remove" => Act::RemoveAgent { account, confirm: action.confirm.clone() },
        _ => return None,
    })
}

fn account_row(entry: &AccountEntry, facts: &Facts) -> Row {
    let name = entry.integration.name.clone();
    let key = format!("account-{}", entry.integration.id);
    let update = entry.program_update();
    let version = match &update {
        Some(ProgramUpdate::Updated { version } | ProgramUpdate::Current { version }) => Some(version.as_str()),
        Some(ProgramUpdate::Failed { version, .. }) => version.as_deref().or_else(|| entry.program_version()),
        _ => entry.program_version(),
    };
    let mut detail = entry.line.clone();
    if let Some(version) = version {
        if !detail.is_empty() {
            detail.push_str(" · ");
        }
        detail.push_str(version);
    }
    let updating = matches!(update, Some(ProgramUpdate::Updating));
    let notice = match &update {
        Some(ProgramUpdate::Failed { message, .. }) => Some(format!("Couldn’t update {name}\n{message}")),
        Some(ProgramUpdate::Updating) if facts.update_progress_unavailable => Some("Can’t check update progress".to_owned()),
        _ => None,
    };
    let control = facts.busy(&key).unwrap_or_else(|| {
        let offered: Vec<_> = entry.actions.iter().filter(|action| action.offered(facts.here)).collect();
        let mut others = offered.iter().filter(|action| action.id != "update");
        let more = entry.state == State::SignedIn
            && others.next().is_some_and(|first| first.confirm.is_none() || others.next().is_some());

        let mut buttons = Vec::new();
        let mut menu = Vec::new();
        for action in offered {
            let mut item = Push { label: action.label.clone(), act: act_of(entry, action) };
            if more && action.id != "update" {
                if item.act.is_some() && (action.confirm.is_some() || action.signs_in() || action.id == "rename") {
                    item.label.push('…');
                }
                menu.push(item);
            } else {
                buttons.push(item);
            }
        }
        let busy = (updating && !facts.update_progress_unavailable).then(|| "Updating…".to_owned());
        if menu.is_empty() && busy.is_none() {
            if buttons.is_empty() { Control::None } else { Control::Buttons(buttons) }
        } else {
            Control::Thing { buttons, menu, switch: None, busy }
        }
    });
    let mut row = Row::new(key, name.clone(), detail, control).leading(mark(entry, name)).warning(entry.state.needs_you());
    row.notice = notice.map(|words| (words, Tone::Warning));
    row
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
    if let Some(state) = &accounts.agents_down {
        blocks.push(agents_state("Coding Agents", state, facts));
    } else if !accounts.agents.is_empty() {
        blocks.push(Block::Group("Coding Agents".into(), String::new(), accounts.agents.iter().map(|entry| account_row(entry, facts)).collect()));
    }
    // (The box's sentence says whose engine, and why: alone.)
    if let Some(why) = &accounts.engine_down {
        blocks.push(note("note-engine-down", why.message.clone(), false));
    }
    for (section, title) in [(AccountSection::Providers, "Providers"), (AccountSection::AddProvider, "Add a Provider")] {
        let rows: Vec<_> = accounts.engine.iter().filter(|entry| entry.section == section).map(|entry| account_row(entry, facts)).collect();
        if !rows.is_empty() {
            blocks.push(Block::Group(title.into(), String::new(), rows));
        }
    }
    blocks
}

/// The key of the coding agents' state on a page (its row, and the busy Try Again).
pub const AGENTS_DOWN: &str = "agents-down";

/// The coding agents' state, as the box says it (PROTOCOL.md, "The coding agents' state"), in
/// place of what needs them: restarting, its sentence as a quiet line (what was read stays, and
/// it's read again); down, its sentence alone in a card under `label`, with the box's Try Again
/// where it offers one. Never words of this app's in front of it.
fn agents_state(label: &str, state: &Refusal, facts: &Facts) -> Block {
    if state.agents_restarting() {
        return note("note-agents-restarting", state.message.clone(), false);
    }
    let control = facts.busy(AGENTS_DOWN).unwrap_or_else(|| state.action.clone().map_or(Control::None, |action| Control::Buttons(vec![Push::new(action.label.clone(), Act::TakeAction(action))])));
    Block::Group(label.into(), String::new(), vec![Row::new(AGENTS_DOWN, "", state.message.clone(), control).warning(true)])
}

// MARK: Usage

fn usage(facts: &Facts) -> Vec<Block> {
    let mut blocks = Vec::new();
    let agents = facts.usage_agents;
    if let Some(state) = agents {
        blocks.push(agents_state("Notices", state, facts));
    }
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
        // (Not read for the coding agents' state, said once above.)
        Reading::Failed(_) if agents.is_some() => {}
        Reading::Failed(_) => blocks.push(note("note-usage-settings", "Couldn’t load usage notices.", false)),
        Reading::NotYet => blocks.push(note("note-usage-settings", "Reading usage notices…", true)),
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
        Reading::Ready(offered) => offered.models.iter().map(|model| (format!("{}/{}", model.provider_id, model.id), model.name.clone())).collect(),
        _ => Vec::new(),
    };
    let ready = matches!(facts.offered, Reading::Ready(_));
    let rows = MODEL_SETTINGS
        .iter()
        .enumerate()
        .map(|(ix, (path, title, detail, none))| {
            let key = format!("model-{ix}");
            let now = config.model_at(path);
            let control = facts.busy(&key).unwrap_or_else(|| {
                let shown = now.as_ref().map(|now| offered.iter().find(|(id, _)| id == now).map_or_else(|| now.clone(), |(_, name)| name.clone())).unwrap_or_else(|| (*none).to_owned());
                if !ready {
                    // (The models not read yet, or not readable: only leaving it to the engine.)
                    return Control::Menu { shown: Some(shown), choices: vec![((*none).to_owned(), None, Some(Act::SetModel { path, model: None, was: now.clone() }), now.is_none())] };
                }
                let current = now.as_deref().and_then(|now| now.split_once('/')).map(|(provider, id)| kuvo_box::typed::ModelRef { provider_id: provider.to_owned(), id: id.to_owned(), variant: None });
                Control::Model { shown, current, unset: (*none).to_owned(), path, was: now.clone() }
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

fn shortcuts(facts: &YoursFacts) -> Vec<Block> {
    let mut blocks = vec![Block::Group(String::new(), String::new(), vec![Row::new("row-shortcut-search", "Search shortcuts", "Read-only. These are Kuvo’s current key bindings.", Control::Field { unit: None, lines: None, masked: false, value: facts.shortcut_filter.into(), placeholder: "Search actions or keys".into(), revision: String::new(), save: Act::Filter, more: vec![], browse: None, picks: vec![], busy: None })])];
    let query = facts.shortcut_filter.to_lowercase();
    for (index, shortcut) in facts.shortcuts.iter().enumerate() {
        if !format!("{} {} {} {}", shortcut.area, shortcut.title, shortcut.keys, shortcut.context).to_lowercase().contains(&query) { continue; }
        let row = Row::new(format!("row-shortcut-{index}"), &shortcut.title, super::shortcuts::description(&shortcut.context), Control::Text(shortcut.keys.clone()));
        match blocks.last_mut() {
            Some(Block::Group(area, _, rows)) if area == &shortcut.area => rows.push(row),
            _ => blocks.push(Block::Group(shortcut.area.clone(), String::new(), vec![row])),
        }
    }
    if blocks.len() == 1 { blocks.push(note("shortcuts-empty", "No shortcuts found.", false)); }
    blocks
}
