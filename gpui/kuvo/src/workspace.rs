//! The window: the title bar across the top; under it the rail, and beside the rail one raised
//! panel holding the open area's sidebar and its main view (OpenBot's layout). In Sessions the main
//! view is the conversation open or, for a new one, the landing view, with the side panel beside it;
//! in Bots, a bot's page or its own conversation; in the Work Queue, the queue or an item's page; Computers' pages aren't in
//! the app yet. In a narrow window the area's sidebar folds away.

use std::collections::{HashMap, HashSet};

use std::time::Duration;

use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement, MouseButton,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render, ScrollHandle, SharedString, Styled,
    Subscription, Task, Window, div, prelude::FluentBuilder as _, px,
};
use kuvo_box::accounts::Confirm;
use kuvo_box::follow::{Follow, ListUpdate, Timing};
use kuvo_box::models::StartIn;
use kuvo_box::search::{Found, SEARCHED_FROM};
use kuvo_box::sessions::{Facts, project_name};
use kuvo_box::areas::{Computer, Waiting};
use kuvo_box::follow::Followed;
use kuvo_box::{BoxClient, BoxInfo, Listing, Where};
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, IconName, StyledType, TextField, TextFieldEvent, TypeSize,
    menu::Menu,
    theme::{radius, space},
};

use crate::area::{Area, Back, Forward, OpenHelp, ShowAccounts, ShowBots, ShowComputers, ShowQueue, ShowSessions, ShowSettings, ShowUsage};
use crate::areas::{AreaView, Go, Picked, Reading};
use crate::composer::{Composer, ComposerEvent, Target};
use crate::first_run::{Finished, FirstRun};
use crate::conversation::ConversationView;
use crate::rail::{self, Rings};
use crate::session_menu::{
    self, ActivePopover, ArchiveSession, ConfirmDeletePopover, DeleteSession, PopoverAnchor,
    RenamePopover, RenameSession, SessionMenu,
};
use crate::sidebar::{FocusSearch, Line};
use crate::panel::{self, Place, SidePanel, Tab};
use crate::{NewSession, ShowChanges, ShowFiles, ToggleAppearance, TogglePanel, ToggleSidebar, areas, landing, sidebar, time, title_bar};

/// How long typing pauses before the box is asked to search conversations.
const SEARCH_PAUSE: Duration = Duration::from_millis(300);
/// How many conversations a search shows.
const SEARCH_MOST: usize = 20;
/// How long before a box that couldn't say whether the first run's been had is asked again.
const WELCOME_AGAIN: Duration = Duration::from_secs(2);
/// Narrower than this, the area's sidebar folds away (the rail stays).
pub const NARROW: Pixels = px(820.);
/// How far back the window remembers where it's been.
const HISTORY: usize = 100;

/// Where the window was: a place back and forward go to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Nav {
    Landing,
    Session(String),
    /// Another area, and the row picked in it (none: its own page).
    Area(Area, Option<String>),
    /// A bot's own conversation, in Bots (whichever session the box says is its conversation then).
    BotChat { bot: String },
}

/// What's still to open of where the window was last time ([`crate::yours::Spot`]), once the
/// box says it's there: until then nothing's drawn in its place (no landing view first), and
/// what's gone since, or a box that can't be reached, leaves the area's own list, quietly.
#[derive(Default)]
struct Restoring {
    /// The conversation open in Sessions: once the session list is read.
    session: Option<String>,
    /// The bot whose conversation was open in Bots: once Bots' list is read, with Bots in front.
    bot: Option<String>,
    /// The rows picked in the other areas, picked again at once: unpicked once their list is
    /// read without them.
    picks: Vec<(Area, String)>,
}

/// A bot's own conversation, shown in Bots in its page's place.
struct BotChat {
    bot: String,
    session: String,
    view: Entity<ConversationView>,
}

/// How the session lists stand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lists {
    Loading,
    /// The box can't read its engine yet.
    Waiting(String),
    /// The box waits for its update.
    Held,
    Failed(String),
    Ready,
}

/// How the box stands with this app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoxState {
    Connecting,
    Connected,
    Held,
    Unreachable(String),
}

/// What's said of a computer whose box the update gate holds: its title, and what brings it up to
/// date. `name`: the computer's (none: "This computer"); `here`: it's this Mac's own box. The window's
/// notice and the Computers area's page say it in the same words.
pub fn held_words(name: Option<&str>, here: bool) -> (String, String) {
    let subject = name.filter(|name| !name.is_empty()).unwrap_or("This computer");
    let title = format!("{subject}’s Kuvo is older than this app");
    let next = if here {
        "It can’t be used here until it’s updated.".to_owned()
    } else {
        let computer = if subject == "This computer" { "this computer" } else { subject };
        format!("Kuvo on {computer}{HELD_ELSEWHERE}")
    };
    (title, next)
}

/// How the gate's words for another computer end (`held_words`).
const HELD_ELSEWHERE: &str = " needs an update before it can be used here.";

/// What's said of a computer whose Kuvo lacks some of what this app shows (only behind): it's used
/// as it is.
pub fn behind_words() -> &'static str {
    "It works here as it is. What’s new in this app shows there once it’s updated."
}

/// What the title bar's update chip says: of the computer the window shows while its box waits for
/// an update, the box's words where the box that lists your computers brings it up to date (its
/// row's `update`, `computers.update`), else the gate's own (`held_words`); or of this Mac's own
/// Kuvo while it waits to take the release this app carries. Whether it's under way, and what its
/// card offers: always something that works, where anything can be done from here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldUpdate {
    /// The computer's key, as your computers' list names it (none: not listed there).
    pub key: Option<String>,
    pub title: String,
    pub detail: String,
    pub updating: bool,
    /// It couldn't be updated, or has waited a long while: drawn with a warning.
    pub attention: bool,
    pub action: Option<kuvo_box::areas::ComputerAction>,
    /// What the action does.
    pub does: Option<UpdateDoes>,
    /// It's this Mac's own Kuvo's update (it waits for your sessions here: "Update Ready").
    pub own: bool,
}

/// What Update Now asks of the box that lists your computers.
type Asked = Box<dyn FnOnce(&BoxClient) -> kuvo_box::Result<()> + Send>;

/// What the update chip's action does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateDoes {
    /// Asks the box that lists your computers to bring this one up to date (`update`).
    Update(String),
    /// Asks the box that lists your computers to set this one up again over SSH (`setup`), and
    /// shows its page in Computers, where how that goes (and anything it asks) is.
    SetUp(String),
    /// Asks the box that lists your computers to take the release waiting for it now (its own
    /// row's `update-here`).
    Here,
    /// Asks this Mac's own box itself to update now (`kuvo/update {now}`), where this app carries
    /// no release to bring it up with and its list says nothing of its update.
    HereNow,
    /// This Mac's own Kuvo, brought up by the installer this app carries: `now` (asked first),
    /// else once the work on this Mac ends (at once where none is).
    OwnBox { now: bool },
    /// Opens Computers › Add a Machine on the box that lists your computers, the address it was
    /// added under typed in (none: empty), to set it up over SSH from there.
    AddMachine { address: Option<String> },
}

/// How bringing this Mac's own Kuvo up to the release this app carries stands, where its box is
/// from before it did that by itself the moment its work ended (`computers.update.here`): the
/// installer this app carries runs the release's own code for it (`kuvo-install update`), and says
/// how it goes in the box's words.
// COMPAT(0.8 beta update): goes once no Mac's box is from before `computers.update.here`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnUpdate {
    /// It waits for the work on this Mac to end; Update Now asks this first.
    Waiting { title: String, confirm: Option<Confirm> },
    Updating { title: String },
    /// It didn't go through: Try Again.
    Failed { message: String },
    /// It can't be updated here now, in the box's words (blocked, off, refused): nothing to press.
    CantNow { message: String },
}

/// What Update Now asks before this Mac's own Kuvo is updated now, where the box didn't say.
// COMPAT(0.8 beta update): an installer's waiting run always says it.
pub fn own_update_confirm() -> Confirm {
    Confirm {
        title: "Update Kuvo on this Mac now?".into(),
        message: "What your sessions on this Mac are doing stops as Kuvo restarts. It’s back in a moment.".into(),
        label: "Update Now".into(),
    }
}

/// What Update Now asks first where this Mac's list says `working` conversations are at work
/// here: those it names (`at_work`, the first few), by their titles, and that what they're doing
/// stops as Kuvo restarts.
pub fn here_at_work_confirm(at_work: &[kuvo_box::areas::AtWork], working: usize) -> Confirm {
    let titles: Vec<String> = at_work.iter().filter_map(|work| work.title.clone().filter(|title| !title.is_empty())).map(|title| format!("“{title}”")).collect();
    let total = working.max(at_work.len()).max(1);
    let named = match (total, titles.as_slice()) {
        (1, [one, ..]) => format!("{one} is at work on this Mac."),
        (1, []) => "A conversation is at work on this Mac.".to_owned(),
        (2, [one, two, ..]) => format!("{one} and {two} are at work on this Mac."),
        (_, [one, ..]) => format!("{one} and {} more are at work on this Mac.", total - 1),
        (_, []) => format!("{total} conversations are at work on this Mac."),
    };
    let then = if total == 1 { "What it’s doing stops as Kuvo restarts." } else { "What they’re doing stops as Kuvo restarts." };
    Confirm { title: "Update Kuvo on this Mac now?".into(), message: format!("{named} {then} Kuvo is back in a moment."), label: "Update Now".into() }
}

/// The update chip's card: open, asking first (the box's confirm), sending, or why it couldn't.
#[derive(Default)]
struct UpdateCard {
    open: bool,
    confirming: bool,
    sending: bool,
    error: Option<String>,
    _task: Option<Task<()>>,
}

/// Whether the box says you've had the first run with it (`welcomed` in `kuvo/setup`).
enum Welcome {
    /// Not asked yet (or it couldn't say): asked as the box is reached.
    Unknown,
    Asking,
    /// Not had yet: the window shows it, and nothing else.
    FirstRun { run: Entity<FirstRun>, _finished: Subscription, _switch: Subscription },
    Had,
}

enum Open {
    Landing,
    Conversation { view: Entity<ConversationView>, session: String },
}

/// Asked of what holds the window's workspaces ([`crate::home::Home`]) from the switcher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Switch {
    /// Show the computer with this key.
    To(String),
    /// This app was removed from your devices: pair it again.
    PairAgain,
    /// Something was asked of the box that lists your computers (Update Now): read the list again
    /// where it isn't followed.
    ReadList,
    /// Update Now for this Mac's own Kuvo, by the installer this app carries: `now`, else once
    /// the work on it ends (at once where none is).
    UpdateOwnBox { now: bool },
    /// Show this computer in Computers, on the box that lists your computers: Add a Machine with
    /// this address (`add_at`), else its page.
    ShowComputer { key: String, add_at: Option<String> },
}

impl gpui::EventEmitter<Switch> for Workspace {}

pub struct Workspace {
    client: BoxClient,
    /// Your computers as this app reaches them, where the window can show another's (none: a
    /// workspace by itself, which shows its one box).
    switching: Option<crate::home::Switching>,
    /// The box that lists your computers (the nearest), which does what their rows offer.
    lister: Option<BoxClient>,
    /// How bringing this Mac's own Kuvo up to date stands, where this app does it.
    own_update: Option<OwnUpdate>,
    /// Whether this app carries a release it brings this Mac's own Kuvo up to (its installer).
    carries_release: bool,
    /// How the gate stood when `kuvo/info` was last taken: one that changes since (the box updated,
    /// its server started again on new code) has `kuvo/info` read again.
    gate_seen: kuvo_box::gate::Gate,
    /// The title bar's update chip's card.
    update_card: UpdateCard,
    /// Its box is being reached again, on your word (Try Again).
    reaching_again: bool,
    /// Whether it follows the box (the app), or shows what it's handed (tests and shots).
    live: bool,
    info: Option<BoxInfo>,
    box_state: BoxState,
    listing: Listing,
    lists: Lists,
    open: Open,
    landing: Entity<Composer>,
    /// Each conversation's composer, kept with its draft (and any send on its way) while you're
    /// in another.
    composers: HashMap<String, (Entity<Composer>, Subscription)>,
    /// Where the landing view starts a new session: the newest project's, set as the list is first
    /// read and again as the landing view opens (so it doesn't move while you write), unless one's
    /// been picked.
    landing_place: Option<StartIn>,
    landing_picked: bool,
    /// What the box said of sessions started here, until its list has them: their projects.
    started: HashMap<String, Option<String>>,
    /// Sessions started here whose first message didn't go (or came back with why), while you were
    /// elsewhere: marked in the sidebar until opened.
    unsent: HashSet<String>,
    collapsed: HashSet<String>,
    showing_all: HashSet<String>,
    /// Now, as the sidebar's times count from it (fixed for shots).
    now: Option<f64>,
    focus: FocusHandle,
    /// The sidebar's search, and what the box found in conversations for it (`finds_for`).
    search: Entity<TextField>,
    archived_filter: bool,
    finds: Vec<Found>,
    finds_for: String,
    finding: bool,
    find_failed: Option<String>,
    find_task: Option<Task<()>>,
    /// The sidebar's list while the keyboard walks it: where it is (a line's key), its focus, and
    /// its scrolling.
    walked: Option<String>,
    list_focus: FocusHandle,
    list_scroll: ScrollHandle,
    session_menu: SessionMenu,
    /// The side panel beside the conversation, whether it's open, its width (kept while it's
    /// closed), and where a drag of its edge started (the pointer's x and the width then).
    panel: Entity<SidePanel>,
    panel_open: bool,
    panel_width: Pixels,
    resizing: Option<(Pixels, Pixels)>,
    /// The area open, the other areas' sidebars (kept while hidden), and the accounts' rings.
    area: Area,
    areas: Vec<(Area, Entity<AreaView>)>,
    rings: Entity<Rings>,
    /// The bot's conversation last opened in Bots (kept while its page shows), and whether it's
    /// what Bots shows now.
    bot_chat: Option<BotChat>,
    in_bot_chat: bool,
    /// Whether Bots' main view was on screen as the window was last drawn (a group chat there is
    /// followed and marked read only while it is).
    groups_on_screen: bool,
    /// The bot whose conversation shows, as Bots was last told.
    chat_open: Option<String>,
    /// How many things wait on you in each area, as the box counts them (`kuvo/waiting`): followed
    /// where its box streams, else read again as its session list changes.
    waiting: Waiting,
    queue_badge_open_pending: bool,
    queue_waiting: Option<usize>,
    queue_badge_loaded: bool,
    queue_badge_items: Vec<kuvo_box::work_queue::Item>,
    queue_badge_task: Option<Task<()>>,
    waiting_task: Option<Task<()>>,
    /// The list changed while a read of what waits was on its way: read once more after it.
    waiting_again: bool,
    /// Your computers, as the box lists them (`kuvo/computers`): the title bar's switcher and the
    /// Computers area draw them. Followed where its box streams, else read as the box is reached
    /// and as Computers opens.
    computers: Reading<Vec<Computer>>,
    computers_task: Option<Task<()>>,
    _waiting_follow: Option<Follow<Followed<Waiting>>>,
    _computers_follow: Option<Follow<Followed<kuvo_box::areas::Computers>>>,
    /// The area's sidebar folded away by its button (a wide window), or brought out (a narrow
    /// one, where it starts folded); and whether the window was narrow when last drawn.
    folded: bool,
    shown_narrow: bool,
    narrow: bool,
    /// Where the window has been, and where in that it is now.
    history: Vec<Nav>,
    at: usize,
    /// The window it's drawn in (a restore finishes in it once the lists arrive).
    window_handle: AnyWindowHandle,
    /// The key this computer's place is kept under in your settings, once the window said it
    /// ([`Workspace::restore`]); none keeps nothing.
    place_key: Option<String>,
    restoring: Restoring,
    /// Opening what's restored now: not a move of yours, which calls the rest off.
    restoring_now: bool,
    /// The area kept as the one in front (Settings isn't one to open on: the one before stays).
    kept_area: Option<String>,
    /// The first run, while the box says it hasn't been had; and the keyboard to go to the
    /// landing view once it's done.
    welcome: Welcome,
    /// Switching computers must never start onboarding.
    onboarding_suppressed: bool,
    /// Whether the box has answered that once (it had, it hadn't, or it couldn't say): until
    /// then the window waits, saying how the box stands, rather than show the app and perhaps
    /// take it away again.
    welcome_answered: bool,
    /// Why the box couldn't say, the last time it was asked (shown while it's asked again).
    welcome_why: Option<String>,
    focus_landing: bool,
    _sessions: Option<Follow<ListUpdate>>,
    /// What macOS allows this Mac's agents: a step of the first run where it needs you, and the
    /// tile beside System Settings when it's opened from Settings › This Mac (never over the
    /// main view).
    permissions: Entity<crate::permissions::HeadsUp>,
    tasks: Vec<Task<()>>,
    _landing: Subscription,
    _search: Subscription,
    _areas: Vec<Subscription>,
}

impl Workspace {
    pub fn new(client: BoxClient, live: bool, window: &mut Window, cx: &mut Context<Self>) -> Workspace {
        let landing = cx.new(|cx| Composer::new(client.clone(), Target::New(StartIn::Chat), live, cx));
        let subscription = cx.subscribe_in(&landing, window, |this, _, event: &ComposerEvent, window, cx| this.composer_event(event, window, cx));
        let search = cx.new(|cx| TextField::new(cx).bare().placeholder("Search"));
        let search_subscription = cx.subscribe(&search, |this: &mut Workspace, _, event: &TextFieldEvent, cx| {
            if matches!(event, TextFieldEvent::Changed) {
                this.search_changed(cx);
            }
        });
        let permissions = cx.new(|cx| crate::permissions::HeadsUp::new(client.clone(), live, window, cx));
        let areas = areas::views(&client, live, cx);
        let mut area_subscriptions: Vec<Subscription> = areas.iter().map(|(_, view)| cx.subscribe_in(view, window, |this: &mut Workspace, view, _: &Picked, window, cx| this.area_picked(&view, window, cx))).collect();
        for (_, view) in &areas {
            area_subscriptions.push(cx.subscribe(view, |this: &mut Workspace, _, said: &areas::QueueDelivered, cx| {
                match &said.0 {
                    Some(said) => this.take_queue_badge(said.clone(), cx),
                    None => { this.queue_badge_open_pending = false; cx.notify(); }
                }
            }));
        }
        if let Some(view) = areas.iter().find(|(area, _)| *area == Area::Queue).map(|(_, view)| view) {
            area_subscriptions.push(cx.observe(view, |_, _, cx| cx.notify()));
        }
        // A row picked again as the window opened, gone from its list since: the list instead.
        area_subscriptions.extend(areas.iter().map(|(_, view)| cx.observe(view, |this: &mut Workspace, _, cx| this.check_picks(cx))));

        area_subscriptions.extend(areas.iter().map(|(_, view)| {
            cx.subscribe_in(view, window, |this: &mut Workspace, _, go: &Go, window, cx| match go {
                Go::BotChat { bot, session, draft } => this.open_bot_chat(bot, session, draft.as_deref(), window, cx),
                Go::Session(session) => this.open_session_now(session, window, cx),
                Go::Sessions => this.show_area(Area::Sessions, window, cx),
                Go::ReadComputers => {
                    if this._computers_follow.is_none() {
                        this.read_computers(cx);
                    }
                }
                Go::ReadList => cx.emit(Switch::ReadList),
                Go::UpdateOwnBox { now } => cx.emit(Switch::UpdateOwnBox { now: *now }),
                Go::AddAt(address) => cx.emit(Switch::ShowComputer { key: String::new(), add_at: Some(address.clone().unwrap_or_default()) }),
                // System Settings opened from Settings › This Mac: a permission added by hand
                // gets the tile beside it, as from the heads-up.
                Go::Opened(address) => {
                    this.permissions.update(cx, |permissions, cx| permissions.opened(address, cx));
                }
                Go::AddMachine => {
                    if let Some(view) = this.area_view(Area::Computers).cloned() {
                        view.update(cx, |view, cx| view.select(Some("add".into()), cx));
                    }
                    this.show_area(Area::Computers, window, cx);
                }
            })
        }));
        // The Bots list as it's read again (a bot's conversation may be another session by then).
        if let Some((_, bots)) = areas.iter().find(|(area, _)| *area == Area::Bots) {
            // A group chat picked takes the keyboard in its composer; New Group and Edit, in its name.
            area_subscriptions.push(cx.subscribe_in(bots, window, |_: &mut Workspace, view, _: &Picked, window, cx| {
                let groups = view.read(cx).groups().cloned().filter(|_| view.read(cx).selected().is_some_and(crate::areas::is_group_key));
                if let Some(groups) = groups {
                    window.focus(&groups.focus_handle(cx), cx);
                }
            }));
            area_subscriptions.push(cx.observe_in(bots, window, |this: &mut Workspace, _, window, cx| this.bots_changed(window, cx)));
        }
        let rings = cx.new(|cx| Rings::new(client.clone(), live, cx));
        let mut workspace = Workspace {
            client: client.clone(),
            switching: None,
            lister: None,
            own_update: None,
            carries_release: false,
            gate_seen: kuvo_box::gate::Gate::Unknown,
            update_card: UpdateCard::default(),
            live,
            info: None,
            box_state: BoxState::Connecting,
            listing: Listing::new(),
            lists: Lists::Loading,
            open: Open::Landing,
            landing,
            composers: HashMap::new(),
            landing_place: None,
            landing_picked: false,
            started: HashMap::new(),
            unsent: HashSet::new(),
            collapsed: HashSet::new(),
            showing_all: HashSet::new(),
            now: None,
            focus: cx.focus_handle(),
            search,
            archived_filter: false,
            finds: Vec::new(),
            finds_for: String::new(),
            finding: false,
            find_failed: None,
            find_task: None,
            walked: None,
            list_focus: cx.focus_handle(),
            list_scroll: ScrollHandle::new(),
            session_menu: SessionMenu::default(),
            panel: cx.new(|cx| SidePanel::new(client.clone(), live, cx)),
            panel_open: false,
            panel_width: panel::WIDTH,
            resizing: None,
            area: Area::Sessions,
            areas,
            rings,
            bot_chat: None,
            in_bot_chat: false,
            groups_on_screen: false,
            chat_open: None,
            waiting: Waiting::default(),
            queue_badge_open_pending: false,
            queue_waiting: None,
            queue_badge_loaded: false,
            queue_badge_items: Vec::new(),
            queue_badge_task: None,
            waiting_task: None,
            waiting_again: false,
            computers: Reading::NotYet,
            computers_task: None,
            reaching_again: false,
            _waiting_follow: None,
            _computers_follow: None,
            folded: false,
            shown_narrow: false,
            narrow: false,
            history: vec![Nav::Landing],
            at: 0,
            window_handle: window.window_handle(),
            place_key: None,
            restoring: Restoring::default(),
            restoring_now: false,
            kept_area: None,
            // (Tests and shots hand it what to show: asked in `read_at_once`.)
            onboarding_suppressed: false,
            welcome: if live { Welcome::Unknown } else { Welcome::Had },
            welcome_answered: !live,
            welcome_why: None,
            focus_landing: false,
            _sessions: None,
            permissions,
            tasks: Vec::new(),
            _landing: subscription,
            _search: search_subscription,
            _areas: area_subscriptions,
        };
        if live {
            let connecting = client.clone();
            let task = cx.background_executor().spawn(async move { connecting.connect() });
            workspace.tasks.push(cx.spawn(async move |this, cx| {
                let result = task.await;
                this.update(cx, |this, cx| this.connected(result, cx)).ok();
            }));
            let follow = client.follow_sessions(Timing::default());
            let updates = follow.updates().clone();
            workspace._sessions = Some(follow);
            workspace.tasks.push(cx.spawn(async move |this, cx| {
                while let Ok(update) = updates.recv().await {
                    if this.update(cx, |this, cx| this.take_list(update, cx)).is_err() {
                        break;
                    }
                }
            }));
            // Where the box streams, what waits on you and your computers are followed as they
            // change; elsewhere they're read again (as the list changes, as Computers opens).
            if client.streams() {
                let follow = client.follow_waiting(Timing::default());
                let updates = follow.updates().clone();
                workspace._waiting_follow = Some(follow);
                workspace.tasks.push(cx.spawn(async move |this, cx| {
                    while let Ok(update) = updates.recv().await {
                        let read = match update {
                            Followed::Now(waiting) => Ok(waiting),
                            Followed::Failed(error) => Err(error),
                        };
                        if this.update(cx, |this, cx| this.take_waiting(read, cx)).is_err() {
                            break;
                        }
                    }
                }));
                let follow = client.follow_computers(Timing::default());
                let updates = follow.updates().clone();
                workspace._computers_follow = Some(follow);
                workspace.tasks.push(cx.spawn(async move |this, cx| {
                    while let Ok(update) = updates.recv().await {
                        let read = match update {
                            Followed::Now(said) => Ok(said.computers),
                            Followed::Failed(error) => Err(error),
                        };
                        if this.update(cx, |this, cx| this.take_computers(read, cx)).is_err() {
                            break;
                        }
                    }
                }));
            }
        }
        workspace
    }

    /// Reads the box at once, on this thread, as the follows would hand it: what it is, its lists,
    /// and the landing composer's chips (shots and tests, against a stand-in).
    #[cfg(feature = "stand-in")]
    pub fn read_at_once(&mut self, cx: &mut Context<Self>) {
        let info = self.client.connect();
        self.connected(info, cx);
        let update = match self.client.session_list(None) {
            Ok(line) => {
                let mut listing = Listing::new();
                listing.take(line);
                ListUpdate::Listing(listing)
            }
            Err(kuvo_box::Error::NeedsUpdating { missing }) => ListUpdate::Held { missing },
            Err(error) => ListUpdate::Failed(error),
        };
        self.take_list(update, cx);
        self.landing.update(cx, |composer, cx| composer.read_at_once(cx));
        self.rings.update(cx, |rings, cx| rings.read_at_once(cx));
        self.read_computers(cx);
        if matches!(self.welcome, Welcome::Had | Welcome::Unknown) {
            self.welcome = Welcome::Unknown;
            self.ask_welcomed(cx);
        }
    }

    /// Opens `session` and reads it at once, on this thread (shots and tests, against a stand-in).
    #[cfg(feature = "stand-in")]
    pub fn open_read_at_once(&mut self, session: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_session(session, window, cx);
        if let Some(view) = self.conversation().cloned() {
            self.read_conversation_at_once(&view, session, cx);
        }
    }

    /// Reads `session` into `view` at once, on this thread, as its follows would hand it.
    #[cfg(feature = "stand-in")]
    fn read_conversation_at_once(&self, view: &Entity<ConversationView>, session: &str, cx: &mut Context<Self>) {
        let client = self.client.clone();
        {
            let update = match crate::conversation::read_now(&client, session) {
                Ok(conversation) => kuvo_box::ConversationUpdate::Conversation(conversation),
                Err(kuvo_box::Error::Refused(refusal)) if refusal.status == 404 => kuvo_box::ConversationUpdate::Gone(refusal),
                Err(error) => kuvo_box::ConversationUpdate::Failed(error),
            };
            let asks = client.asks(session).unwrap_or_default();
            let queue = client.queue(session).unwrap_or_default();
            view.update(cx, |view, cx| {
                view.take(update, cx);
                view.take_asks(asks, cx);
                view.take_queue(queue, cx);
                view.composer().clone().update(cx, |composer, cx| composer.read_at_once(cx));
                view.bars().clone().update(cx, |bars, cx| bars.read_at_once(cx));
            });
        }
    }

    /// What `kuvo/info` said, or why it couldn't be read.
    pub fn connected(&mut self, result: kuvo_box::Result<BoxInfo>, cx: &mut Context<Self>) {
        let was_held = matches!(self.box_state, BoxState::Held);
        self.gate_seen = self.client.gate();
        match result {
            Ok(info) => {
                self.info = Some(info);
                self.box_state = if self.client.gate().is_held() { BoxState::Held } else { BoxState::Connected };
                let name = self.box_name();
                let asked = self.computer_asked();
                for composer in self.composers.values().map(|(composer, _)| composer.clone()).chain([self.landing.clone()]).collect::<Vec<_>>() {
                    composer.update(cx, |composer, cx| composer.set_computer(asked.clone(), cx));
                }
                self.permissions.update(cx, |permissions, cx| permissions.read(cx));
                let kind = self.info.as_ref().map(|info| info.kind.clone()).unwrap_or_default();
                for (_, view) in self.areas.clone() {
                    view.update(cx, |view, cx| {
                        view.set_this_box(name.clone(), &kind, cx);
                    });
                }
                if was_held && !self.client.gate().is_held()
                    && let Some(view) = self.area_view(self.area).cloned()
                {
                    view.update(cx, |view, cx| view.shown(cx));
                }
                // Reached (perhaps updated since): what it says of its accounts is asked again, and
                // of your computers where they aren't followed.
                if self.live {
                    self.rings.update(cx, |rings, cx| rings.read_again(cx));
                    if self._computers_follow.is_none() {
                        self.read_computers(cx);
                    }
                }
            }
            Err(kuvo_box::Error::NeedsUpdating { .. }) => self.box_state = BoxState::Held,
            Err(error) => self.box_state = BoxState::Unreachable(error.to_string()),
        }
        self.box_changed(cx);
        cx.notify();
    }

    // MARK: The first run

    /// The first run, while the box says it hasn't been had.
    pub fn first_run(&self) -> Option<&Entity<FirstRun>> {
        match &self.welcome {
            Welcome::FirstRun { run, .. } => Some(run),
            _ => None,
        }
    }

    /// How the box stands changed: the first run is told, and a box just reached is asked whether
    /// it's been had, where that isn't known yet.
    fn box_changed(&mut self, cx: &mut Context<Self>) {
        // The box's bots are drawn offline while it can't be reached (a box can't say so itself).
        let reached = !matches!(self.box_state, BoxState::Unreachable(_));
        if let Some(view) = self.area_view(Area::Bots).cloned() {
            view.update(cx, |view, cx| view.set_reached(reached, cx));
        }
        if let Some(run) = self.first_run().cloned() {
            let (state, name) = (self.box_state.clone(), self.box_name());
            run.update(cx, |run, cx| run.set_box(state, name, cx));
        }
        if self.box_state == BoxState::Connected {
            self.ask_welcomed(cx);
        }
    }

    /// As the app's window starts: the box not reached yet, and not asked whether the first run's
    /// been had (tests, which otherwise start on a box that has).
    #[cfg(feature = "stand-in")]
    pub fn await_box(&mut self, cx: &mut Context<Self>) {
        self.welcome = Welcome::Unknown;
        self.welcome_answered = false;
        self.welcome_why = None;
        self.box_state = BoxState::Connecting;
        cx.notify();
    }

    /// Whether the window still waits to hear if the first run's been had: the box hasn't
    /// answered that yet. One held for its update can't say: the window shows that instead.
    pub fn awaits_box(&self) -> bool {
        !self.welcome_answered && matches!(self.welcome, Welcome::Unknown | Welcome::Asking) && self.box_state != BoxState::Held
    }

    /// Opens a selected computer directly in the app, including computers with no projects.
    pub fn skip_first_run(&mut self, cx: &mut Context<Self>) {
        self.onboarding_suppressed = true;
        self.welcome = Welcome::Had;
        self.welcome_answered = true;
        self.welcome_why = None;
        cx.notify();
    }

    /// Asks the initial box for migration from older desktop releases, once.
    // COMPAT: keep until desktop installs predating the device welcomed flag are unsupported.
    fn ask_welcomed(&mut self, cx: &mut Context<Self>) {
        if self.onboarding_suppressed || crate::yours::now(cx).welcomed {
            self.welcome = Welcome::Had;
            self.welcome_answered = true;
            return;
        }
        if !matches!(self.welcome, Welcome::Unknown) {
            return;
        }
        let client = self.client.clone();
        if !self.live {
            let said = client.welcomed();
            self.take_welcomed(said, cx);
            return;
        }
        self.welcome = Welcome::Asking;
        self.tasks.push(cx.spawn(async move |this, cx| {
            let said = cx.background_executor().spawn(async move { client.welcomed() }).await;
            this.update(cx, |this, cx| this.take_welcomed(said, cx)).ok();
        }));
    }

    /// What the box said: not had yet, the window shows the first run. A box that couldn't say
    /// (held for its update, or not answering) is asked again once it's reached; until then the
    /// window is as it would be, saying how the box stands.
    fn take_welcomed(&mut self, said: kuvo_box::Result<bool>, cx: &mut Context<Self>) {
        if self.onboarding_suppressed || crate::yours::now(cx).welcomed {
            self.welcome = Welcome::Had;
            self.welcome_answered = true;
            cx.notify();
            return;
        }
        // Only its word counts as an answer: a box that couldn't say (not answering just now, or
        // held for its update, which never got the question) is asked again, the window still
        // waiting, saying why, rather than showing the app a first run would then take away.
        self.welcome_answered |= said.is_ok();
        self.welcome_why = match &said {
            Err(kuvo_box::Error::NeedsUpdating { .. }) | Ok(_) => None,
            Err(error) => Some(kuvo_box::accounts::said(error)),
        };
        if self.live && self.welcome_why.is_some() {
            self.tasks.push(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(WELCOME_AGAIN).await;
                this.update(cx, |this, cx| this.ask_welcomed(cx)).ok();
            }));
        }
        match said {
            Ok(false) => {
                let (client, live, state, name) = (self.client.clone(), self.live, self.box_state.clone(), self.box_name());
                let (computers, now, permissions) = (self.computers.clone(), self.now, self.permissions.clone());
                let run = cx.new(|cx| {
                    let mut run = FirstRun::new(client, live, state, name, cx);
                    run.set_computers(computers, cx);
                    run.set_permissions(permissions, cx);
                    if let Some(now) = now {
                        run.set_now(now);
                    }
                    run
                });
                let finished = cx.subscribe(&run, |this: &mut Workspace, _, finished: &Finished, cx| this.finish_first_run(finished.place.clone(), cx));
                let switch = cx.subscribe(&run, |this: &mut Workspace, _, event: &Switch, cx| {
                    if let Switch::To(key) = event {
                        let key = key.clone();
                        this.skip_first_run(cx);
                        crate::yours::change(crate::yours::Change::Welcomed, cx);
                        cx.emit(Switch::To(key));
                    }
                });
                self.welcome = Welcome::FirstRun { run, _finished: finished, _switch: switch };
            }
            Ok(true) => {
                self.welcome = Welcome::Had;
                crate::yours::change(crate::yours::Change::Welcomed, cx);
            },
            Err(_) => self.welcome = Welcome::Unknown,
        }
        cx.notify();
    }

    /// The first run is done: the box is told (so every client of it agrees, and one installed
    /// again doesn't welcome you again), and the window opens on its landing view, set to start
    /// where you picked.
    fn finish_first_run(&mut self, place: StartIn, cx: &mut Context<Self>) {
        crate::yours::change(crate::yours::Change::Welcomed, cx);
        self.welcome = Welcome::Had;
        let client = self.client.clone();
        if self.live {
            // (Nothing waits on its answer: a box that didn't take it welcomes you once more.)
            self.tasks.push(cx.background_executor().spawn(async move {
                client.set_welcomed(true).ok();
            }));
        } else {
            client.set_welcomed(true).ok();
        }
        self.area = Area::Sessions;
        self.open = Open::Landing;
        self.restoring = Restoring::default();
        self.pick_place(place, cx);
        let landing = self.landing.clone();
        self.show_composer(Some(&landing), cx);
        // (What was signed in to on the way: the rail's accounts are read again.)
        if self.live {
            self.rings.update(cx, |rings, cx| rings.read_again(cx));
        }
        self.focus_landing = true;
        cx.notify();
    }

    /// What the box's session lists say, shown.
    pub fn take_list(&mut self, update: ListUpdate, cx: &mut Context<Self>) {
        match update {
            ListUpdate::Listing(listing) => {
                // (The box updated meanwhile, its server new: what it is now, read again.)
                // (Not before the first read is back: that one's still on its way.)
                if self.gate_seen != kuvo_box::gate::Gate::Unknown && self.client.gate() != self.gate_seen && self.client.gate() != kuvo_box::gate::Gate::Unknown {
                    self.read_info_again(cx);
                }
                let was_held = matches!(self.box_state, BoxState::Held);
                let previous_listing = std::mem::replace(&mut self.listing, listing);
                self.lists = Lists::Ready;
                if matches!(self.box_state, BoxState::Connecting | BoxState::Unreachable(_) | BoxState::Held) {
                    self.box_state = BoxState::Connected;
                }
                if was_held && !self.client.gate().is_held()
                    && let Some(view) = self.area_view(self.area).cloned()
                {
                    view.update(cx, |view, cx| view.shown(cx));
                }
                // Where a new one starts: set once, as the list is first read.
                if self.landing_place.is_none() {
                    let place = self.default_place();
                    self.landing_place = Some(place.clone());
                    self.landing.update(cx, |composer, cx| composer.set_start(place, cx));
                }
                // The conversation open is a bot's, as the list says now (it may not have known it
                // as the conversation opened): its sessions' bar follows the bot's work.
                if let Open::Conversation { view, session } = &self.open {
                    let bot = self.listing.facts(session).filter(|facts| facts.kind.0 == "bot").and_then(|facts| facts.bot.clone());
                    if bot.is_some() {
                        view.read(cx).bars().clone().update(cx, |bars, cx| bars.set_bot(bot, cx));
                    }
                }
                // Sessions started here are in the list now: what their box said of them goes.
                let listing = &self.listing;
                self.started.retain(|session, _| listing.facts(session).is_none());
                // A composer opened before the list knew its conversation's folder (or since it
                // moved): what's offered is read there now.
                let moved: Vec<_> = self
                    .composers
                    .iter()
                    .filter_map(|(session, (composer, _))| {
                        let folder = self.listing.facts(session)?.directory.clone();
                        (composer.read(cx).folder() != Some(folder.as_str())).then(|| (composer.clone(), folder))
                    })
                    .collect();
                for (composer, folder) in moved {
                    composer.update(cx, |composer, cx| composer.set_folder(Some(folder), cx));
                }
                // The panel's folder, as the list says it now (a session opened before the list had
                // it, a worktree's folder said later).
                self.place_panel(cx);
                // What waits on you may have changed with it: asked again where it isn't followed.
                if self._waiting_follow.is_none() {
                    self.read_waiting(cx);
                }
                // The conversation open last time, opened again now the list says whether it's
                // there (in the window, once this is done).
                if self.restoring.session.is_some() {
                    let (this, handle) = (cx.entity().downgrade(), self.window_handle);
                    App::defer(cx, move |cx| {
                        handle.update(cx, |_, window, cx| this.update(cx, |this, cx| this.restore_session(window, cx))).ok();
                    });
                }
                self.session_menu.mark_changed_actions(&previous_listing, &self.listing);
                self.prune_session_popover(cx);
                self.refresh_active_row_actions(cx);
            }
            ListUpdate::Waiting(why) => {
                // (Its engine isn't answering yet: Sessions' own view meanwhile, not nothing.)
                self.restoring.session = None;
                self.lists = Lists::Waiting(why);
            }
            ListUpdate::Held { .. } => {
                // (Held for its update: the held view, not last time's conversation.)
                self.restoring.session = None;
                self.lists = Lists::Held;
                self.box_state = BoxState::Held;
                self.session_menu.clear_actions();
                self.session_menu.active_popover = None;
            }
            ListUpdate::Failed(error) => {
                // Not reached: the list it would have been in, quietly.
                self.restoring.session = None;
                if !self.listing.is_read() {
                    self.box_state = BoxState::Unreachable(error.to_string());
                }
                self.lists = Lists::Failed(error.to_string());
            }
        }
        self.box_changed(cx);
        cx.notify();
    }

    pub fn client(&self) -> &BoxClient {
        &self.client
    }

    pub fn listing(&self) -> &Listing {
        &self.listing
    }

    pub fn lists(&self) -> &Lists {
        &self.lists
    }

    /// The heads-up for what macOS allows this Mac's agents.
    pub fn permissions(&self) -> &Entity<crate::permissions::HeadsUp> {
        &self.permissions
    }

    pub fn box_state(&self) -> &BoxState {
        &self.box_state
    }

    /// The computer as a question names it: "this Mac", or its name.
    fn computer_asked(&self) -> SharedString {
        match &self.info {
            Some(info) if !info.is_mac() || !self.client.here() => info.name.clone().into(),
            _ => "this Mac".into(),
        }
    }

    /// The box's name as the sidebar's foot says it: "This Mac" for this Mac's own.
    pub fn box_name(&self) -> SharedString {
        match &self.info {
            Some(info) if !info.is_mac() || !self.client.here() => info.name.clone().into(),
            _ => "This Mac".into(),
        }
    }

    /// The gate supplies the held fact; this window supplies the computer and update path.
    pub fn held_notice(&self) -> Option<(String, String)> {
        self.held_update().map(|held| (held.title, held.detail))
    }

    /// What the update chip says while the box this window shows waits for its update: none while
    /// it doesn't. Its card always offers what brings it up to date from here, where anything can.
    pub fn held_update(&self) -> Option<HeldUpdate> {
        let gate = self.client.gate();
        if !gate.is_older() {
            return None;
        }
        let update = self.older_update()?;
        if gate.is_held() {
            return Some(update);
        }
        // Only behind: it's in use, so never said to wait; and with nothing to press from here,
        // nothing's said at all.
        if update.action.is_none() && !update.updating {
            return None;
        }
        // (The gate's own words say it can't be used: for one in use, what's true instead.)
        let gates_words = update.detail == held_words(None, true).1 || update.detail.ends_with(HELD_ELSEWHERE);
        Some(if gates_words { HeldUpdate { detail: behind_words().to_owned(), ..update } } else { update })
    }

    /// What brings the computer this window shows up to date, held or behind.
    fn older_update(&self) -> Option<HeldUpdate> {
        let row = self.switching.as_ref().and_then(|switching| switching.shown.as_deref().and_then(|shown| switching.rows.iter().find(|row| row.key() == shown)));
        if let Some(row) = row
            && row.computer.update.is_some()
        {
            return self.offer_for(row, &row.computer.name);
        }
        let name = self.info.as_ref().map(|_| self.box_name().to_string());
        let (title, detail) = held_words(name.as_deref(), self.client.here());
        // This Mac's own box: how its update goes, where this app brings it.
        if self.client.here() {
            let key = row.map(|row| row.key().to_owned());
            return Some(self.own_update_chip().unwrap_or_else(|| self.update_here_now(key, title, detail)));
        }
        let Some(row) = row else {
            return Some(HeldUpdate { key: None, title, detail, updating: false, attention: false, action: None, does: None, own: false });
        };
        let computer = name.unwrap_or_else(|| row.computer.name.clone());
        self.offer_for(row, &computer)
    }

    /// How the box that lists your computers brings the computer `key` (another of yours) up to
    /// date, in its words, with its action (`computers.update`): what a computer's page in Computers
    /// draws where the box the window shows doesn't bring that computer up itself. Only what that
    /// box says: none where it says nothing (the chip's own fallback for a box from before isn't
    /// drawn there).
    pub fn update_offer(&self, key: &str) -> Option<HeldUpdate> {
        let row = self.switching.as_ref()?.rows.iter().find(|row| row.key() == key && !row.computer.here)?;
        row.computer.update.as_ref()?;
        self.offer_for(row, &row.computer.name)
    }

    /// What brings `row`'s computer (named `computer`) up to date from here, as the chip and a
    /// computer's page offer it.
    fn offer_for(&self, row: &kuvo_box::boxes::Row, computer: &str) -> Option<HeldUpdate> {
        if let Some(update) = &row.computer.update {
            let updating = update.is_updating();
            // (Brought up by the box that lists it, or set up over SSH from there: `add`.)
            let action = (!updating).then(|| row.computer.actions.iter().find(|action| action.id == "update" || action.id == "add").cloned()).flatten();
            let does = action.as_ref().map(|action| if action.id == "add" { UpdateDoes::AddMachine { address: row.computer.id.clone() } } else { UpdateDoes::Update(row.key().to_owned()) });
            return Some(HeldUpdate {
                key: Some(row.key().to_owned()),
                title: update.title.clone(),
                detail: update.detail.clone(),
                updating,
                attention: update.state == "failed" || update.long,
                action,
                does,
                own: false,
            });
        }
        let (title, detail) = held_words(Some(computer), false);
        let key = row.key().to_owned();
        // COMPAT(0.8 beta update): what's offered where the box that lists your computers is from
        // before it said how each older one is brought up (`computers.update`): goes once no box is
        // from before `computers.update.here`.
        // This Mac's own Kuvo first, while its update is under way or waits.
        if let Some(own) = self.own_update_chip() {
            return Some(HeldUpdate { key: Some(key), detail: format!("Then {computer} can be brought up to date from here."), ..own });
        }
        // Set up again over SSH, by the box that set it up: that brings it up to that box's Kuvo.
        if row.computer.action("setup").is_some_and(|action| action.why.is_none()) {
            return Some(HeldUpdate {
                key: Some(key.clone()),
                title,
                detail: format!("Update Now sets Kuvo up on {computer} again over SSH. What’s at work there keeps going."),
                updating: false,
                attention: false,
                action: Some(kuvo_box::areas::ComputerAction { id: "setup".into(), label: "Update Now".into(), confirm: None, why: None }),
                does: Some(UpdateDoes::SetUp(key)),
                own: false,
            });
        }
        // Another Mac whose Kuvo is from before computers followed each other (one that follows
        // says so: its row's `update`, above): only the gate's words, nothing to press here.
        if row.computer.is_mac() {
            return Some(HeldUpdate { key: Some(key), title, detail, updating: false, attention: false, action: None, does: None, own: false });
        }
        // A machine nothing here reaches over SSH: it's set up over SSH once more, from Computers.
        let address = row.computer.id.clone().filter(|id| !id.is_empty());
        Some(HeldUpdate {
            key: Some(key),
            title,
            detail: format!("This Mac can’t reach {computer} over SSH to update it. Set it up over SSH from Computers, at the address you reach it at."),
            updating: false,
            attention: false,
            action: Some(kuvo_box::areas::ComputerAction { id: "add".into(), label: "Set Up Over SSH…".into(), confirm: None, why: None }),
            does: Some(UpdateDoes::AddMachine { address }),
            own: false,
        })
    }

    // COMPAT(0.8 beta update): for a box from before `computers.update.here` (its row says nothing
    // of its update); goes with `OwnUpdate`.
    /// Update Now for this Mac's own Kuvo where nothing says how its update stands: by the release
    /// this app carries (its installer, `--now`), else by asking the box itself. Asked first only
    /// where its list says something is at work there, naming what.
    fn update_here_now(&self, key: Option<String>, title: String, detail: String) -> HeldUpdate {
        let here = self.switching.as_ref().and_then(|switching| switching.rows.iter().find(|row| row.computer.here));
        // (Asked first unless its box says nothing's at work: one that never said is asked.)
        let confirm = match here.map(|row| (&row.computer.at_work, row.computer.working)) {
            Some((_, Some(0))) => None,
            Some((at_work, Some(working))) => Some(here_at_work_confirm(at_work, working)),
            _ => Some(own_update_confirm()),
        };
        let does = if self.carries_release { UpdateDoes::OwnBox { now: true } } else { UpdateDoes::HereNow };
        HeldUpdate {
            key,
            title,
            detail,
            updating: false,
            attention: false,
            action: Some(kuvo_box::areas::ComputerAction { id: "update-here".into(), label: "Update Now".into(), confirm, why: None }),
            does: Some(does),
            own: true,
        }
    }

    /// Whether this app carries a release it brings this Mac's own Kuvo up to.
    pub fn set_carries_release(&mut self, carries: bool, cx: &mut Context<Self>) {
        if self.carries_release != carries {
            self.carries_release = carries;
            cx.notify();
        }
    }

    /// How this Mac's own Kuvo's update stands, where it waits for one or takes it now: as its box
    /// says (its own row's `update`, `computers.update.here`), else as the installer this app
    /// carries says it, for a box from before.
    fn own_update_chip(&self) -> Option<HeldUpdate> {
        let here = self.switching.as_ref().and_then(|switching| switching.rows.iter().find(|row| row.computer.here));
        if let Some(row) = here
            && let Some(update) = &row.computer.update
        {
            let updating = update.is_updating();
            let action = (!updating).then(|| row.computer.action("update-here").cloned()).flatten();
            return Some(HeldUpdate {
                key: Some(row.key().to_owned()),
                title: update.title.clone(),
                detail: update.detail.clone(),
                updating,
                attention: update.state == "failed" || update.long,
                does: action.as_ref().map(|_| UpdateDoes::Here),
                action,
                own: true,
            });
        }
        // COMPAT(0.8 beta update): the installer's words for a box from before `computers.update.here`.
        let action = |label: &str, confirm: Option<Confirm>| Some(kuvo_box::areas::ComputerAction { id: "update-here".into(), label: label.into(), confirm, why: None });
        let chip = |title: String, detail: String| HeldUpdate { key: None, title, detail, updating: false, attention: false, action: None, does: None, own: true };
        Some(match self.own_update.clone()? {
            // (Update Now asks first, always: what's at work here stops.)
            OwnUpdate::Waiting { title, confirm } => HeldUpdate { action: action("Update Now", Some(confirm.unwrap_or_else(own_update_confirm))), does: Some(UpdateDoes::OwnBox { now: true }), ..chip(title, String::new()) },
            OwnUpdate::Updating { title } => HeldUpdate { updating: true, ..chip(title, String::new()) },
            // (Tried again as it was at first: at once where nothing's at work, else once that ends.)
            OwnUpdate::Failed { message } => HeldUpdate { attention: true, action: action("Try Again", None), does: Some(UpdateDoes::OwnBox { now: false }), ..chip("Kuvo on this Mac couldn’t be updated".into(), message) },
            OwnUpdate::CantNow { message } => HeldUpdate { attention: true, ..chip("Kuvo on this Mac can’t be updated now".into(), message) },
        })
    }

    /// What the title bar's update chip says now: of the computer shown while it waits for its
    /// update, else of this Mac's own Kuvo while it waits for one or takes it.
    pub fn update_chip(&self) -> Option<HeldUpdate> {
        self.held_update().or_else(|| self.own_update_chip())
    }

    /// How bringing this Mac's own Kuvo up to date stands, where this app does it.
    pub fn set_own_update(&mut self, own: Option<OwnUpdate>, cx: &mut Context<Self>) {
        if self.own_update != own {
            self.own_update = own;
            self.sync_offers(cx);
            cx.notify();
        }
    }

    /// Hands the Computers area what brings each of your other computers up to date from here
    /// (`update_offer`), and the box that does it.
    fn sync_offers(&self, cx: &mut Context<Self>) {
        let Some(page) = self.area_view(Area::Computers).and_then(|view| view.read(cx).computers_page().cloned()) else { return };
        let offers: HashMap<String, HeldUpdate> = self
            .switching
            .iter()
            .flat_map(|switching| switching.rows.iter())
            .filter_map(|row| self.update_offer(row.key()).map(|offer| (row.key().to_owned(), offer)))
            .collect();
        let lister = self.lister.clone();
        page.update(cx, |page, cx| page.set_offers(offers, lister, cx));
    }

    pub fn own_update(&self) -> Option<&OwnUpdate> {
        self.own_update.as_ref()
    }

    /// Shows a computer in Computers: Add a Machine with `add_at` typed in, else its page.
    pub fn show_computer(&mut self, key: &str, add_at: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.area_view(Area::Computers).cloned() {
            if let Some(address) = add_at
                && let Some(page) = view.read(cx).computers_page().cloned()
            {
                page.update(cx, |page, cx| page.type_host(address, cx));
            }
            if let Some(page) = view.read(cx).computers_page().cloned() {
                page.update(cx, |page, cx| page.tries_changed(cx));
            }
            view.update(cx, |view, cx| view.select(Some(if add_at.is_some() { "add".into() } else { format!("computer:{key}") }), cx));
        }
        self.show_area(Area::Computers, window, cx);
    }

    /// The box that lists your computers, which Update Now asks.
    pub fn set_lister(&mut self, lister: Option<BoxClient>) {
        self.lister = lister;
    }

    pub fn update_card_open(&self) -> bool {
        self.update_card.open
    }

    pub fn update_card_confirming(&self) -> bool {
        self.update_card.confirming
    }

    pub fn update_card_sending(&self) -> bool {
        self.update_card.sending
    }

    pub fn update_card_error(&self) -> Option<&str> {
        self.update_card.error.as_deref()
    }

    /// Opens the update chip's card, or closes it.
    pub fn toggle_update_card(&mut self, cx: &mut Context<Self>) {
        if self.update_card.open {
            return self.close_update_card(cx);
        }
        self.update_card = UpdateCard { open: true, ..UpdateCard::default() };
        cx.notify();
    }

    /// Closes the update chip's card (not while what it asked is on its way).
    pub fn close_update_card(&mut self, cx: &mut Context<Self>) {
        if self.update_card.sending {
            return;
        }
        self.update_card = UpdateCard::default();
        cx.notify();
    }

    /// Update Now: asked first where the box says to (`confirmed`: it was), then done as the chip
    /// offers it: asked of the box that lists your computers, by the installer this app carries
    /// for this Mac's own Kuvo, or in Computers. The list says how it goes from then on.
    pub fn update_now(&mut self, confirmed: bool, cx: &mut Context<Self>) {
        if self.update_card.sending {
            return;
        }
        let Some(held) = self.update_chip() else { return self.close_update_card(cx) };
        let (Some(does), Some(action)) = (held.does, held.action) else { return };
        let here_now = does == UpdateDoes::HereNow;
        if action.why.is_some() {
            return;
        }
        if action.confirm.is_some() && !confirmed {
            self.update_card.confirming = true;
            self.update_card.error = None;
            return cx.notify();
        }
        // (What's shown once it's asked: setting a machine up again shows its page in Computers,
        // where how that goes, and anything it asks, is.)
        let mut then = None;
        let work: Asked = match does {
            UpdateDoes::OwnBox { now } => {
                self.update_card = UpdateCard::default();
                cx.emit(Switch::UpdateOwnBox { now });
                return cx.notify();
            }
            UpdateDoes::AddMachine { address } => {
                self.update_card = UpdateCard::default();
                cx.emit(Switch::ShowComputer { key: String::new(), add_at: Some(address.unwrap_or_default()) });
                return cx.notify();
            }
            UpdateDoes::SetUp(key) => {
                then = Some(Switch::ShowComputer { key: key.clone(), add_at: None });
                Box::new(move |lister| lister.setup_computer(&key).map(|_| ()))
            }
            UpdateDoes::Update(key) => Box::new(move |lister| lister.update_computer(&key).map(|_| ())),
            UpdateDoes::Here | UpdateDoes::HereNow => Box::new(|lister| lister.update_here()),
        };
        // (This Mac's own box is asked itself: its update never waits on another computer's list.)
        let target = if here_now { Some(self.client.clone()) } else { self.lister.clone() };
        let Some(lister) = target else {
            self.update_card.error = Some("Your computers’ list isn’t reachable right now. Try again in a moment.".into());
            return cx.notify();
        };
        let done = move |this: &mut Workspace, result: kuvo_box::Result<()>, cx: &mut Context<Workspace>| {
            this.update_card.sending = false;
            match result {
                Ok(()) => {
                    this.update_card = UpdateCard::default();
                    cx.emit(Switch::ReadList);
                    if let Some(then) = then {
                        cx.emit(then);
                    }
                }
                Err(error) => {
                    this.update_card.confirming = false;
                    this.update_card.error = Some(kuvo_box::accounts::said(&error));
                }
            }
            cx.notify();
        };
        self.update_card.sending = true;
        self.update_card.error = None;
        if !self.live {
            let result = work(&lister);
            return done(self, result, cx);
        }
        cx.notify();
        self.update_card._task = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { work(&lister) }).await;
            this.update(cx, |this, cx| done(this, result, cx)).ok();
        }));
    }

    /// What the window shows in place of its areas while the computer it shows can't be used: its
    /// Kuvo is older than this app (with what brings it up, as the update chip offers it), or it
    /// can't be reached (Try Again). With another of your computers that works, to switch to.
    /// None while it can be used.
    pub fn blocked(&self) -> Option<crate::blocked::Blocked> {
        use crate::blocked::{Blocked, BlockedDo, unreached_words};
        let name = self.info.as_ref().map(|_| self.box_name().to_string()).or_else(|| self.shown_row().map(|row| row.computer.name.clone())).unwrap_or_else(|| "this computer".into());
        let error = self.update_card.error.clone();
        // (Only a computer this app can't speak to at all: one that's only behind is used as it is,
        // its update offered in the title bar's chip.)
        let (kind, title, detail, busy, attention, action) = if let Some(held) = self.held_update().filter(|_| self.client.gate().is_held()) {
            let action = held.action.as_ref().filter(|action| action.why.is_none() && held.does.is_some()).map(|action| (action.label.clone(), BlockedDo::Update));
            // (Why it can't be used first, then how its update stands, where the box says more.)
            let (older, _) = held_words(Some(&name), self.client.here());
            let (title, detail) = if held.title == older || held.own {
                (held.title, held.detail)
            } else {
                let line = if held.title.ends_with(['.', '…']) { held.title } else { format!("{}.", held.title) };
                (older, if held.detail.is_empty() { line } else { format!("{line} {}", held.detail) })
            };
            ("held", title, detail, held.updating || self.update_card.sending, held.attention, action)
        } else if let BoxState::Unreachable(_) = &self.box_state
            && !self.client.here()
        {
            // (Another of your computers: this Mac's own box not answering is the launch's to say.)
            // (In the box's words where the list says why it's away; never the error's own.)
            let away = self.shown_row().and_then(|row| row.computer.notice.clone());
            let (title, detail) = away.map(|notice| (notice.title, notice.detail)).unwrap_or_else(|| unreached_words(&name));
            ("unreached", title, detail, false, true, Some(("Try Again".to_owned(), BlockedDo::TryAgain)))
        } else if self.box_state == BoxState::Connecting && self.reaching_again && !self.client.here() {
            let (title, _) = unreached_words(&name);
            ("unreached", title, format!("Reaching {name}…"), true, false, None)
        } else {
            return None;
        };
        // Another of yours that works now: this Mac's own first, then the first answering.
        let shown = self.switching.as_ref().and_then(|switching| switching.shown.clone());
        let switch_to = self.switching.as_ref().and_then(|switching| {
            let works = |row: &&kuvo_box::boxes::Row| Some(row.key()) != shown.as_deref() && !row.computer.is_older() && matches!(row.reach, kuvo_box::boxes::Reach::Nearest | kuvo_box::boxes::Reach::Connected);
            let rows = &switching.rows;
            rows.iter().find(|row| works(row) && matches!(row.reach, kuvo_box::boxes::Reach::Nearest)).or_else(|| rows.iter().find(works)).map(|row| (row.key().to_owned(), row.computer.name.clone()))
        });
        Some(Blocked { kind, title, detail, busy, attention, action, switch_to, error })
    }

    /// The computer this window shows, as your computers' list has it.
    fn shown_row(&self) -> Option<&kuvo_box::boxes::Row> {
        let switching = self.switching.as_ref()?;
        let shown = switching.shown.as_deref()?;
        switching.rows.iter().find(|row| row.key() == shown)
    }

    /// Update Now from the window's "can't be used" view: as the update chip does it, its card
    /// open for what it asks first.
    pub fn blocked_update(&mut self, cx: &mut Context<Self>) {
        if !self.update_card.open {
            self.update_card = UpdateCard { open: true, ..UpdateCard::default() };
        }
        self.update_now(false, cx);
    }

    /// Reaches the box this window shows again (it couldn't be reached): asked once more, as at the
    /// window's start.
    pub fn reach_again(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.box_state = BoxState::Connecting;
        self.reaching_again = true;
        cx.notify();
        let client = self.client.clone();
        if !self.live {
            let result = client.connect();
            self.reaching_again = false;
            return self.connected(result, cx);
        }
        let task = cx.background_executor().spawn(async move { client.connect() });
        self.tasks.push(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.reaching_again = false;
                this.connected(result, cx);
            })
            .ok();
        }));
    }

    /// Reads `kuvo/info` again, quietly (the box changed under the window: updated, or started
    /// again): what it has now, its update chip with it.
    fn read_info_again(&mut self, cx: &mut Context<Self>) {
        self.gate_seen = self.client.gate();
        let client = self.client.clone();
        if !self.live {
            let result = client.connect();
            return self.connected(result, cx);
        }
        let task = cx.background_executor().spawn(async move { client.connect() });
        self.tasks.push(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| this.connected(result, cx)).ok();
        }));
    }

    /// Shows another of your computers in this window, as the title bar's computer menu does.
    pub fn switch_to(&mut self, key: &str, cx: &mut Context<Self>) {
        cx.emit(Switch::To(key.to_owned()));
    }

    pub fn now(&self) -> f64 {
        self.now.unwrap_or_else(time::now_ms)
    }

    /// Counts the sidebar's times from `now` (shots, so they read the same each time).
    pub fn set_now(&mut self, now: f64, cx: &mut Context<Self>) {
        self.now = Some(now);
        self.rings.update(cx, |rings, _| rings.set_now(now));
        if let Some(run) = self.first_run().cloned() {
            run.update(cx, |run, _| run.set_now(now));
        }
        for (_, view) in self.areas.clone() {
            view.update(cx, |view, cx| view.set_now(now, cx));
        }
    }

    pub fn is_landing(&self) -> bool {
        matches!(self.open, Open::Landing)
    }

    pub fn landing(&self) -> &Entity<Composer> {
        &self.landing
    }

    /// The session open, if one is.
    pub fn opened(&self) -> Option<&str> {
        match &self.open {
            Open::Conversation { session, .. } => Some(session),
            Open::Landing => None,
        }
    }

    pub fn conversation(&self) -> Option<&Entity<ConversationView>> {
        match &self.open {
            Open::Conversation { view, .. } => Some(view),
            Open::Landing => None,
        }
    }

    pub fn is_collapsed(&self, project: &str) -> bool {
        self.collapsed.contains(project)
    }

    pub fn toggle_project(&mut self, project: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(project) {
            self.collapsed.insert(project.to_owned());
        }
        self.prune_session_popover(cx);
        cx.notify();
    }

    pub fn shows_all(&self, project: &str) -> bool {
        self.showing_all.contains(project)
    }

    pub fn toggle_all(&mut self, project: &str, cx: &mut Context<Self>) {
        if !self.showing_all.remove(project) {
            self.showing_all.insert(project.to_owned());
        }
        self.prune_session_popover(cx);
        cx.notify();
    }

    /// Whether `session` was started here and what was written didn't go while you were elsewhere.
    pub fn is_unsent(&self, session: &str) -> bool {
        self.unsent.contains(session)
    }

    /// Opens a session's conversation.
    pub fn open_session(&mut self, session: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.area = Area::Sessions;
        self.session_menu.active_popover = None;
        self.session_menu.action_error = None;
        self.record(Nav::Session(session.to_owned()), cx);
        if self.opened() == Some(session) {
            // Already open (back to it from another area): the keyboard goes back to it.
            if let Some(view) = self.conversation() {
                window.focus(&view.focus_handle(cx), cx);
            }
            cx.notify();
            return;
        }
        self.unsent.remove(session);
        let (client, live) = (self.client.clone(), self.live);
        let composer = self.composer_for(session, window, cx);
        // Only the composer on screen asks again for what the box couldn't say.
        self.show_composer(Some(&composer), cx);
        let view = cx.new(|cx| ConversationView::new(client, session.to_owned(), composer, live, cx));
        // (A bot's own conversation, as the session lists say: its sessions show over its composer.)
        let bot = self.listing().facts(session).filter(|facts| facts.kind.0 == "bot").and_then(|facts| facts.bot.clone());
        self.wire_bars(&view, bot, cx);
        window.focus(&view.focus_handle(cx), cx);
        self.open = Open::Conversation { view, session: session.to_owned() };
        self.place_panel(cx);
        cx.notify();
    }

    /// The bars over a conversation's composer: the bot it is (its sessions' bar), and how a row
    /// opens its conversation (here, in Sessions).
    fn wire_bars(&self, view: &Entity<ConversationView>, bot: Option<String>, cx: &mut Context<Self>) {
        let workspace = cx.entity().downgrade();
        let opener: crate::work_bars::Opener = std::rc::Rc::new(move |session: &str, window: &mut Window, cx: &mut App| {
            workspace.update(cx, |workspace, cx| workspace.open_session_now(session, window, cx)).ok();
        });
        let now = self.now;
        view.read(cx).bars().clone().update(cx, |bars, cx| {
            bars.set_opener(opener);
            bars.set_now(now);
            bars.set_bot(bot, cx);
        });
    }

    /// Opens a session's conversation, read as the window reads (in tests and shots, at once).
    pub fn open_session_now(&mut self, session: &str, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(feature = "stand-in")]
        if !self.live {
            self.open_read_at_once(session, window, cx);
            return;
        }
        self.open_session(session, window, cx);
    }

    // MARK: A bot's conversation

    /// The bot whose conversation Bots shows now, and the conversation.
    pub fn bot_chat(&self) -> Option<(&str, &Entity<ConversationView>)> {
        self.bot_chat.as_ref().filter(|_| self.in_bot_chat && self.area == Area::Bots).map(|chat| (chat.bot.as_str(), &chat.view))
    }

    /// Opens a bot's own conversation in Bots, in its page's place, its row still picked; `draft`
    /// goes in its composer where nothing's written there (a new bot's job, to edit and send).
    pub fn open_bot_chat(&mut self, bot: &str, session: &str, draft: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.restoring_now {
            self.restoring.bot = None;
        }
        self.area = Area::Bots;
        self.bring_out_sidebar(window);
        if let Some(view) = self.area_view(Area::Bots).cloned() {
            view.update(cx, |view, cx| view.select(Some(format!("bot:{bot}")), cx));
        }
        self.record(Nav::BotChat { bot: bot.to_owned() }, cx);
        self.in_bot_chat = true;
        let composer = self.composer_for(session, window, cx);
        if self.bot_chat.as_ref().is_none_or(|chat| chat.session != session) {
            let (client, live) = (self.client.clone(), self.live);
            let view = cx.new(|cx| ConversationView::new(client, session.to_owned(), composer.clone(), live, cx));
            self.wire_bars(&view, Some(bot.to_owned()), cx);
            #[cfg(feature = "stand-in")]
            if !self.live {
                self.read_conversation_at_once(&view, session, cx);
            }
            self.bot_chat = Some(BotChat { bot: bot.to_owned(), session: session.to_owned(), view });
        }
        if let Some(draft) = draft
            && composer.read(cx).field().read(cx).text().is_empty()
        {
            composer.update(cx, |composer, cx| composer.restore(draft, &[], cx));
        }
        self.show_composer_on_screen(cx);
        if let Some(chat) = &self.bot_chat {
            window.focus(&chat.view.focus_handle(cx), cx);
        }
        cx.notify();
    }

    /// The session the box lists as `bot`'s own conversation now.
    fn bot_session(&self, bot: &str, cx: &App) -> Option<String> {
        match self.area_view(Area::Bots)?.read(cx).bots() {
            Reading::Ready(bots) => bots.bots.iter().find(|each| each.id == bot).map(|each| each.session.clone()),
            _ => None,
        }
    }

    /// The box's bots were read again: a conversation shown for a bot whose conversation is
    /// another session now (a coding agent's model carried it on) goes to that one.
    fn bots_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.restore_bot(window, cx);
        let Some(chat) = &self.bot_chat else { return };
        let (bot, session) = (chat.bot.clone(), chat.session.clone());
        match self.bot_session(&bot, cx) {
            Some(now) if now != session => {
                if self.bot_chat().is_some() {
                    self.open_bot_chat(&bot, &now, None, window, cx);
                } else {
                    // (Not on screen: made again the next time it's opened.)
                    self.bot_chat = None;
                }
            }
            _ => {}
        }
    }

    /// From a bot's conversation back to its page.
    pub fn close_bot_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bot) = self.bot_chat().map(|(bot, _)| bot.to_owned()) else { return };
        self.restoring.bot = None;
        self.in_bot_chat = false;
        self.record(Nav::Area(Area::Bots, Some(format!("bot:{bot}"))), cx);
        self.show_composer_on_screen(cx);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Marks the composer on screen now as the one shown: a bot's conversation's in Bots, the open
    /// conversation's or the landing view's in Sessions, none elsewhere.
    fn show_composer_on_screen(&mut self, cx: &mut Context<Self>) {
        let shown = match self.area {
            Area::Bots => self.bot_chat().map(|(_, view)| view.read(cx).composer().clone()),
            Area::Sessions => Some(self.conversation().map_or_else(|| self.landing.clone(), |view| view.read(cx).composer().clone())),
            _ => None,
        };
        self.show_composer(shown.as_ref(), cx);
    }

    /// `session`'s composer: the one it had, with its draft (its model read again, as it may have
    /// changed meanwhile), or a new one.
    fn composer_for(&mut self, session: &str, window: &mut Window, cx: &mut Context<Self>) -> Entity<Composer> {
        // Its folder, where its project's own settings may add models to what's offered.
        let folder = self.listing.facts(session).map(|facts| facts.directory.clone());
        if let Some((composer, _)) = self.composers.get(session) {
            let composer = composer.clone();
            if self.live {
                composer.update(cx, |composer, cx| {
                    if !composer.set_folder(folder, cx) {
                        composer.read_models(cx);
                    }
                });
            }
            return composer;
        }
        let (client, live) = (self.client.clone(), self.live);
        let composer = cx.new(|cx| Composer::in_folder(client, Target::Session { id: session.to_owned(), place: Where::Engine }, folder, live, cx));
        let subscription = cx.subscribe_in(&composer, window, |this, _, event: &ComposerEvent, window, cx| this.composer_event(event, window, cx));
        // Not on screen until its conversation opens.
        let asked = self.computer_asked();
        composer.update(cx, |composer, cx| {
            composer.set_shown(false, cx);
            composer.set_computer(asked, cx);
        });
        self.composers.insert(session.to_owned(), (composer.clone(), subscription));
        composer
    }

    /// The landing view, for a new conversation in `project` (or the one picked last).
    pub fn new_session(&mut self, project: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.client.gate().is_held() {
            return;
        }
        self.area = Area::Sessions;
        self.session_menu.active_popover = None;
        self.session_menu.action_error = None;
        self.record(Nav::Landing, cx);
        if let Some(project) = project {
            self.landing_place = Some(StartIn::Project(project));
            self.landing_picked = true;
        } else if !self.landing_picked {
            // The newest project's, as it opens; it stays put while you write.
            self.landing_place = Some(self.default_place());
        }
        let place = self.landing_place();
        self.landing.update(cx, |composer, cx| composer.set_start(place, cx));
        let landing = self.landing.clone();
        self.show_composer(Some(&landing), cx);
        // What's offered may have changed since it was last open (an account signed in elsewhere),
        // or (another project) a new one there starts on another: read again, once.
        // (And this computer's mode for a new one: the last picked, Full Access confirmed since.)
        landing.update(cx, |composer, cx| {
            composer.read_models(cx);
            if composer.reads() {
                composer.read_approval(cx);
            }
        });
        self.open = Open::Landing;
        self.place_panel(cx);
        window.focus(&self.landing.focus_handle(cx), cx);
        cx.notify();
    }

    /// Marks `shown` as the composer on screen, and every other as not.
    fn show_composer(&mut self, shown: Option<&Entity<Composer>>, cx: &mut Context<Self>) {
        let all: Vec<Entity<Composer>> = self.composers.values().map(|(composer, _)| composer.clone()).chain([self.landing.clone()]).collect();
        for composer in all {
            let on = shown == Some(&composer);
            composer.update(cx, |composer, cx| composer.set_shown(on, cx));
        }
    }

    /// Where a new conversation starts before one's picked: the newest project's, else a chat.
    fn default_place(&self) -> StartIn {
        self.listing.sidebar().projects.first().map_or(StartIn::Chat, |group| StartIn::Project(group.directory.clone()))
    }

    pub fn landing_place(&self) -> StartIn {
        self.landing_place.clone().unwrap_or_else(|| self.default_place())
    }

    pub fn pick_place(&mut self, place: StartIn, cx: &mut Context<Self>) {
        self.landing_place = Some(place.clone());
        self.landing_picked = true;
        self.place_panel(cx);
        self.landing.update(cx, |composer, cx| {
            composer.set_start(place, cx);
            composer.read_models(cx);
        });
        cx.notify();
    }

    /// A project's name, as the lists name it.
    pub fn project_name(&self, directory: &str) -> String {
        project_name(directory, self.listing.projects())
    }

    fn composer_event(&mut self, event: &ComposerEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            ComposerEvent::Started { session, project, written, files, sent, .. } => {
                // What the box said of what was written (or why it didn't go) goes to the new
                // conversation's own composer; the landing one is left empty.
                self.started.insert(session.clone(), project.clone());
                let composer = self.composer_for(session, window, cx);
                composer.update(cx, |composer, cx| composer.take_sent(written, files.clone(), sent.clone(), cx));
                // Opened, unless you've gone to another conversation meanwhile, or started
                // writing the next one.
                let writing_next = !self.landing.read(cx).field().read(cx).text().trim().is_empty() || !self.landing.read(cx).attached().is_empty();
                if self.is_landing() && !writing_next {
                    self.open_session(session, window, cx);
                } else if self.opened() != Some(session.as_str())
                    && (composer.read(cx).field().read(cx).error().is_some()
                        || !composer.read(cx).field().read(cx).text().is_empty()
                        || !composer.read(cx).attached().is_empty())
                {
                    // You're elsewhere: what didn't go waits there, marked in the sidebar.
                    self.unsent.insert(session.clone());
                }
            }
            // (A group chat's composer is its chat's; what's on its way, its conversation's.)
            ComposerEvent::Wrote | ComposerEvent::Outgoing => {}
        }
    }

    // MARK: The side panel

    pub fn panel(&self) -> &Entity<SidePanel> {
        &self.panel
    }

    pub fn is_panel_open(&self) -> bool {
        self.panel_open
    }

    pub fn panel_width(&self) -> Pixels {
        self.panel_width
    }

    /// Opens or closes the side panel; its width stays as it was.
    pub fn toggle_panel(&mut self, cx: &mut Context<Self>) {
        self.panel_open = !self.panel_open;
        let open = self.panel_open;
        self.place_panel(cx);
        self.panel.update(cx, |panel, cx| panel.set_shown(open, cx));
        cx.notify();
    }

    /// Opens the side panel on `tab`.
    pub fn show_panel(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.panel.update(cx, |panel, cx| panel.show(tab, cx));
        if !self.panel_open {
            self.toggle_panel(cx);
        }
    }

    /// The folder the panel shows: the open session's (its worktree's, for one in a worktree), or on
    /// the landing view the project a new one starts in.
    fn place_panel(&mut self, cx: &mut Context<Self>) {
        let place = match self.opened() {
            Some(session) => self.listing.facts(session).filter(|facts| !facts.kind.is_chat()).map(|facts| Place { directory: facts.directory.clone(), session: Some(session.to_owned()) }),
            None => match self.landing_place() {
                StartIn::Project(directory) => Some(Place { directory, session: None }),
                _ => None,
            },
        };
        self.panel.update(cx, |panel, cx| panel.set_place(place, cx));
    }

    /// A drag of the panel's edge started at `x`.
    pub fn start_resize(&mut self, x: Pixels, cx: &mut Context<Self>) {
        self.resizing = Some((x, self.panel_width));
        cx.notify();
    }

    /// The panel's edge dragged to `x` in a window `wide`: wider as it goes left, within its
    /// bounds, never leaving the conversation narrower than its narrowest.
    pub fn resize_to(&mut self, x: Pixels, wide: Pixels, cx: &mut Context<Self>) {
        let Some((from, width)) = self.resizing else { return };
        self.panel_width = (width + (from - x)).clamp(panel::NARROWEST, self.widest_panel(wide));
        cx.notify();
    }

    pub fn end_resize(&mut self, cx: &mut Context<Self>) {
        if self.resizing.take().is_some() {
            cx.notify();
        }
    }

    /// Its width as it first opened.
    pub fn reset_panel_width(&mut self, cx: &mut Context<Self>) {
        self.panel_width = panel::WIDTH;
        cx.notify();
    }

    /// The side panel, in the raised panel beside the conversation, with its edge to drag.
    fn panel_card(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        // In a window narrower than it was made for, the panel gives way before the conversation
        // does; its width is kept for a wider one.
        let width = self.panel_width.min(self.widest_panel(window.viewport_size().width));
        let resizing = self.resizing.is_some();
        div()
            .relative()
            .flex()
            .flex_none()
            .h_full()
            .border_l_1()
            .border_color(c.border)
            .child(div().w(width).h_full().flex().flex_col().overflow_hidden().child(self.panel.clone()))
            .child(
                // The edge to drag: wider than it looks, over the line between the two.
                div()
                    .id("panel-edge")
                    .debug_selector(|| "panel-edge".into())
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(-5.))
                    .w(px(8.))
                    .flex()
                    .justify_center()
                    .cursor_col_resize()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                        if event.click_count >= 2 {
                            this.reset_panel_width(cx);
                        } else {
                            this.start_resize(event.position.x, cx);
                        }
                    }))
                    .child(div().w(px(2.)).h_full().when(resizing, |d| d.bg(c.accent)).hover(|s| s.bg(c.border_strong))),
            )
            .into_any_element()
    }

    // MARK: Search and walking the sidebar

    pub fn search(&self) -> &Entity<TextField> {
        &self.search
    }

    /// Whether nothing's typed in the search (cheaply, as the sidebar's rows ask while drawn).
    pub fn search_is_empty(&self) -> bool {
        self.finds_for.is_empty()
    }

    pub fn search_text(&self, cx: &App) -> String {
        self.search.read(cx).text().to_owned()
    }

    /// What the box found in conversations for the search as it is now.
    pub fn finds(&self) -> &[Found] {
        &self.finds
    }

    pub fn is_finding(&self) -> bool {
        self.finding
    }

    pub fn find_failed(&self) -> Option<&str> {
        self.find_failed.as_deref()
    }

    /// Where the keyboard is in the sidebar's list: a line's key.
    pub fn walked(&self) -> Option<&str> {
        self.walked.as_deref()
    }

    pub fn list_focus(&self) -> &FocusHandle {
        &self.list_focus
    }

    pub fn list_scroll(&self) -> &ScrollHandle {
        &self.list_scroll
    }

    /// The search changed: names match at once; conversations once typing pauses, from three
    /// characters (the box searches them).
    fn search_changed(&mut self, cx: &mut Context<Self>) {
        self.walked = None;
        let query = self.search_text(cx).trim().to_owned();
        if query == self.finds_for {
            self.prune_session_popover(cx);
            cx.notify();
            return;
        }
        self.finds.clear();
        self.find_failed = None;
        self.finds_for = query.clone();
        self.find_task = None;
        self.finding = query.chars().count() >= SEARCHED_FROM;
        if self.finding {
            let client = self.client.clone();
            if self.live {
                self.find_task = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(SEARCH_PAUSE).await;
                    let asked = query.clone();
                    let found = cx.background_executor().spawn(async move { client.search(&asked, SEARCH_MOST) }).await;
                    this.update(cx, |this, cx| this.found(&query, found, cx)).ok();
                }));
            } else {
                // (Tests and shots: at once, as the box would answer.)
                let found = client.search(&query, SEARCH_MOST);
                self.found(&query, found, cx);
            }
        }
        self.prune_session_popover(cx);
        cx.notify();
    }

    /// What the box found for `query`, unless the search has changed since.
    fn found(&mut self, query: &str, found: kuvo_box::Result<Vec<Found>>, cx: &mut Context<Self>) {
        if query != self.finds_for {
            return;
        }
        self.finding = false;
        match found {
            Ok(finds) => self.finds = finds,
            Err(error) => self.find_failed = Some(error.to_string()),
        }
        self.prune_session_popover(cx);
        cx.notify();
    }

    pub fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |field, cx| field.set_text("", cx));
        self.walked = None;
        self.prune_session_popover(cx);
        window.focus(&self.search.focus_handle(cx), cx);
        cx.notify();
    }

    /// Puts the keyboard in the sidebar's search.
    pub fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.area != Area::Sessions {
            self.show_area(Area::Sessions, window, cx);
        }
        self.bring_out_sidebar(window);
        self.keep_place(cx);
        self.walked = None;
        window.focus(&self.search.focus_handle(cx), cx);
        cx.notify();
    }

    /// Steps the keyboard through the sidebar's list, `by` one down or up. Up from its first row
    /// goes back to the search.
    pub fn walk(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let lines = sidebar::lines(self, cx);
        let walkable: Vec<usize> = lines.iter().enumerate().filter(|(_, line)| line.walkable()).map(|(ix, _)| ix).collect();
        if walkable.is_empty() {
            return;
        }
        let at = self.walked.as_ref().and_then(|key| walkable.iter().position(|ix| &lines[*ix].key() == key));
        let next = match (at, by > 0) {
            (None, true) => Some(0),
            (None, false) => Some(walkable.len() - 1),
            (Some(at), true) => Some((at + 1).min(walkable.len() - 1)),
            (Some(0), false) => None,
            (Some(at), false) => Some(at - 1),
        };
        match next {
            Some(next) => {
                let ix = walkable[next];
                self.walked = Some(lines[ix].key());
                self.list_scroll.scroll_to_item(ix);
                window.focus(&self.list_focus, cx);
            }
            None => {
                self.walked = None;
                window.focus(&self.search.focus_handle(cx), cx);
            }
        }
        cx.notify();
    }

    /// Return: opens the session under the keyboard (from the search, the first one found), folds
    /// a project, or shows more of one.
    pub fn open_walked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let lines = sidebar::lines(self, cx);
        let line = match &self.walked {
            Some(key) => lines.iter().find(|line| &line.key() == key),
            // From the search: the first found, once something's searched for.
            None if !self.search_text(cx).trim().is_empty() => lines.iter().find(|line| line.walkable()),
            None => None,
        };
        match line.cloned() {
            Some(Line::Project(directory)) => {
                self.walked = Some(Line::Project(directory.clone()).key());
                self.toggle_project(&directory, cx);
            }
            Some(Line::More(directory)) => self.toggle_all(&directory, cx),
            Some(Line::Session { id, .. }) | Some(Line::Found { id, .. }) | Some(Line::Needs { id, .. }) => {
                self.walked = None;
                self.open_session(&id, window, cx);
                // (One already open: the keyboard goes back to it all the same.)
                if let Some(view) = self.conversation() {
                    window.focus(&view.focus_handle(cx), cx);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    /// Escape: out of the list, to the search; in the search, its words cleared, or (with none) back
    /// to the conversation or the landing view.
    pub fn leave_walk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.walked.take().is_some() {
            window.focus(&self.search.focus_handle(cx), cx);
        } else if !self.search_text(cx).is_empty() {
            self.search.update(cx, |field, cx| field.set_text("", cx));
        } else {
            match self.conversation() {
                Some(view) => window.focus(&view.focus_handle(cx), cx),
                None => window.focus(&self.landing.focus_handle(cx), cx),
            }
        }
        cx.notify();
    }

    // MARK: The rail and its areas

    pub fn area(&self) -> Area {
        self.area
    }

    pub fn rings(&self) -> &Entity<Rings> {
        &self.rings
    }

    /// How many things wait on you in each area, as the box counts them.
    pub fn waiting(&self) -> Waiting {
        self.waiting
    }

    /// Your computers as this app reaches them, for the switcher.
    pub fn switching(&self) -> Option<&crate::home::Switching> {
        self.switching.as_ref()
    }

    pub fn set_switching(&mut self, switching: Option<crate::home::Switching>, cx: &mut Context<Self>) {
        if self.switching != switching {
            // Why this app isn't paired, whole, on the Computers pages (the switcher's line is short).
            let pairing = switching.as_ref().and_then(|switching| match &switching.pairing {
                kuvo_box::boxes::Pairing::Waiting(why) | kuvo_box::boxes::Pairing::Refused(why) => Some(("This app doesn’t reach your other computers yet".to_owned(), why.clone())),
                kuvo_box::boxes::Pairing::Removed(why) => Some(("This app was removed from your devices".to_owned(), format!("{why} To add it again, choose Connect This App Again in the computer menu at the top."))),
                _ => None,
            });
            if let Some(page) = self.area_view(Area::Computers).and_then(|view| view.read(cx).computers_page().cloned()) {
                page.update(cx, |page, cx| page.set_pairing(pairing, cx));
            }
            self.switching = switching;
            cx.notify();
        }
        // (The box that lists them may have changed with them, `set_lister`: handed on each time.)
        self.sync_offers(cx);
    }

    /// Your computers, as the box lists them.
    pub fn computers(&self) -> &Reading<Vec<Computer>> {
        &self.computers
    }

    /// An area's sidebar view (none for Sessions, which the workspace draws itself).
    pub fn area_view(&self, area: Area) -> Option<&Entity<AreaView>> {
        self.areas.iter().find(|(of, _)| *of == area).map(|(_, view)| view)
    }

    /// Whether the area's sidebar shows now: in a wide window unless folded by its button, in a
    /// narrow one only once brought out.
    pub fn sidebar_shown(&self) -> bool {
        if self.narrow { self.shown_narrow } else { !self.folded }
    }

    pub fn is_narrow(&self) -> bool {
        self.narrow
    }

    /// Opens `area`, its sidebar shown; Sessions comes back to the conversation (or landing view)
    /// it showed.
    fn queue_computer(&self) -> String {
        if self.client.here() { return "nearest".into(); }
        self.switching.as_ref().and_then(|switching| switching.shown.clone()).unwrap_or_else(|| "nearest".into())
    }

    fn queue_needs_you_count(&self) -> usize {
        if self.queue_badge_loaded { crate::yours::queue_badge(&self.queue_badge_items, None) }
        else { self.queue_waiting.unwrap_or(0) }
    }

    fn queue_opened(&mut self, cx: &mut Context<Self>) {
        let at = crate::yours::queue_watermark(&self.queue_badge_items);
        crate::yours::change(crate::yours::Change::QueueOpened(self.queue_computer(), at, self.queue_needs_you_count()), cx);
        self.queue_badge_open_pending = true;
    }

    fn take_queue_badge(&mut self, said: kuvo_box::work_queue::QueueSaid, cx: &mut Context<Self>) {
        self.queue_badge_loaded = true;
        self.queue_badge_items = said.items;
        if self.queue_badge_open_pending {
            self.queue_badge_open_pending = false;
            let at = crate::yours::queue_watermark(&self.queue_badge_items);
            crate::yours::change(crate::yours::Change::QueueOpened(self.queue_computer(), at, self.queue_needs_you_count()), cx);
        }
        cx.notify();
    }

    pub fn queue_badge(&self, cx: &App) -> usize {
        if self.queue_badge_open_pending { return 0; }
        let yours = crate::yours::now(cx);
        if !self.queue_badge_loaded {
            return crate::yours::queue_count_badge(self.queue_waiting.unwrap_or(0), yours.queue_opened_count.get(&self.queue_computer()).copied());
        }
        let opened = yours.queue_last_opened.get(&self.queue_computer()).copied();
        crate::yours::queue_badge(&self.queue_badge_items, opened)
    }

    pub(crate) fn open_queue_reference(&mut self, item: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(queue) = self.area_view(Area::Queue).cloned() {
            queue.update(cx, |queue, cx| queue.select(Some(format!("item:{item}")), cx));
            self.show_area(Area::Queue, window, cx);
        }
    }

    pub fn show_area(&mut self, area: Area, window: &mut Window, cx: &mut Context<Self>) {
        if area == Area::Queue { self.queue_opened(cx); }
        self.area = area;
        self.bring_out_sidebar(window);
        match area {
            Area::Sessions => {
                let nav = self.opened().map_or(Nav::Landing, |session| Nav::Session(session.to_owned()));
                self.record(nav, cx);
                match self.conversation() {
                    Some(view) => window.focus(&view.focus_handle(cx), cx),
                    None => window.focus(&self.landing.focus_handle(cx), cx),
                }
            }
            _ => {
                if let Some(view) = self.area_view(area).cloned() {
                    let picked = view.read(cx).selected().map(str::to_owned);
                    // (Bots comes back to the conversation it showed, as Sessions does.)
                    match self.bot_chat().map(|(bot, _)| bot.to_owned()) {
                        Some(bot) => self.record(Nav::BotChat { bot }, cx),
                        None => self.record(Nav::Area(area, picked), cx),
                    }
                    view.update(cx, |view, cx| view.shown(cx));
                }
                // Computers opens on the list as it is now, where it isn't followed.
                if area == Area::Computers && self._computers_follow.is_none() {
                    self.read_computers(cx);
                }
                // The keyboard stays in the window, where the areas' keys are (in a bot's
                // conversation, in it).
                match self.bot_chat() {
                    Some((_, view)) => window.focus(&view.focus_handle(cx), cx),
                    None => window.focus(&self.focus, cx),
                }
            }
        }
        self.prune_session_popover(cx);
        self.show_composer_on_screen(cx);
        cx.notify();
    }

    /// Shows the area's sidebar: unfolded in a wide window, brought out in a narrow one (where it
    /// folds again once the window changes width class).
    fn bring_out_sidebar(&mut self, window: &Window) {
        self.narrow = window.viewport_size().width < NARROW;
        if self.narrow {
            self.shown_narrow = true;
        } else {
            self.folded = false;
        }
    }

    /// A press on an area's rail button: the area open folds its sidebar away (or brings it back);
    /// another opens.
    pub fn rail_pressed(&mut self, area: Area, window: &mut Window, cx: &mut Context<Self>) {
        if area != self.area {
            self.show_area(area, window, cx);
            return;
        }
        if area == Area::Queue {
            let at = crate::yours::queue_watermark(&self.queue_badge_items);
            crate::yours::change(crate::yours::Change::QueueOpened(self.queue_computer(), at, self.queue_needs_you_count()), cx);
        }
        self.toggle_sidebar(window, cx);
    }

    /// Folds the open area's sidebar away, or brings it back (its rail button pressed again, or
    /// View › Show Sidebar).
    pub fn toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // (The window as it is now, should it have changed since it was last drawn.)
        self.narrow = window.viewport_size().width < NARROW;
        if self.narrow {
            self.shown_narrow = !self.shown_narrow;
        } else {
            self.folded = !self.folded;
            self.keep_place(cx);
        }
        self.prune_session_popover(cx);
        // Folded with the keyboard in it (its search, its list): the keyboard goes to the main view.
        if !self.sidebar_shown() && (self.search.focus_handle(cx).is_focused(window) || self.list_focus.is_focused(window)) {
            self.walked = None;
            match self.conversation() {
                Some(view) if self.area == Area::Sessions => window.focus(&view.focus_handle(cx), cx),
                None if self.area == Area::Sessions => window.focus(&self.landing.focus_handle(cx), cx),
                _ => window.focus(&self.focus, cx),
            }
        }
        cx.notify();
    }

    /// Settings, on a place there (its key; a section's name alone is the computer shown's).
    pub fn show_setting(&mut self, section: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.area_view(Area::Settings).cloned() {
            view.update(cx, |view, cx| view.select(Some(section.to_owned()), cx));
        }
        self.show_area(Area::Settings, window, cx);
    }

    fn area_picked(&mut self, view: &Entity<AreaView>, window: &mut Window, cx: &mut Context<Self>) {
        let (area, picked) = view.read_with(cx, |view, _| (view.area(), view.selected().map(str::to_owned)));
        if area == Area::Bots {
            self.restoring.bot = None;
        }
        if area == Area::Bots {
            if let Some(bot) = picked.as_deref().and_then(|key| key.strip_prefix("bot:")) {
                if let Some(session) = self.bot_session(bot, cx) {
                    self.open_bot_chat(bot, &session, None, window, cx);
                    return;
                }
            }
        }
        // Non-bot rows in Bots show their page.
        if area == Area::Bots {
            self.in_bot_chat = false;
            self.show_composer_on_screen(cx);
        }
        self.record(Nav::Area(area, picked), cx);
        cx.notify();
    }

    /// Asks the box what waits on you.
    fn read_waiting(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        if !self.live {
            let read = client.waiting();
            self.take_waiting(read, cx);
            return;
        }
        // One read at a time: changes meanwhile make one more once it's back.
        if self.waiting_task.is_some() {
            self.waiting_again = true;
            return;
        }
        self.waiting_task = Some(cx.spawn(async move |this, cx| {
            let read = cx.background_executor().spawn(async move { client.waiting() }).await;
            this.update(cx, |this, cx| {
                this.waiting_task = None;
                this.take_waiting(read, cx);
                if std::mem::take(&mut this.waiting_again) {
                    this.read_waiting(cx);
                }
            })
            .ok();
        }));
    }

    fn read_queue_badge(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        if !self.live {
            match client.work_queue_view() {
                Ok(said) => self.take_queue_badge(said, cx),
                Err(_) => self.queue_badge_open_pending = false,
            }
        } else {
            self.queue_badge_task = Some(cx.spawn(async move |this, cx| {
                let read = cx.background_executor().spawn(async move { client.work_queue_view() }).await;
                this.update(cx, |this, cx| {
                    match read {
                        Ok(said) => this.take_queue_badge(said, cx),
                        Err(_) => this.queue_badge_open_pending = false,
                    }
                }).ok();
            }));
        }
    }

    fn take_waiting(&mut self, read: kuvo_box::Result<Waiting>, cx: &mut Context<Self>) {
        if let Ok(waiting) = &read {
            // Waiting has counts, not item stamps. The first delivery is a baseline;
            // only a changed queue count warrants an off-screen read for new items.
            let previous = self.queue_waiting.replace(waiting.queue);
            if previous.is_some_and(|count| count != waiting.queue) {
                self.read_queue_badge(cx);
            }
        }
        // Couldn't be read now: what was said stands until it can.
        if let Ok(waiting) = read
            && self.waiting != waiting
        {
            self.waiting = waiting;
            cx.notify();
        }
    }

    /// Asks the box for your computers (in the app, on another thread; in tests and shots, at once).
    fn read_computers(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        if !self.live {
            let read = client.computers();
            self.take_computers(read, cx);
            return;
        }
        self.computers_task = Some(cx.spawn(async move |this, cx| {
            let read = cx.background_executor().spawn(async move { client.computers() }).await;
            this.update(cx, |this, cx| this.take_computers(read, cx)).ok();
        }));
    }

    /// What the box said of your computers: kept, and handed to the Computers area.
    fn take_computers(&mut self, read: kuvo_box::Result<Vec<Computer>>, cx: &mut Context<Self>) {
        let computers = areas::reading(read, &self.computers);
        if self.computers != computers {
            self.computers = computers.clone();
            if let Some(run) = self.first_run().cloned() {
                let computers = computers.clone();
                run.update(cx, |run, cx| run.set_computers(computers, cx));
            }
            // (Bots: a bot on a computer that's away is drawn offline.)
            for area in [Area::Computers, Area::Settings, Area::Bots] {
                if let Some(view) = self.area_view(area).cloned() {
                    let computers = computers.clone();
                    view.update(cx, |view, cx| view.set_computers(computers, cx));
                }
            }
            cx.notify();
        }
    }

    // MARK: Where the window was

    /// Opens where this computer's window was last time, as kept under `key` in your settings
    /// (`yours::Place`), and keeps where it goes from now on. Called once, as the window makes it.
    pub fn restore(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        let spot = crate::yours::place(cx).computers.get(&key).cloned().unwrap_or_default();
        self.place_key = Some(key);
        self.folded = spot.sidebar_folded;
        self.kept_area = spot.area.clone();
        self.restoring_now = true;
        for area in [Area::Bots, Area::Queue, Area::Computers] {
            let (Some(picked), Some(view)) = (spot.picked.get(area.slug()), self.area_view(area).cloned()) else { continue };
            view.update(cx, |view, cx| view.select(Some(picked.clone()), cx));
            self.restoring.picks.push((area, picked.clone()));
        }
        self.restoring.session = spot.session;
        self.restoring.bot = spot.bot;
        let area = spot.area.as_deref().and_then(|slug| Area::ALL.into_iter().find(|area| area.slug() == slug && !matches!(area, Area::Settings | Area::Sessions)));
        if let Some(area) = area {
            self.show_area_as_kept(area, window, cx);
        }
        self.restoring_now = false;
        if self.listing.is_read() || matches!(self.lists, Lists::Failed(_) | Lists::Held) {
            self.restore_session(window, cx);
        }
        self.restore_bot(window, cx);
        self.check_picks(cx);
        cx.notify();
    }

    /// Shows `area` with the sidebar as it was (restoring isn't you asking for it).
    fn show_area_as_kept(&mut self, area: Area, window: &mut Window, cx: &mut Context<Self>) {
        let (folded, shown_narrow) = (self.folded, self.shown_narrow);
        self.show_area(area, window, cx);
        (self.folded, self.shown_narrow) = (folded, shown_narrow);
    }

    /// The session list is read (or can't be): last time's conversation opens again where it's
    /// still there, behind whichever area is in front now; gone, or its box not reached, Sessions
    /// stays on its own view.
    fn restore_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.restoring.session.take() else { return };
        if !self.listing.is_read() {
            return cx.notify();
        }
        // (Archived, or a bot's, it's still there: only one the list no longer has is gone.)
        if self.listing.facts(&session).is_none() {
            // (Gone for good: no longer kept.)
            self.keep_place(cx);
            return cx.notify();
        }
        // Behind another area, or with the keyboard somewhere of yours meanwhile (a search), it
        // opens without taking the keyboard or the window from you.
        let (area, folded, shown_narrow) = (self.area, self.folded, self.shown_narrow);
        let yours = window.focused(cx).filter(|focused| *focused != self.focus && *focused != self.landing.focus_handle(cx));
        let history = (area != Area::Sessions).then(|| (self.history.clone(), self.at));
        self.restoring_now = true;
        self.open_session_now(&session, window, cx);
        self.restoring_now = false;
        if let Some((history, at)) = history {
            (self.history, self.at) = (history, at);
            self.area = area;
            self.show_composer_on_screen(cx);
            match self.bot_chat() {
                Some((_, view)) => window.focus(&view.focus_handle(cx), cx),
                None => window.focus(&self.focus, cx),
            }
        }
        (self.folded, self.shown_narrow) = (folded, shown_narrow);
        if let Some(yours) = yours {
            window.focus(&yours, cx);
        }
        cx.notify();
    }

    /// Bots' list is read: last time's bot conversation opens again where Bots is in front and
    /// the bot's still there; gone, Bots shows its own page.
    fn restore_bot(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bot) = self.restoring.bot.clone() else { return };
        let Some(view) = self.area_view(Area::Bots).cloned() else { return };
        match view.read(cx).bots() {
            Reading::NotYet => return,
            // (Not read: what Bots shows says so.)
            Reading::Failed(_) => {
                self.restoring.bot = None;
                return;
            }
            Reading::Ready(_) if self.area != Area::Bots => return,
            Reading::Ready(_) => {}
        }
        self.restoring.bot = None;
        match self.bot_session(&bot, cx) {
            Some(session) => {
                let yours = window.focused(cx).filter(|focused| *focused != self.focus && *focused != self.landing.focus_handle(cx));
                self.restoring_now = true;
                let (folded, shown_narrow) = (self.folded, self.shown_narrow);
                self.open_bot_chat(&bot, &session, None, window, cx);
                (self.folded, self.shown_narrow) = (folded, shown_narrow);
                self.restoring_now = false;
                if let Some(yours) = yours {
                    window.focus(&yours, cx);
                }
            }
            None => {
                let key = format!("bot:{bot}");
                if view.read(cx).selected() == Some(key.as_str()) {
                    view.update(cx, |view, cx| view.select(None, cx));
                }
                self.keep_place(cx);
            }
        }
        cx.notify();
    }

    /// Rows picked again as the window opened: one its list no longer has is unpicked (the list
    /// shows instead of a page saying it's gone). One picked anew since is left as it is.
    fn check_picks(&mut self, cx: &mut Context<Self>) {
        if self.restoring.picks.is_empty() {
            return;
        }
        let mut left = Vec::new();
        let mut unpicked = false;
        for (area, key) in std::mem::take(&mut self.restoring.picks) {
            let Some(view) = self.area_view(area).cloned() else { continue };
            if view.read(cx).selected() != Some(key.as_str()) {
                continue;
            }
            match view.read(cx).still_there(&key) {
                None => left.push((area, key)),
                Some(true) => {}
                Some(false) => {
                    view.update(cx, |view, cx| view.select(None, cx));
                    unpicked = true;
                }
            }
        }
        self.restoring.picks = left;
        if unpicked {
            self.keep_place(cx);
            cx.notify();
        }
    }

    /// Keeps where the window is now in your settings, for the next time it opens: what's still
    /// to be restored is kept as it was.
    fn keep_place(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.place_key.clone() else { return };
        if self.area != Area::Settings {
            self.kept_area = Some(self.area.slug().to_owned());
        }
        // (What only lasts while it's open isn't a place to open on: New Bot, Add a Machine.)
        let keeps = |key: &str| !matches!(key, "new" | "add" | "new-group") && !key.starts_with("edit-group:");
        let mut picked = std::collections::BTreeMap::new();
        for area in [Area::Bots, Area::Queue, Area::Computers] {
            if let Some(selected) = self.area_view(area).and_then(|view| view.read(cx).selected()).filter(|key| keeps(key)) {
                picked.insert(area.slug().to_owned(), selected.to_owned());
            }
        }
        let session = self.restoring.session.clone().or_else(|| self.opened().map(str::to_owned));
        let bot = self.restoring.bot.clone().or_else(|| self.bot_chat.as_ref().filter(|_| self.in_bot_chat).map(|chat| chat.bot.clone()));
        let spot = crate::yours::Spot { sidebar_folded: self.folded, area: self.kept_area.clone(), session, bot, picked };
        crate::yours::keep(crate::yours::Moved::Spot(key, spot), cx);
    }

    /// Whether last time's conversation is still to open (nothing's drawn in its place meanwhile).
    pub fn is_restoring(&self) -> bool {
        self.restoring.session.is_some()
    }

    // MARK: Back and forward

    /// Notes going to `nav`, unless that's where the window is (or it's going back or forward).
    /// Where the window is now is kept as it settles (`keep_place`).
    fn record(&mut self, nav: Nav, cx: &mut Context<Self>) {
        // A move of yours in Sessions: the conversation it was to open again goes unopened.
        if !self.restoring_now && matches!(nav, Nav::Session(_) | Nav::Landing) {
            self.restoring.session = None;
        }
        let this = cx.entity().downgrade();
        cx.defer(move |cx| drop(this.update(cx, |this, cx| this.keep_place(cx))));
        // (Opening where the window was isn't steps of yours to go back through.)
        if self.restoring_now {
            self.history = vec![nav];
            self.at = 0;
            return;
        }
        if self.history.get(self.at) == Some(&nav) {
            return;
        }
        self.history.truncate(self.at + 1);
        self.history.push(nav);
        if self.history.len() > HISTORY {
            self.history.remove(0);
        }
        self.at = self.history.len() - 1;
    }

    pub fn can_go_back(&self) -> bool {
        self.at > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.at + 1 < self.history.len()
    }

    pub fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.can_go_back() {
            self.go(self.at - 1, window, cx);
        }
    }

    pub fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.can_go_forward() {
            self.go(self.at + 1, window, cx);
        }
    }

    /// Goes to the place noted at `at` without noting it again. A session gone from the list
    /// since is passed over, as a way back to nothing.
    fn go(&mut self, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        // Sessions gone from the list since are dropped from the history (and the places that
        // then repeat), so the press goes on to the next place there still is.
        let forward = at > self.at;
        let here = self.history[self.at].clone();
        // (Gone: deleted, or no longer shown, as an archived one.)
        // (A bot's conversation goes with its bot: one the box's list no longer has.)
        let bots: Option<Vec<String>> = self.area_view(Area::Bots).and_then(|view| match view.read(cx).bots() {
            Reading::Ready(bots) => Some(bots.bots.iter().map(|bot| bot.id.clone()).collect()),
            _ => None,
        });
        let gone = |nav: &Nav, this: &Self| match nav {
            Nav::Session(session) => this.listing.is_read() && kuvo_box::sessions::place(this.listing.facts(session)).is_none() && !this.started.contains_key(session),
            Nav::BotChat { bot } => bots.as_ref().is_some_and(|bots| !bots.contains(bot)),
            // (Its page goes with it too.)
            Nav::Area(Area::Bots, Some(key)) => key.strip_prefix("bot:").is_some_and(|bot| bots.as_ref().is_some_and(|bots| !bots.iter().any(|each| each == bot))),
            _ => false,
        };
        let mut kept: Vec<Nav> = Vec::with_capacity(self.history.len());
        let mut target = None;
        let mut current = 0;
        for (ix, nav) in self.history.iter().enumerate() {
            if ix != self.at && gone(nav, self) {
                continue;
            }
            if kept.last() == Some(nav) {
                if ix == self.at {
                    current = kept.len() - 1;
                }
                continue;
            }
            if ix == self.at {
                current = kept.len();
            }
            if ix == at {
                target = Some(kept.len());
            }
            kept.push(nav.clone());
        }
        self.history = kept;
        self.at = current;
        let at = match target {
            Some(at) if at != current => at,
            // The place asked for went: the next one on that side, if any.
            _ if forward && current + 1 < self.history.len() => current + 1,
            _ if !forward && current > 0 => current - 1,
            _ => {
                cx.notify();
                return;
            }
        };
        debug_assert!(self.history[current] == here);
        let nav = self.history[at].clone();
        let history = std::mem::take(&mut self.history);
        match &nav {
            Nav::Landing => self.new_session(None, window, cx),
            Nav::Session(session) => self.open_session(session, window, cx),
            Nav::Area(area, picked) => {
                if *area == Area::Bots {
                    self.in_bot_chat = false;
                }
                if let Some(view) = self.area_view(*area).cloned() {
                    view.update(cx, |view, cx| view.select(picked.clone(), cx));
                }
                self.show_area(*area, window, cx);
            }
            // Its conversation as the box lists it now (a change of model may have given it a new one).
            Nav::BotChat { bot } => match self.bot_session(bot, cx) {
                Some(session) => self.open_bot_chat(bot, &session, None, window, cx),
                None => {
                    self.in_bot_chat = false;
                    if let Some(view) = self.area_view(Area::Bots).cloned() {
                        view.update(cx, |view, cx| view.select(Some(format!("bot:{bot}")), cx));
                    }
                    self.show_area(Area::Bots, window, cx);
                }
            },
        }
        self.history = history;
        self.at = at;
        cx.notify();
    }

    /// A bot's conversation in Bots: a strip saying whose it is, with the way back to its page,
    /// over the conversation.
    fn bot_chat_view(&self, bot: &str, chat: &Entity<ConversationView>, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let listed = self.area_view(Area::Bots).and_then(|view| match view.read(cx).bots() {
            Reading::Ready(bots) => bots.bots.iter().find(|each| each.id == bot).cloned(),
            _ => None,
        });
        let strip = div()
            .debug_selector(|| "bot-chat-strip".into())
            .flex()
            .flex_none()
            .items_center()
            .gap(space::S2)
            .h(px(44.))
            .px(space::S3)
            .border_b_1()
            .border_color(c.border)
            .child(Button::icon_only("bot-chat-settings", IconName::Settings, "Bot Settings").ghost().size(ButtonSize::Small).icon_size(kuvo_ui::IconSize::Medium).tooltip("Bot Settings").on_click(cx.listener(|this, _, window, cx| this.close_bot_chat(window, cx))))
            .children(listed.as_ref().map(|bot| areas::bot_face(&bot.id, &bot.look, px(22.))))
            .children(listed.as_ref().map(|bot| div().flex_none().text_ui(TypeSize::Small).medium().text_color(c.text).child(bot.name.clone())))
            .children(listed.as_ref().map(|bot| {
                let words = bot.job.clone();
                div().flex_1().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text_muted).child(words)
            }));
        div().debug_selector(|| "bot-chat".into()).size_full().flex().flex_col().child(strip).child(div().flex_1().min_h_0().child(chat.clone().cached(gpui::StyleRefinement::default().size_full()))).into_any_element()
    }

    /// The main view's title and what's beside it, as the title bar shows them: a conversation's
    /// title and its project, or the page an area shows.
    pub fn title(&self, cx: &App) -> (Option<SharedString>, Option<SharedString>) {
        if self.area != Area::Sessions {
            let Some(view) = self.area_view(self.area) else { return (None, None) };
            let title = view.read(cx).title(cx);
            let area = self.area.name();
            let beside = (title.as_ref() != area).then(|| SharedString::from(area));
            return (Some(title), beside);
        }
        match self.opened() {
            Some(session) => {
                let facts = self.listing.facts(session);
                let title = facts.and_then(|facts| facts.title.clone()).filter(|title| !title.trim().is_empty()).unwrap_or_else(|| "New Session".into());
                // (One started here, before the list has it: the project its box said.)
                let project = facts.and_then(|facts| facts.project.clone()).or_else(|| self.started.get(session).cloned().flatten()).map(|project| self.project_name(&project));
                (Some(title.into()), project.map(SharedString::from))
            }
            None => (None, None),
        }
    }

    /// Whether `session` is marked archived in the box's list.
    pub fn session_is_archived(&self, session: &str) -> bool {
        self.listing.facts(session).is_some_and(Facts::is_archived)
    }

    /// Whether the currently open session is archived.
    pub fn opened_is_archived(&self) -> bool {
        self.opened().is_some_and(|session| self.session_is_archived(session))
    }

    pub fn archived_filter(&self) -> bool {
        self.archived_filter
    }

    pub fn toggle_archived_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_archived_filter(!self.archived_filter, window, cx);
    }

    pub fn set_archived_filter(&mut self, on: bool, _window: &mut Window, cx: &mut Context<Self>) {
        if self.archived_filter == on {
            return;
        }
        self.archived_filter = on;
        self.walked = None;
        self.session_menu.hovered_row = None;
        self.session_menu.action_error = None;
        self.session_menu.active_popover = None;
        let placeholder = if on { "Search archived" } else { "Search" };
        self.search.update(cx, |field, cx| {
            field.set_placeholder(placeholder);
            cx.notify();
        });
        cx.notify();
    }

    pub fn session_menu(&self) -> &SessionMenu {
        &self.session_menu
    }

    pub fn session_menu_mut(&mut self) -> &mut SessionMenu {
        &mut self.session_menu
    }

    pub fn set_hovered_row(&mut self, key: Option<String>, cx: &mut Context<Self>) {
        if self.session_menu.hovered_row != key {
            self.session_menu.hovered_row = key.clone();
            if let Some(id) = key.as_deref().and_then(session_menu::session_from_row_key) {
                self.ensure_row_actions(id, true, cx);
            }
            cx.notify();
        }
    }

    fn ensure_row_actions(&mut self, session: &str, retry_failed: bool, cx: &mut Context<Self>) {
        if self.client.gate().is_held() {
            return;
        }
        if let Some(entry) = self.session_menu.row_actions.get(session) {
            if entry.fetching.is_some() {
                return;
            }
            if entry.failed && !retry_failed && !entry.stale {
                return;
            }
            if entry.actions.is_some() && !entry.stale && !entry.failed {
                return;
            }
        }
        if !self.live {
            let result = self.client.session_actions(session);
            let seq = self.session_menu.begin_fetch(session);
            self.session_menu.finish_fetch(session, seq, result);
            return;
        }
        let id = session.to_owned();
        let client = self.client.clone();
        let seq = self.session_menu.begin_fetch(&id);
        self.session_menu.action_tasks.push(cx.spawn(async move |this, cx| {
            let fetch_id = id.clone();
            let result = cx
                .background_executor()
                .spawn(async move { client.session_actions(&fetch_id) })
                .await;
            this.update(cx, |this, cx| {
                if this.session_menu.finish_fetch(&id, seq, result) {
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn refresh_active_row_actions(&mut self, cx: &mut Context<Self>) {
        let mut refresh_ids: Vec<String> = Vec::new();
        if let Some(id) = self
            .session_menu
            .hovered_row
            .as_deref()
            .and_then(session_menu::session_from_row_key)
        {
            refresh_ids.push(id.to_owned());
        }
        for (key, handle) in &self.session_menu.row_menus {
            if handle.is_open()
                && let Some(id) = session_menu::session_from_row_key(key)
                && !refresh_ids.iter().any(|existing| existing == id)
            {
                refresh_ids.push(id.to_owned());
            }
        }
        if let Some(popover) = &self.session_menu.active_popover
            && matches!(popover.anchor(), PopoverAnchor::Row(_))
        {
            let id = popover.session();
            if !refresh_ids.iter().any(|existing| existing == id) {
                refresh_ids.push(id.to_owned());
            }
        }
        for id in refresh_ids {
            self.ensure_row_actions(&id, false, cx);
        }
    }

    fn prune_session_popover(&mut self, cx: &App) {
        let Some(popover) = &self.session_menu.active_popover else {
            return;
        };
        let keep = if self.area != Area::Sessions || self.client.gate().is_held() {
            false
        } else {
            match popover.anchor() {
                PopoverAnchor::TitleBar => self.opened().is_some_and(|open| {
                    open == popover.session() && self.listing.facts(open).is_some()
                }),
                PopoverAnchor::Row(row_key) => {
                    self.sidebar_shown()
                        && sidebar::lines(self, cx)
                            .iter()
                            .any(|line| &line.key() == row_key)
                }
            }
        };
        if !keep {
            self.session_menu.active_popover = None;
        }
    }

    fn clear_row_target(&mut self, session: &str) {
        if self
            .walked
            .as_deref()
            .and_then(session_menu::session_from_row_key)
            == Some(session)
        {
            self.walked = None;
        }
        if self
            .session_menu
            .hovered_row
            .as_deref()
            .and_then(session_menu::session_from_row_key)
            == Some(session)
        {
            self.session_menu.hovered_row = None;
        }
    }

    pub fn clear_action_error(&mut self, cx: &mut Context<Self>) {
        if self.session_menu.action_error.take().is_some() {
            cx.notify();
        }
    }

    pub fn open_row_menu(&mut self, session: &str, key: &str, window: &mut Window, cx: &mut Context<Self>) -> Entity<Menu> {
        let anchor = PopoverAnchor::Row(key.to_owned());
        if !self.live {
            let result = self.client.session_actions(session);
            if let Ok(actions) = &result {
                self.session_menu.cache_actions(session, actions.clone());
            }
            return session_menu::menu_from_actions(result, session.to_owned(), anchor, cx.entity().downgrade(), cx);
        }
        session_menu::open_actions_menu(
            self.client.clone(),
            self.live,
            session.to_owned(),
            anchor,
            cx.entity().downgrade(),
            window,
            cx,
        )
    }

    pub fn open_title_menu(&mut self, session: &str, window: &mut Window, cx: &mut Context<Self>) -> Entity<Menu> {
        if !self.live {
            let result = self.client.session_actions(session);
            if let Ok(actions) = &result {
                self.session_menu.cache_actions(session, actions.clone());
            }
            return session_menu::menu_from_actions(
                result,
                session.to_owned(),
                PopoverAnchor::TitleBar,
                cx.entity().downgrade(),
                cx,
            );
        }
        session_menu::open_actions_menu(
            self.client.clone(),
            self.live,
            session.to_owned(),
            PopoverAnchor::TitleBar,
            cx.entity().downgrade(),
            window,
            cx,
        )
    }

    fn refresh_list_now(&mut self, cx: &mut Context<Self>) {
        if let Ok(line) = self.client.session_list(None) {
            let mut listing = Listing::new();
            listing.take(line);
            self.take_list(ListUpdate::Listing(listing), cx);
        }
    }

    fn refresh_list_after_action(&mut self, cx: &mut Context<Self>) {
        if !self.live {
            self.refresh_list_now(cx);
            return;
        }
        if !self.client.streams() {
            let client = self.client.clone();
            self.tasks.push(cx.spawn(async move |this, cx| {
                let line = cx.background_executor().spawn(async move { client.session_list(None) }).await;
                if let Ok(line) = line {
                    this.update(cx, |this, cx| {
                        let mut listing = Listing::new();
                        listing.take(line);
                        this.take_list(ListUpdate::Listing(listing), cx);
                    })
                    .ok();
                }
            }));
        }
    }

    pub fn pin_session(&mut self, session: &str, pinned: bool, cx: &mut Context<Self>) {
        self.session_menu.action_error = None;
        let client = self.client.clone();
        let id = session.to_owned();
        if !self.live {
            match client.pin_session(&id, pinned) {
                Ok(_) => {
                    self.session_menu.invalidate_session(&id);
                    self.refresh_list_now(cx);
                    self.refresh_active_row_actions(cx);
                }
                Err(error) => {
                    self.session_menu.action_error = Some(error.to_string());
                    cx.notify();
                }
            }
            return;
        }
        self.session_menu.action_tasks.push(cx.spawn(async move |this, cx| {
            let pin_id = id.clone();
            let result = cx.background_executor().spawn(async move { client.pin_session(&pin_id, pinned) }).await;
            this.update(cx, |this, cx| match result {
                Ok(_) => {
                    this.session_menu.invalidate_session(&id);
                    this.refresh_list_after_action(cx);
                    this.refresh_active_row_actions(cx);
                }
                Err(error) => {
                    this.session_menu.action_error = Some(error.to_string());
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    pub fn archive_session(&mut self, session: &str, archived: bool, _window: &mut Window, cx: &mut Context<Self>) {
        self.session_menu.action_error = None;
        self.session_menu.active_popover = None;
        let client = self.client.clone();
        let id = session.to_owned();
        if !self.live {
            match client.archive_session(&id, archived) {
                Ok(_) => {
                    self.clear_row_target(&id);
                    self.session_menu.invalidate_session(&id);
                    self.refresh_list_now(cx);
                }
                Err(error) => {
                    self.session_menu.action_error = Some(error.to_string());
                    cx.notify();
                }
            }
            return;
        }
        self.session_menu.action_tasks.push(cx.spawn(async move |this, cx| {
            let archive_id = id.clone();
            let result = cx.background_executor().spawn(async move { client.archive_session(&archive_id, archived) }).await;
            this.update(cx, |this, cx| match result {
                Ok(_) => {
                    this.clear_row_target(&id);
                    this.session_menu.invalidate_session(&id);
                    this.refresh_list_after_action(cx);
                }
                Err(error) => {
                    this.session_menu.action_error = Some(error.to_string());
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    pub fn start_rename(&mut self, session: &str, anchor: PopoverAnchor, window: &mut Window, cx: &mut Context<Self>) {
        self.session_menu.action_error = None;
        if matches!(anchor, PopoverAnchor::Row(_)) {
            self.ensure_row_actions(session, false, cx);
        }
        let original = self
            .listing
            .facts(session)
            .and_then(|facts| facts.title.clone())
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_default();
        let target = session_menu::PopoverTarget {
            session: session.to_owned(),
            client: self.client.clone(),
            live: self.live,
            return_to: window.focused(cx),
            workspace: cx.entity().downgrade(),
        };
        let view = cx.new(|cx| RenamePopover::new(target, original, window, cx));
        self.session_menu.active_popover = Some(ActivePopover::Rename {
            session: session.to_owned(),
            anchor,
            view,
        });
        cx.notify();
    }

    pub fn run_delete_action(
        &mut self,
        session: &str,
        confirm: Option<Confirm>,
        anchor: PopoverAnchor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session_menu.action_error = None;
        if matches!(anchor, PopoverAnchor::Row(_)) {
            self.ensure_row_actions(session, false, cx);
        }
        let Some(confirm) = confirm else {
            self.session_menu.active_popover = None;
            self.session_menu.action_error = Some(
                kuvo_box::Error::BadAnswer("Kuvo didn’t say what deleting comes to.".into()).to_string(),
            );
            cx.notify();
            return;
        };
        let target = session_menu::PopoverTarget {
            session: session.to_owned(),
            client: self.client.clone(),
            live: self.live,
            return_to: window.focused(cx),
            workspace: cx.entity().downgrade(),
        };
        let view = cx.new(|cx| ConfirmDeletePopover::new(target, confirm, window, cx));
        self.session_menu.active_popover = Some(ActivePopover::Delete {
            session: session.to_owned(),
            anchor,
            view,
        });
        cx.notify();
    }

    pub fn close_session_popover(&mut self, return_to: Option<FocusHandle>, window: &mut Window, cx: &mut Context<Self>) {
        self.session_menu.active_popover = None;
        if let Some(target) = return_to {
            window.focus(&target, cx);
        }
        cx.notify();
    }

    pub fn close_session_popover_no_focus(&mut self, cx: &mut Context<Self>) {
        self.session_menu.active_popover = None;
        cx.notify();
    }

    pub fn finish_rename(&mut self, return_to: Option<FocusHandle>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = self.session_menu.active_popover.as_ref().map(|p| p.session().to_owned()) {
            self.session_menu.invalidate_session(&session);
        }
        self.session_menu.active_popover = None;
        self.refresh_list_after_action(cx);
        self.refresh_active_row_actions(cx);
        if let Some(target) = return_to {
            window.focus(&target, cx);
        }
        cx.notify();
    }

    pub fn finish_rename_no_focus(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = self.session_menu.active_popover.as_ref().map(|p| p.session().to_owned()) {
            self.session_menu.invalidate_session(&session);
        }
        self.session_menu.active_popover = None;
        self.refresh_list_after_action(cx);
        self.refresh_active_row_actions(cx);
        cx.notify();
    }

    pub fn finish_delete(&mut self, session: &str, return_to: Option<FocusHandle>, window: &mut Window, cx: &mut Context<Self>) {
        self.session_menu.active_popover = None;
        self.clear_row_target(session);
        self.session_menu.remove_actions(session);
        self.composers.remove(session);
        self.started.remove(session);
        self.unsent.remove(session);
        let was_open = self.opened() == Some(session);
        if was_open {
            self.open = Open::Landing;
            let landing = self.landing.clone();
            self.show_composer(Some(&landing), cx);
            self.place_panel(cx);
            window.focus(&self.landing.focus_handle(cx), cx);
        } else if let Some(target) = return_to {
            window.focus(&target, cx);
        }
        self.refresh_list_after_action(cx);
        cx.notify();
    }

    fn shortcut_target(&self, window: &Window, cx: &App) -> Option<(String, PopoverAnchor)> {
        if self.area != Area::Sessions || self.client.gate().is_held() {
            return None;
        }
        if let Some(key) = &self.walked {
            for line in sidebar::lines(self, cx) {
                if &line.key() == key {
                    let session = match line {
                        Line::Session { id, .. } | Line::Found { id, .. } | Line::Needs { id, .. } => id,
                        _ => return None,
                    };
                    return Some((session, PopoverAnchor::Row(key.clone())));
                }
            }
            return None;
        }
        if self.list_focus.is_focused(window) {
            return None;
        }
        self.opened().map(|id| (id.to_owned(), PopoverAnchor::TitleBar))
    }

    fn run_shortcut(&mut self, kind: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if self.session_menu.active_popover.is_some() {
            return;
        }
        let Some((session, anchor)) = self.shortcut_target(window, cx) else {
            return;
        };
        self.trigger_session_action(&session, kind, anchor, window, cx);
    }

    pub fn trigger_session_action(
        &mut self,
        session: &str,
        kind: &'static str,
        anchor: PopoverAnchor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session_menu.action_error = None;
        let client = self.client.clone();
        let session = session.to_owned();
        if !self.live {
            match client.session_actions(&session) {
                Ok(actions) => {
                    self.session_menu.cache_actions(&session, actions.clone());
                    self.apply_shortcut(kind, &session, anchor, actions, window, cx);
                }
                Err(error) => {
                    self.session_menu.action_error = Some(error.to_string());
                    cx.notify();
                }
            }
            return;
        }
        let window_handle: AnyWindowHandle = window.window_handle();
        let target_session = session.clone();
        self.session_menu.action_tasks.push(cx.spawn(async move |this, cx| {
            let fetch_session = target_session.clone();
            let result = cx.background_executor().spawn(async move { client.session_actions(&fetch_session) }).await;
            let _ = window_handle.update(cx, |_, window, cx| {
                this.update(cx, |this, cx| match result {
                    Ok(actions) => {
                        this.session_menu.cache_actions(&session, actions.clone());
                        this.apply_shortcut(kind, &session, anchor, actions, window, cx);
                    }
                    Err(error) => {
                        this.session_menu.action_error = Some(error.to_string());
                        cx.notify();
                    }
                })
                .ok();
            });
        }));
    }

    fn apply_shortcut(
        &mut self,
        kind: &str,
        session: &str,
        anchor: PopoverAnchor,
        actions: kuvo_box::session_actions::Actions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(held) = actions.held {
            self.session_menu.action_error = Some(held);
            cx.notify();
            return;
        }
        match kind {
            "rename" if actions.actions.iter().any(|action| action.id == "rename") => {
                self.start_rename(session, anchor, window, cx);
            }
            "archive" if actions.actions.iter().any(|action| action.id == "archive") => {
                self.archive_session(session, true, window, cx);
            }
            "delete" => {
                if let Some(delete) = actions.actions.into_iter().find(|action| action.id == "delete") {
                    self.run_delete_action(session, delete.confirm, anchor, window, cx);
                }
            }
            _ => {}
        }
    }

    /// How wide the area's sidebar and the rail are now, together.
    fn sidebars_width(&self) -> Pixels {
        rail::WIDTH + if self.sidebar_shown() { px(sidebar::WIDTH) } else { px(0.) }
    }

    /// The widest the side panel goes in a window `wide`: never leaving the conversation narrower
    /// than its narrowest (the rail, the area's sidebar and the panel's edge apart).
    fn widest_panel(&self, wide: Pixels) -> Pixels {
        let room = wide - self.sidebars_width() - panel::CONVERSATION_NARROWEST - px(8.);
        panel::WIDEST.min(room).max(panel::NARROWEST)
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let c = &theme.colors;
        // The first run takes the whole window until it's done; while the box is first being
        // asked whether it's been had, the window waits, empty, rather than show the app and
        // take it away again.
        if let Some(run) = self.first_run() {
            return div().size_full().child(run.clone());
        }
        if self.awaits_box() {
            // How the box stands, in the window's middle: being reached, or why it can't be.
            let why = match (&self.box_state, &self.welcome_why) {
                (BoxState::Unreachable(why), _) => Some(why.clone()),
                (_, why) => why.clone(),
            };
            // (One that can't be reached says so, with Try Again and another computer to switch to.)
            let blocked = self.blocked();
            let words: AnyElement = match (blocked, why) {
                (Some(blocked), _) => crate::blocked::render(&blocked, cx),
                (None, Some(why)) => div().debug_selector(|| "window-waiting-why".into()).max_w(px(420.)).text_color(c.text_muted).child(why.clone()).into_any_element(),
                _ => kuvo_ui::small::LoadingLabel::new("window-starting", "Starting…").into_any_element(),
            };
            return div()
                .debug_selector(|| "window-waiting".into())
                .size_full()
                .flex()
                .flex_col()
                .bg(c.background)
                .text_ui(TypeSize::Small)
                .child(div().id("window-waiting-title-bar").h(px(title_bar::HEIGHT)).flex_none().on_mouse_down(MouseButton::Left, |event, window, _| {
                    if event.click_count == 1 {
                        window.start_window_move();
                    } else if event.click_count == 2 {
                        window.titlebar_double_click();
                    }
                }))
                .child(div().flex_1().flex().items_center().justify_center().pb(px(96.)).child(words));
        }
        if std::mem::take(&mut self.focus_landing) {
            window.focus(&self.landing.focus_handle(cx), cx);
        }
        // A narrow window folds the area's sidebar away; brought out there, it folds again once
        // the window has been wide and narrows again.
        let narrow = window.viewport_size().width < NARROW;
        if self.narrow != narrow {
            self.shown_narrow = false;
        }
        self.narrow = narrow;

        // Bots' main view on screen or not, whichever way the window got there.
        let groups_on_screen = self.area == Area::Bots && self.bot_chat().is_none() && !self.client.gate().is_held();
        if self.groups_on_screen != groups_on_screen {
            self.groups_on_screen = groups_on_screen;
            if let Some(view) = self.area_view(Area::Bots).cloned() {
                view.update(cx, |view, cx| view.set_on_screen(groups_on_screen, cx));
            }
        }
        // The bot whose conversation shows: what it finished or stopped on is seen (the box's).
        let chat_open = self.bot_chat().map(|(bot, _)| bot.to_owned());
        if self.chat_open != chat_open {
            self.chat_open = chat_open.clone();
            if let Some(view) = self.area_view(Area::Bots).cloned() {
                view.update(cx, |view, cx| view.set_chat_open(chat_open, cx));
            }
        }
        let sessions = self.area == Area::Sessions;
        // A computer that can't be used shows why, and what moves it on, in every area but
        // Settings, never areas that open empty.
        let blocked = (self.area != Area::Settings).then(|| self.blocked()).flatten();
        let content: AnyElement = if let Some(blocked) = &blocked {
            div().debug_selector(|| "held-area".into()).size_full().child(crate::blocked::render(blocked, cx)).into_any_element()
        } else { match (&self.open, self.area_view(self.area)) {
            (_, Some(view)) if !sessions => match (self.bot_chat(), view.read(cx).settings(), view.read(cx).main()) {
                (Some((bot, chat)), _, _) => self.bot_chat_view(bot, chat, cx),
                (None, Some(settings), _) => settings.clone().into_any_element(),
                (None, None, Some(main)) => main.into_any_element(),
                (None, None, None) => areas::placeholder(self.area, view.read(cx).title(cx), cx),
            },
            (Open::Conversation { view, .. }, _) => view.clone().into_any_element(),
            // Last time's conversation, till the list says whether it's there: nothing yet, not
            // the landing view first.
            _ if self.restoring.session.is_some() => div().debug_selector(|| "restoring".into()).size_full().into_any_element(),
            _ => landing::render(self, window, cx),
        } };
        let area_view = (!sessions).then(|| self.area_view(self.area).cloned()).flatten();
        let sidebar: Option<AnyElement> = (self.sidebar_shown() && blocked.is_none()).then(|| match area_view {
            Some(view) => view.into_any_element(),
            None => sidebar::render(self, window, cx),
        });
        let panel = (sessions && self.panel_open && !self.client.gate().is_held()).then(|| self.panel_card(window, cx));
        div()
            .key_context("Workspace")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &NewSession, window, cx| this.new_session(None, window, cx)))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| this.focus_search(window, cx)))
            .on_action(cx.listener(|this, _: &RenameSession, window, cx| this.run_shortcut("rename", window, cx)))
            .on_action(cx.listener(|this, _: &ArchiveSession, window, cx| this.run_shortcut("archive", window, cx)))
            .on_action(cx.listener(|this, _: &DeleteSession, window, cx| this.run_shortcut("delete", window, cx)))
            .on_action(cx.listener(|this, _: &ToggleSidebar, window, cx| this.toggle_sidebar(window, cx)))
            .on_action(cx.listener(|this, _: &TogglePanel, window, cx| {
                if this.area != Area::Sessions {
                    this.show_area(Area::Sessions, window, cx);
                }
                this.toggle_panel(cx);
            }))
            .on_action(cx.listener(|this, _: &ShowChanges, window, cx| {
                if this.area != Area::Sessions {
                    this.show_area(Area::Sessions, window, cx);
                }
                this.show_panel(Tab::Changes, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowFiles, window, cx| {
                if this.area != Area::Sessions {
                    this.show_area(Area::Sessions, window, cx);
                }
                this.show_panel(Tab::Files, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowSessions, window, cx| this.show_area(Area::Sessions, window, cx)))
            .on_action(cx.listener(|this, _: &ShowBots, window, cx| this.show_area(Area::Bots, window, cx)))
            .on_action(cx.listener(|this, _: &ShowQueue, window, cx| this.show_area(Area::Queue, window, cx)))
            .on_action(cx.listener(|this, _: &ShowComputers, window, cx| this.show_area(Area::Computers, window, cx)))
            .on_action(cx.listener(|this, _: &ShowSettings, window, cx| this.show_area(Area::Settings, window, cx)))
            .on_action(cx.listener(|this, _: &ShowUsage, window, cx| this.show_setting(&crate::settings::box_key("", "usage"), window, cx)))
            .on_action(cx.listener(|this, _: &ShowAccounts, window, cx| this.show_setting(&crate::settings::box_key("", "accounts"), window, cx)))
            .on_action(cx.listener(|this, _: &Back, window, cx| this.go_back(window, cx)))
            .on_action(cx.listener(|this, _: &Forward, window, cx| this.go_forward(window, cx)))
            .on_action(|_: &OpenHelp, _, cx| cx.open_url(crate::area::HELP))
            // A drag of the panel's edge, wherever the pointer goes meanwhile.
            .when(self.resizing.is_some(), |d| {
                d.cursor_col_resize()
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                        // Let go outside the window: the drag ends as the pointer comes back.
                        if event.pressed_button != Some(MouseButton::Left) {
                            this.end_resize(cx);
                        } else {
                            this.resize_to(event.position.x, window.viewport_size().width, cx);
                        }
                    }))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.end_resize(cx)))
            })
            .on_action(cx.listener(|_, _: &ToggleAppearance, _, cx| {
                let next = cx.theme().appearance.toggled();
                crate::yours::set_theme(next, cx);
            }))
            .flex()
            .flex_col()
            .size_full()
            .bg(c.background)
            .text_ui(TypeSize::Small)
            .text_color(c.text)
            .child(title_bar::render(self, cx))
            .child(
                div().flex_1().min_h_0().flex().child(rail::render(self, cx)).child(
                    // The area's sidebar and its main view share one raised panel, its corner
                    // rounded against the rail.
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .bg(c.surface)
                        .rounded_tl(radius::CARD)
                        .border_t_1()
                        .border_l_1()
                        .border_color(c.border)
                        .overflow_hidden()
                        .children(sidebar.map(|sidebar| {
                            div().id("area-sidebar").debug_selector(|| "area-sidebar".into()).w(px(sidebar::WIDTH)).h_full().flex_none().bg(c.sidebar).border_r_1().border_color(c.border).child(sidebar)
                        }))
                        .child(div().relative().flex_1().min_w_0().h_full().child(content))
                        .children(panel),
                ),
            )
    }
}
