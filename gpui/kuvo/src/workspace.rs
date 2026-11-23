//! The window: the title bar across the top; under it the rail, and beside the rail one raised
//! panel holding the open area's sidebar and its main view (OpenBot's layout). In Sessions the main
//! view is the conversation open or, for a new one, the landing view, with the side panel beside it;
//! in Bots, a bot's page or its own conversation; in the Work Queue, the queue or an item's page; Computers' pages aren't in
//! the app yet. In a narrow window the area's sidebar folds away.

use std::collections::{HashMap, HashSet};

use std::time::Duration;

use gpui::{AnimationExt as _, 
    AnyElement, AnyWindowHandle, App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement, MouseButton,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled,
    Subscription, Task, Window, deferred, div, prelude::FluentBuilder as _, px,
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
    ActiveTheme, StyledType, TextField, TextFieldEvent, TypeSize,
    menu::Menu,
    theme::radius,
};

use crate::area::{Area, Back, Forward, OpenHelp, ShowAccounts, ShowBots, ShowComputers, ShowLibrary, ShowMarketplace, ShowQueue, ShowSessions, ShowSettings, ShowUsage, UsageFilter};
use crate::areas::{AreaView, Go, Picked, Reachability, Reading};
use crate::composer::{Composer, ComposerEvent, Target};
use crate::first_run::{Finished, FirstRun};
use crate::conversation::ConversationView;
use crate::rail::{self, Rings};
use crate::session_menu::{
    self, ActivePopover, ArchiveSession, ConfirmDeletePopover, DeleteSession, PopoverAnchor,
    RenamePopover, RenameSession, SessionMenu,
};
use crate::sidebar::{FocusSearch, Line};
use crate::panel::{self, Kind, PanelEvent, Place, SidePanel};
use crate::{NewSession, OpenFolder, ShowChanges, ShowFiles, ToggleAppearance, TogglePanel, ToggleSidebar, areas, landing, sidebar, time, title_bar};

#[path = "quick_search.rs"]
mod quick_search;

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

/// How far a conversation's scrolled within a row before where it's read up to is kept again.
const READ_AT_STRETCH: f32 = 120.;
/// The raised panel's leading edge, between the rail and the sidebar.
const CARD_EDGE: Pixels = px(1.);

/// The raised panel's rounded corner, kept clean of what's inside it. GPUI clips children to a
/// square, so a sidebar's or a page's own background would show its square corner past the
/// panel's rounded one: a second edge along the corner and down the side. This lays the window's
/// background over just that sliver, drawn last, under the panel's own edge (borders draw after
/// children), so every area gets one rounded edge without rounding each page itself.
///
/// It's a quad whose border is the fill: placed a radius above and left of the panel, rounded at
/// twice the radius, its inner curve lands exactly on the inside of the panel's edge.
fn card_corner(background: gpui::Hsla) -> gpui::Div {
    let r = radius::CARD;
    // Insets are from inside the panel's 1px edge.
    let mut corner = div().absolute().top(-(r + CARD_EDGE)).left(-(r + CARD_EDGE)).right_0().bottom_0().rounded_tl(r * 2.).border_color(background);
    let widths = &mut corner.style().border_widths;
    widths.top = Some((r + CARD_EDGE).into());
    widths.left = Some((r + CARD_EDGE).into());
    corner
}

struct SidebarDrag {
    from: Pixels,
    initial: Pixels,
    width: Pixels,
}

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
    /// Where each conversation was read up to, by its session: brought into view as it opens.
    reading: std::collections::BTreeMap<String, crate::yours::ReadAt>,
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
    /// Asks the box that lists your computers to take the release waiting for it now (its own
    /// row's `update-here`).
    Here,
    /// Opens Computers › Add a Machine on the box that lists your computers, the address it was
    /// added under typed in (none: empty), to set it up over SSH from there.
    AddMachine { address: Option<String> },
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

/// The title bar's update notifier's card: open, Update Now asked of a computer (by key),
/// Dismiss on its way, or why what was asked couldn't be done.
#[derive(Default)]
struct FleetCard {
    open: bool,
    retrying: Option<String>,
    error: Option<String>,
    _task: Option<Task<()>>,
}

/// One computer in the update notifier's card: its key, name and how its update stands.
#[derive(Clone, Debug, PartialEq)]
pub struct FleetRow {
    pub key: String,
    pub name: String,
    pub standing: crate::fleet::Standing,
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
    OpenMoved { computer: String, target: kuvo_box::moves::Target },
    /// Show the computer with this key.
    To(String),
    /// Open a session on another connected computer.
    Session { computer: String, session: String },
    /// Open a session on another connected computer at a message (the Library's "Show in conversation").
    SessionAt { computer: String, session: String, message: Option<String> },
    /// This app was removed from your devices: pair it again.
    PairAgain,
    /// Something was asked of the box that lists your computers (Update Now): read the list again
    /// where it isn't followed.
    ReadList,
    /// Update Now for this Mac's own Kuvo, by the installer this app carries: `now`, else once
    /// the work on it ends (at once where none is).
    /// Show this computer in Computers, on the box that lists your computers: Add a Machine with
    /// this address (`add_at`), else its page.
    ShowComputer { key: String, add_at: Option<String> },
    /// This computer's last setup was wiped: its installer is asked again, and offers to set
    /// Kuvo up here anew.
    Reinstall,
}

impl gpui::EventEmitter<Switch> for Workspace {}

pub struct Workspace {
    client: BoxClient,
    moves: Entity<crate::moves::MovesView>,
    image_viewer: Entity<crate::image_viewer::ImageViewer>,
    insights: Entity<crate::insights::InsightsView>,
    library: Entity<crate::library::LibraryView>,
    marketplace: Entity<crate::marketplace::MarketplaceView>,
    /// Your computers as this app reaches them, where the window can show another's (none: a
    /// workspace by itself, which shows its one box).
    switching: Option<crate::home::Switching>,
    /// The box that lists your computers (the nearest), which does what their rows offer.
    lister: Option<BoxClient>,
    /// How bringing this Mac's own Kuvo up to date stands, where this app does it.
    /// Whether this app carries a release it brings this Mac's own Kuvo up to (its installer).
    /// How the gate stood when `kuvo/info` was last taken: one that changes since (the box updated,
    /// its server started again on new code) has `kuvo/info` read again.
    gate_seen: kuvo_box::gate::Gate,
    /// The title bar's update chip's card.
    update_card: UpdateCard,
    /// Whether the title bar's notifications are open.
    notifications_open: bool,
    fleet_card: FleetCard,
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
    /// Sessions started here on another computer (picked in Runs on), by that computer's key, until
    /// the setup's list says where they are.
    started_on: HashMap<String, String>,
    opening_session: Option<Task<()>>,
    /// Where a new session or bot can run, as the nearest box says it (`kuvo/setup/runs-on`).
    runs_on: Vec<kuvo_box::models::RunsOnComputer>,
    _runs_on_follow: Option<Follow<Followed<kuvo_box::models::RunsOn>>>,
    /// Sessions started here whose first message didn't go (or came back with why), while you were
    /// elsewhere: marked in the sidebar until opened.
    unsent: HashSet<String>,
    collapsed: HashSet<String>,
    /// Projects whose whole list shows (a box before it paged them).
    showing_all: HashSet<String>,
    /// Projects whose list "Show more" opened further, as far as the box's page for them goes: each
    /// with the cursor asked with, asked again as the list changes (`kuvo/sessions/page`).
    pages: HashMap<String, ProjectPage>,
    /// Now, as the sidebar's times count from it (fixed for shots).
    now: Option<f64>,
    /// The hour of day the landing's sky shows (fixed for shots); otherwise this computer's clock.
    sky_hour: Option<f32>,
    focus: FocusHandle,
    sidebar_toggle_focus: FocusHandle,
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
    /// Where the conversations open were read up to as last kept (`keep_place`), by session, and
    /// what follows their scrolling to keep it again as it moves on.
    reading_kept: std::collections::BTreeMap<String, crate::yours::ReadAt>,
    _open_reading: Option<Subscription>,
    _bot_reading: Option<Subscription>,
    resizing: Option<(Pixels, Pixels)>,
    sidebar_drag: Option<SidebarDrag>,
    quick_search: Option<quick_search::QuickSearch>,
    /// Keep the bot strip beside a session opened with the compact sidebar's search.
    session_with_bot_strip: bool,
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
    computers_reachability: Reachability,
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
        let moves = cx.new(|cx| crate::moves::MovesView::new(client.clone(), live, cx));
        let areas = areas::views(&client, live, cx);
        if let Some((_, bots)) = areas.iter().find(|(area, _)| *area == Area::Bots) {
            bots.update(cx, |bots, cx| bots.set_moves(moves.clone(), cx));
        }
        let insights = cx.new(|cx| crate::insights::InsightsView::new(client.clone(), window, cx));
        cx.observe(&insights, |_, _, cx| cx.notify()).detach();
        let library = cx.new(|cx| crate::library::LibraryView::new(client.clone(), live, window, cx));
        cx.subscribe_in(&library, window, |this: &mut Self, _, at: &crate::library::ShowAt, window, cx| {
            // (On whichever computer it is: the session list says, and it opens through that one.)
            this.open_session_at(&at.session, at.message.as_deref(), window, cx);
        }).detach();
        cx.subscribe_in(&library, window, |this: &mut Self, _, open: &crate::library::OpenInPanel, window, cx| {
            match &open.session {
                Some(session) => this.open_session_at(session, None, window, cx),
                None => this.show_area(Area::Sessions, window, cx),
            }
            this.preview_file(&open.path, window, cx);
        }).detach();
        cx.observe(&library, |_, _, cx| cx.notify()).detach();
        let marketplace = cx.new(|cx| crate::marketplace::MarketplaceView::new(client.clone(), window, cx));
        // An agent added here: on to its bot's conversation, as New Bot goes.
        cx.subscribe_in(&marketplace, window, |this: &mut Self, _, made: &crate::marketplace::BotMade, window, cx| {
            if let Some(bots) = this.area_view(Area::Bots).cloned() {
                bots.update(cx, |view, cx| view.read_list(cx));
            }
            this.open_bot_chat(&made.bot, &made.session, None, window, cx);
        }).detach();
        cx.observe(&marketplace, |_, _, cx| cx.notify()).detach();
        let mut area_subscriptions: Vec<Subscription> = areas.iter().map(|(_, view)| cx.subscribe_in(view, window, |this: &mut Workspace, view, _: &Picked, window, cx| this.area_picked(&view, window, cx))).collect();
        area_subscriptions.push(cx.observe(&moves, |_, _, cx| cx.notify()));
        area_subscriptions.push(cx.subscribe(&moves, |_: &mut Self, _, moved: &crate::moves::OpenMoved, cx| {
            cx.emit(Switch::OpenMoved { computer: moved.computer.clone(), target: moved.target.clone() });
        }));
        area_subscriptions.push(cx.subscribe(&moves, |this: &mut Self, _, finished: &crate::moves::MoveFinished, cx| {
            use kuvo_box::moves::Kind;
            if finished.0.kind == Kind::Chat {
                this.refresh_list_after_action(cx);
            } else if let Some(bots) = this.area_view(Area::Bots).cloned() {
                bots.update(cx, |bots, cx| bots.refresh_after_move(cx));
            }
        }));
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
            cx.subscribe_in(view, window, |this: &mut Workspace, view, go: &Go, window, cx| match go {
                Go::BotChat { bot, session, draft } => this.open_bot_chat(bot, session, draft.as_deref(), window, cx),
                Go::BotRehomed => {
                    if this.area == Area::Bots {
                        this.area_picked(&view, window, cx);
                    } else {
                        this.in_bot_chat = false;
                        this.keep_place(cx);
                    }
                }
                Go::Session(session) => this.open_session_now(session, window, cx),
                Go::Sessions => this.show_area(Area::Sessions, window, cx),
                Go::Marketplace => this.show_marketplace(crate::marketplace::Section::Agents, window, cx),
                Go::SearchBotsAndSessions => { this.open_quick_search(window, cx); }
                Go::ReadComputers => {
                    if this._computers_follow.is_none() {
                        this.read_computers(cx);
                    }
                }
                Go::ReadList => cx.emit(Switch::ReadList),
                Go::PairAgain => cx.emit(Switch::PairAgain),
                Go::Reinstall => cx.emit(Switch::Reinstall),
                Go::AddAt(address) => cx.emit(Switch::ShowComputer { key: String::new(), add_at: Some(address.clone().unwrap_or_default()) }),
                // System Settings opened from Settings › This Mac: a permission added by hand
                // gets the tile beside it, as from the heads-up.
                Go::Opened(address) => {
                    this.permissions.update(cx, |permissions, cx| permissions.opened(address, cx));
                }
                Go::Setting(place) => this.show_setting(place, window, cx),
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
            area_subscriptions.push(cx.observe_in(bots, window, |this: &mut Workspace, _, window, cx| { this.bots_changed(window, cx); cx.notify(); }));
        }
        let rings = cx.new(|cx| Rings::new(client.clone(), live, cx));
        let image_viewer = cx.new(|cx| crate::image_viewer::ImageViewer::new(live, cx));
        crate::image_viewer::register(window, &image_viewer, cx);
        area_subscriptions.push(cx.observe_global::<crate::yours::Store>(|_, cx| cx.notify()));
        let side_panel = cx.new(|cx| SidePanel::new(client.clone(), live, cx));
        area_subscriptions.push(cx.subscribe_in(&side_panel, window, |this: &mut Workspace, _, event: &PanelEvent, window, cx| match event {
            PanelEvent::Close => {
                if this.panel_open {
                    this.toggle_panel(cx);
                }
            }
            PanelEvent::OpenSession(session) => this.open_session_now(session, window, cx),
        }));
        let mut workspace = Workspace {
            client: client.clone(),
            switching: None,
            moves,
            image_viewer,
            lister: None,
            gate_seen: kuvo_box::gate::Gate::Unknown,
            update_card: UpdateCard::default(),
            notifications_open: false,
            fleet_card: FleetCard::default(),
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
            started_on: HashMap::new(),
            opening_session: None,
            runs_on: Vec::new(),
            _runs_on_follow: None,
            unsent: HashSet::new(),
            collapsed: HashSet::new(),
            showing_all: HashSet::new(),
            pages: HashMap::new(),
            now: None,
            sky_hour: None,
            focus: cx.focus_handle(),
            sidebar_toggle_focus: cx.focus_handle(),
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
            panel: side_panel,
            panel_open: false,
            panel_width: panel::WIDTH,
            reading_kept: Default::default(),
            _open_reading: None,
            _bot_reading: None,
            resizing: None,
            sidebar_drag: None,
            quick_search: None,
            session_with_bot_strip: false,
            area: Area::Sessions,
            areas,
            insights,
            library,
            marketplace,
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
            computers_reachability: Reachability::Unanswered,
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
            workspace.tasks.push(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(30)).await;
                    if this.update(cx, |this, cx| this.tick_wake_labels(cx)).is_err() { break; }
                }
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
            // Where a new session can run, as the nearest box says it (an older box without it:
            // this computer alone, so no Runs on).
            let follow = client.follow_runs_on();
            let updates = follow.updates().clone();
            workspace._runs_on_follow = Some(follow);
            workspace.tasks.push(cx.spawn(async move |this, cx| {
                while let Ok(update) = updates.recv().await {
                    let rows = match update {
                        Followed::Now(said) => said.computers,
                        Followed::Failed(_) => Vec::new(),
                    };
                    if this.update(cx, |this, cx| this.take_runs_on(rows, cx)).is_err() {
                        break;
                    }
                }
            }));
            // Where the box streams, what waits on you and your computers are followed as they
            // change; elsewhere they're read again (as the list changes, as Computers opens).
            if client.streams() {
                let follow = client.follow_waiting();
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
                let follow = client.follow_computers();
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

    /// The session list, read once (shots and tests, against a stand-in).
    #[cfg(feature = "stand-in")]
    fn read_list_at_once(&self) -> ListUpdate {
        let line = if self.client.advertises(kuvo_box::sessions::MERGED_SESSIONS) {
            self.client.merged_sessions_at_once()
        } else {
            self.client.session_list(None)
        };
        match line {
            Ok(line) => {
                let mut listing = Listing::new();
                listing.take(line);
                ListUpdate::Listing(listing)
            }
            Err(kuvo_box::Error::NeedsUpdating { missing }) => ListUpdate::Held { missing },
            Err(error) => ListUpdate::Failed(error),
        }
    }

    /// Reads the box at once, on this thread, as the follows would hand it: what it is, its lists,
    /// and the landing composer's chips (shots and tests, against a stand-in).
    #[cfg(feature = "stand-in")]
    pub fn read_at_once(&mut self, cx: &mut Context<Self>) {
        let info = self.client.connect();
        self.connected(info, cx);
        // (As the follow does: a box that says it's held shows its update, not its sessions.)
        let update = match self.client.gate() {
            kuvo_box::Gate::Held { missing } => ListUpdate::Held { missing },
            _ => self.read_list_at_once(),
        };
        self.take_list(update, cx);
        self.landing.update(cx, |composer, cx| composer.read_at_once(cx));
        #[cfg(feature = "stand-in")]
        {
            let rows = self.client.runs_on_at_once().map(|said| said.computers).unwrap_or_default();
            self.take_runs_on(rows, cx);
        }
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
        if self.bot_chat().is_some() {
            // (A bot's: Bots read it at once as it opened.)
            return;
        }
        if let Some(view) = self.conversation().cloned() {
            self.read_conversation_at_once(&view, session, cx);
        }
    }

    /// Reads `session` into `view` at once, on this thread, as its follows would hand it.
    #[cfg(feature = "stand-in")]
    fn read_conversation_at_once(&self, view: &Entity<ConversationView>, session: &str, cx: &mut Context<Self>) {
        // (Its own computer's box: another computer's conversation is read there.)
        let client = view.read(cx).client().clone();
        {
            // (Through this computer's box, as its follow reads it: another computer's last copy
            // while that computer's away.)
            let nearest = view.read(cx).nearest().clone();
            let update = match crate::conversation::read_now(&nearest, session) {
                Ok(conversation) => kuvo_box::ConversationUpdate::Conversation(conversation),
                Err(kuvo_box::Error::Refused(refusal)) if refusal.status == 404 => kuvo_box::ConversationUpdate::Gone(refusal),
                Err(error) => kuvo_box::ConversationUpdate::Failed(error),
            };
            let asks = client.asks(session).unwrap_or_default();
            let queue = view.read(cx).composer().read(cx).delivery_box().map_or_else(|| client.queue(session), |delivery| delivery.queue(session)).unwrap_or_default();
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
            Err(error) => {
                self.box_state = BoxState::Unreachable(error.to_string());
                // A connection probe can fail after the open area last answered. Ask
                // that area again so it can distinguish a dropped box from a failed probe.
                match self.area {
                    Area::Bots | Area::Queue => {
                        if let Some(view) = self.area_view(self.area).cloned() {
                            view.update(cx, |view, cx| view.read_list(cx));
                        }
                    }
                    Area::Computers => self.read_computers(cx),
                    Area::Insights => self.insights.update(cx, |view, cx| view.read_again(cx)),
                    Area::Library => self.library.update(cx, |view, cx| view.read_again(cx)),
                    Area::Marketplace => self.marketplace.update(cx, |view, cx| view.read_again(cx)),
                    _ => {}
                }
            }
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
                // A list "Show more" opened, read again as far as it was as its project changes
                // (its order, a session come or gone); gone with its project.
                let changed: Vec<String> = self
                    .pages
                    .keys()
                    .filter(|project| previous_listing.projects().get(*project) != self.listing.projects().get(*project))
                    .cloned()
                    .collect();
                for project in changed {
                    if self.listing.projects().contains_key(&project) {
                        self.read_page(&project, cx);
                    } else {
                        self.pages.remove(&project);
                    }
                }
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
                self.show_refuses(cx);
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
        let held = self.client.gate().is_held();
        if !held && !self.info.as_ref().is_some_and(|info| info.compatibility.is_older()) {
            return None;
        }
        if held {
            return Some(self.listed_update().unwrap_or_else(|| self.held_fallback()));
        }
        // Only behind: it's in use, so never said to wait; and with nothing to press from here,
        // nothing's said at all.
        self.listed_update().filter(|update| update.action.is_some() || update.updating)
    }

    /// The computer this window shows, as your computers' list has it.
    fn shown_listed(&self) -> Option<&kuvo_box::boxes::Row> {
        self.switching.as_ref().and_then(|switching| switching.shown.as_deref().and_then(|shown| switching.rows.iter().find(|row| row.key() == shown)))
    }

    /// What brings the computer this window shows up to date, in the words of the box that lists
    /// your computers (its row's `update`): none where it says nothing.
    fn listed_update(&self) -> Option<HeldUpdate> {
        self.shown_listed().and_then(Self::offer_for)
    }

    /// What's said of a held computer whose list says nothing of its update: that it's older.
    fn held_fallback(&self) -> HeldUpdate {
        let name = self.info.as_ref().map(|_| self.box_name().to_string());
        let (title, detail) = held_words(name.as_deref(), self.client.here());
        HeldUpdate { key: self.shown_listed().map(|row| row.key().to_owned()), title, detail, updating: false, attention: false, action: None, does: None, own: self.client.here() }
    }

    /// The update offer from the box that lists this computer.
    pub fn update_offer(&self, key: &str) -> Option<HeldUpdate> {
        let row = self.switching.as_ref()?.rows.iter().find(|row| row.key() == key && !row.computer.here)?;
        Self::offer_for(row)
    }

    fn offer_for(row: &kuvo_box::boxes::Row) -> Option<HeldUpdate> {
        let update = row.computer.update.as_ref()?;
        let action = row.computer.update_action().cloned();
        let does = action.as_ref().and_then(|action| match action.id.as_str() {
            "update-here" => Some(UpdateDoes::Here),
            "update" => Some(UpdateDoes::Update(row.key().to_owned())),
            "add" => Some(UpdateDoes::AddMachine { address: action.address.clone() }),
            _ => None,
        });
        Some(HeldUpdate {
            key: Some(row.key().to_owned()),
            title: update.title.clone(),
            detail: update.detail.clone(),
            updating: update.is_updating(),
            attention: update.state == "failed" || update.long,
            action,
            does,
            own: row.computer.here,
        })
    }

    /// The shown computer's offer, or the lister's own update.
    pub fn update_chip(&self) -> Option<HeldUpdate> {
        self.held_update().or_else(|| {
            self.switching.as_ref()?.rows.iter().find(|row| row.computer.here).and_then(Self::offer_for)
        })
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
        let rows = self.switching.iter().flat_map(|switching| &switching.rows)
            .filter(|row| row.computer.fleet_update.is_some())
            .map(|row| (row.key().to_owned(), row.computer.clone()))
            .collect();
        let now = self.now();
        page.update(cx, |page, cx| {
            page.set_offers(offers, lister, cx);
            page.set_fleet(rows, now, cx);
        });
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

    /// The update summary supplied by the box that lists your computers.
    pub fn fleet(&self) -> Option<&kuvo_box::areas::FleetUpdateSummary> {
        self.switching.as_ref()?.fleet.as_ref()
    }

    /// The setup's update, while the title bar shows it (`fleet::header_shown`).
    pub fn fleet_header(&self) -> Option<&kuvo_box::areas::FleetUpdateSummary> {
        self.fleet().filter(|summary| crate::fleet::header_shown(Some(summary)))
    }

    /// Each of your computers and how its update stands, in the list's order.
    pub fn fleet_rows(&self) -> Vec<FleetRow> {
        let Some(switching) = &self.switching else { return Vec::new() };
        switching.rows.iter().filter_map(|row| {
            let standing = crate::fleet::standing(&row.computer)?;
            Some(FleetRow { key: row.key().to_owned(), name: row.computer.name.clone(), standing })
        }).collect()
    }

    pub fn fleet_card_open(&self) -> bool {
        self.fleet_card.open
    }

    /// The computer Update Now was asked of, while it's on its way.
    pub fn fleet_retrying(&self) -> Option<&str> {
        self.fleet_card.retrying.as_deref()
    }

    pub fn fleet_error(&self) -> Option<&str> {
        self.fleet_card.error.as_deref()
    }

    /// Opens the update notifier's card, or closes it.
    pub fn toggle_fleet_card(&mut self, cx: &mut Context<Self>) {
        if self.fleet_card.open {
            return self.close_fleet_card(cx);
        }
        self.fleet_card = FleetCard { open: true, ..FleetCard::default() };
        cx.notify();
    }

    /// Closes the update notifier's card (not while what it asked is on its way).
    pub fn close_fleet_card(&mut self, cx: &mut Context<Self>) {
        if self.fleet_card.retrying.is_some() {
            return;
        }
        self.fleet_card = FleetCard::default();
        cx.notify();
    }

    /// Try Again from the notifier asks the box that lists your computers, naming this one (`POST
    /// kuvo/update/retry {key}`), which passes it on; a refusal (it can't retry now, the release
    /// changed, it can't be reached) is said in the box's words.
    pub fn fleet_retry(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.fleet_card.retrying.is_some() {
            return;
        }
        let Some(row) = self.fleet_rows().into_iter().find(|row| row.key == key) else { return };
        let Some(lister) = self.lister.clone() else {
            self.fleet_card.error = Some("Your computers’ list isn’t reachable right now. Try again in a moment.".into());
            return cx.notify();
        };
        let (asked, target, sha256) = (row.key.clone(), row.standing.target.clone(), row.standing.target_sha256.clone());
        let work = move || lister.retry_fleet_release(&asked, &target, sha256.as_deref()).map(|_| ());
        self.fleet_card.retrying = Some(key.to_owned());
        self.fleet_card.error = None;
        self.fleet_run(work, cx);
    }

    fn fleet_run(&mut self, work: impl FnOnce() -> kuvo_box::Result<()> + Send + 'static, cx: &mut Context<Self>) {
        let done = |this: &mut Workspace, result: kuvo_box::Result<()>, cx: &mut Context<Workspace>| {
            this.fleet_card.retrying = None;
            match result {
                Ok(()) => {
                    cx.emit(Switch::ReadList);
                }
                Err(error) => this.fleet_card.error = Some(kuvo_box::accounts::said(&error)),
            }
            cx.notify();
        };
        if !self.live {
            let result = work();
            return done(self, result, cx);
        }
        cx.notify();
        self.fleet_card._task = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { work() }).await;
            this.update(cx, |this, cx| done(this, result, cx)).ok();
        }));
    }

    /// Open Computers, from the notifier's card: all your computers, or one (`key`) whose page
    /// says what brings it up.
    pub fn fleet_open_computers(&mut self, key: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        if self.fleet_card.retrying.is_some() {
            return;
        }
        self.close_fleet_card(cx);
        if let Some(key) = key {
            return self.show_computer(key, None, window, cx);
        }
        if let Some(view) = self.area_view(Area::Computers).cloned() {
            view.update(cx, |view, cx| view.select(None, cx));
        }
        self.show_area(Area::Computers, window, cx);
    }

    pub fn notifications_open(&self) -> bool {
        self.notifications_open
    }

    /// Opens the title bar's notifications, or closes them.
    pub fn toggle_notifications(&mut self, cx: &mut Context<Self>) {
        if self.notifications_open {
            return self.close_notifications(cx);
        }
        self.notifications_open = true;
        cx.notify();
    }

    /// Closes the notifications: what was only news has now been seen, and goes.
    pub fn close_notifications(&mut self, cx: &mut Context<Self>) {
        if !self.notifications_open {
            return;
        }
        self.notifications_open = false;
        self.moves.update(cx, |moves, cx| moves.seen(cx));
        cx.notify();
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

    /// Ask the listing Box to run its update action after any required confirmation.
    /// An SSH setup offer opens Computers at the address supplied by the Box.
    pub fn update_now(&mut self, confirmed: bool, cx: &mut Context<Self>) {
        if self.update_card.sending {
            return;
        }
        let Some(held) = self.update_chip() else { return self.close_update_card(cx) };
        let (Some(does), Some(action)) = (held.does, held.action) else { return };
        if action.why.is_some() {
            return;
        }
        if action.confirm.is_some() && !confirmed {
            self.update_card.confirming = true;
            self.update_card.error = None;
            return cx.notify();
        }
        let work: Asked = match does {
            UpdateDoes::AddMachine { address } => {
                self.update_card = UpdateCard::default();
                cx.emit(Switch::ShowComputer { key: String::new(), add_at: Some(address.unwrap_or_default()) });
                return cx.notify();
            }
            UpdateDoes::Update(key) => Box::new(move |lister| lister.update_computer(&key).map(|_| ())),
            UpdateDoes::Here => Box::new(|lister| lister.update_here()),
        };
        let Some(lister) = self.lister.clone() else {
            self.update_card.error = Some("Your computers’ list isn’t reachable right now. Try again in a moment.".into());
            return cx.notify();
        };
        let done = move |this: &mut Workspace, result: kuvo_box::Result<()>, cx: &mut Context<Workspace>| {
            this.update_card.sending = false;
            match result {
                Ok(()) => {
                    this.update_card = UpdateCard::default();
                    cx.emit(Switch::ReadList);
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

    /// Whether the current area has answered independently of the workspace's connection probe.
    fn area_reached(&self, cx: &App) -> bool {
        let reachability = match self.area {
            Area::Bots | Area::Queue => self.area_view(self.area)
                .map_or(Reachability::Unanswered, |view| view.read(cx).reachability()),
            Area::Computers => self.computers_reachability,
            Area::Insights => self.insights.read(cx).reachability(),
            Area::Library => self.library.read(cx).reachability(),
            Area::Marketplace => self.marketplace.read(cx).reachability(),
            _ => Reachability::Unanswered,
        };
        reachability == Reachability::Reached
    }

    /// What the window shows in place of its areas while the computer it shows can't be used: its
    /// Kuvo is older than this app (with what brings it up, as the update chip offers it), or it
    /// can't be reached (Try Again). With another of your computers that works, to switch to.
    /// None while it can be used.
    pub fn blocked(&self, cx: &App) -> Option<crate::blocked::Blocked> {
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
        } else if matches!(self.box_state, BoxState::Unreachable(_))
            && !self.client.here()
            && !self.area_reached(cx)
        {
            // (Another of your computers: this Mac's own box not answering is the launch's to say.)
            // (In the box's words where the list says why it's away; never the error's own.)
            let away = self.shown_row().and_then(|row| row.computer.notice.clone());
            let (title, detail) = away.map(|notice| (notice.title, notice.detail)).unwrap_or_else(|| unreached_words(&name));
            ("unreached", title, detail, false, true, Some(("Try Again".to_owned(), BlockedDo::TryAgain)))
        } else if let Some((feature, page, shows)) = self.area_feature()
            && self.info.as_ref().is_some_and(|info| info.compatibility.is_behind())
            && !self.client.gate().has(feature)
        {
            let computer = if name == "This Mac" { "this Mac" } else { name.as_str() };
            let title = format!("Kuvo on {computer} needs to update to show {page}");
            let fallback_detail = format!("Update Kuvo on {computer} to see {shows} here.");
            let (detail, busy, attention, action) = if let Some(update) = self.listed_update() {
                let action = update.action.as_ref().filter(|action| action.why.is_none() && update.does.is_some()).map(|action| (action.label.clone(), BlockedDo::Update));
                let detail = if update.detail.is_empty() { fallback_detail } else { update.detail };
                (detail, update.updating || self.update_card.sending, update.attention, action)
            } else {
                (fallback_detail, self.update_card.sending, false, None)
            };
            ("held", title, detail, busy, attention, action)
        } else if self.box_state == BoxState::Connecting && self.reaching_again && !self.client.here() {
            let (title, _) = unreached_words(&name);
            ("unreached", title, format!("Reaching {name}…"), true, false, None)
        } else {
            return None;
        };
        // (One window shows all your computers: there's no other to switch it to.)
        let switch_to = None;
        Some(Blocked { kind, title, detail, busy, attention, action, switch_to, error })
    }

    /// The bots the Library's sidebar lists: this computer's and your others', as the Bots list has them.
    fn library_bots(&self, cx: &App) -> Vec<crate::library::LibraryBot> {
        let Some(view) = self.area_view(Area::Bots) else { return Vec::new() };
        let Reading::Ready(bots) = view.read(cx).bots() else { return Vec::new() };
        let here = bots.bots.iter().map(|bot| crate::library::LibraryBot { id: bot.id.clone(), name: bot.name.clone(), look: bot.look.clone() });
        let elsewhere = bots.elsewhere.iter().filter_map(|bot| {
            Some(crate::library::LibraryBot { id: bot.id.clone(), name: bot.name.clone()?, look: bot.look.clone().unwrap_or_else(crate::areas::unknown_bot_look) })
        });
        here.chain(elsewhere).collect()
    }

    /// The feature an area's page needs of a box, its name and what it shows (for "needs to update").
    fn area_feature(&self) -> Option<(&'static str, &'static str, &'static str)> {
        match self.area {
            Area::Insights => Some(("usage.insights", "Insights", "usage and cost insights")),
            _ => None,
        }
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

    /// The local hour of day (`0.0..24.0`) the landing's sky shows: this computer's clock.
    pub fn sky_hour(&self) -> f32 {
        self.sky_hour.unwrap_or_else(|| time::hour_of_day(time::now_ms()))
    }

    /// Fixes the landing's sky at `hour` (shots, so each time of day can be seen).
    pub fn set_sky_hour(&mut self, hour: f32, cx: &mut Context<Self>) {
        self.sky_hour = Some(hour);
        cx.notify();
    }

    /// Counts the sidebar's times from `now` (shots, so they read the same each time).
    pub fn set_now(&mut self, now: f64, cx: &mut Context<Self>) {
        self.now = Some(now);
        self.panel.update(cx, |panel, _| panel.set_now(now));
        self.refresh_conversation_wakes(cx);
        if let Some(view) = self.conversation().cloned() {
            view.update(cx, |view, _| view.set_now(now));
        }
        if let Some(chat) = &self.bot_chat {
            chat.view.clone().update(cx, |view, _| view.set_now(now));
        }
        self.rings.update(cx, |rings, _| rings.set_now(now));
        self.insights.update(cx, |insights, _| insights.set_now(now));
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

    /// "Show more" under a project's list, or "Show fewer" where it's opened.
    pub fn toggle_all(&mut self, project: &str, cx: &mut Context<Self>) {
        if self.pages_lists() {
            return if self.pages.contains_key(project) && self.more_of(project).is_none() { self.show_fewer(project, cx) } else { self.show_more(project, cx) };
        }
        if !self.showing_all.remove(project) {
            self.showing_all.insert(project.to_owned());
        }
        self.prune_session_popover(cx);
        cx.notify();
    }

    /// The conversation open, a subagent that takes no messages as its box says: its note in the
    /// composer's place, with the way to the conversation that started it.
    fn show_refuses(&self, cx: &mut Context<Self>) {
        let Open::Conversation { view, session } = &self.open else { return };
        let facts = self.listing.facts(session);
        let refuses = facts.and_then(|facts| facts.messages.as_ref()).filter(|messages| messages.refused()).map(|messages| {
            let parent = facts.and_then(|facts| facts.parent.clone()).map(|parent| {
                let title = self.listing.facts(&parent).and_then(|facts| facts.title.clone()).filter(|title| !title.trim().is_empty()).unwrap_or_else(|| "New Session".into());
                let short = if title.chars().count() > 32 { format!("{}…", title.chars().take(31).collect::<String>().trim_end()) } else { title };
                (parent, format!("“{short}”"))
            });
            crate::conversation::Refuses { note: messages.note.clone().unwrap_or_else(|| "This subagent takes no messages.".into()), parent }
        });
        view.update(cx, |view, cx| view.set_refuses(refuses, cx));
    }

    /// Whether its box pages its projects' lists: it says each one's first sessions.
    fn pages_lists(&self) -> bool {
        self.listing.projects().values().any(|project| project.shown.is_some())
    }

    /// A project's list as it shows now, in the box's order: as far as "Show more" opened it, else
    /// its first sessions; none from a box before it paged them.
    pub fn project_list(&self, project: &str) -> Option<&[String]> {
        if let Some(page) = self.pages.get(project).and_then(|page| page.page.as_ref()) {
            return Some(&page.sessions);
        }
        self.listing.projects().get(project).and_then(|listed| listed.shown.as_deref())
    }

    /// What "Show more" under a project adds next, as its box says; none once all show.
    pub fn more_of(&self, project: &str) -> Option<&kuvo_box::sessions::ShowMore> {
        match self.pages.get(project).and_then(|page| page.page.as_ref()) {
            Some(page) => page.more.as_ref(),
            None => self.listing.projects().get(project).and_then(|listed| listed.more.as_ref()),
        }
    }

    /// How many sessions a project's list has in all, as its box says (none from a box before it said).
    pub fn total_of(&self, project: &str) -> Option<u64> {
        match self.pages.get(project).and_then(|page| page.page.as_ref()) {
            Some(page) => Some(page.total).filter(|total| *total > 0),
            None => self.listing.projects().get(project).and_then(|listed| listed.total),
        }
    }

    /// Whether "Show more" opened a project's list further than it shows first.
    pub fn shows_more(&self, project: &str) -> bool {
        self.pages.contains_key(project)
    }

    /// Whether "Show more" under a project is on its way, and why the last one couldn't be shown.
    pub fn paging(&self, project: &str) -> (bool, Option<&str>) {
        self.pages.get(project).map_or((false, None), |page| (page.asking, page.failed.as_deref()))
    }

    /// "Show more": its list as far as the box's next page goes.
    pub fn show_more(&mut self, project: &str, cx: &mut Context<Self>) {
        let Some(more) = self.more_of(project).cloned() else { return };
        let page = self.pages.entry(project.to_owned()).or_default();
        if page.asking {
            return;
        }
        page.cursor = more.cursor;
        page.failed = None;
        self.read_page(project, cx);
    }

    /// "Show fewer": its first sessions again.
    pub fn show_fewer(&mut self, project: &str, cx: &mut Context<Self>) {
        self.pages.remove(project);
        self.prune_session_popover(cx);
        cx.notify();
    }

    /// A project's page read from its box, with the cursor it was asked with (again as its list
    /// changes); one asked again while it's on its way is read once more after it.
    fn read_page(&mut self, project: &str, cx: &mut Context<Self>) {
        let Some(page) = self.pages.get_mut(project) else { return };
        if page.asking {
            page.again = true;
            return;
        }
        page.asking = true;
        page.again = false;
        let key = project.to_owned();
        let (client, cursor, project) = (self.client.clone(), page.cursor.clone(), project.to_owned());
        let asked = (project.clone(), cursor.clone());
        let work = move || client.session_page(&asked.0, &asked.1);
        let done = move |this: &mut Self, result: kuvo_box::Result<kuvo_box::sessions::Page>, cx: &mut Context<Self>| {
            let Some(page) = this.pages.get_mut(&project) else { return };
            // (Asked with a cursor since given up: what it brought goes.)
            if page.cursor != cursor {
                page.asking = false;
                return this.read_page(&project, cx);
            }
            page.asking = false;
            match result {
                Ok(read) => page.page = Some(read),
                Err(error) => page.failed = Some(kuvo_box::accounts::said(&error)),
            }
            if page.again {
                this.read_page(&project, cx);
            }
            this.prune_session_popover(cx);
            cx.notify();
        };
        cx.notify();
        if !self.live {
            let result = work();
            return done(self, result, cx);
        }
        let task = cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { work() }).await;
            this.update(cx, |this, cx| done(this, result, cx)).ok();
        });
        if let Some(page) = self.pages.get_mut(&key) {
            page._task = Some(task);
        }
    }

    /// Whether `session` was started here and what was written didn't go while you were elsewhere.
    pub fn is_unsent(&self, session: &str) -> bool {
        self.unsent.contains(session)
    }

    /// The bot whose own conversation `session` is, as its box says (the session lists' kind, else
    /// the Bots list's own conversation): such a conversation belongs in Bots, never in Sessions.
    pub fn own_bot(&self, session: &str, cx: &App) -> Option<String> {
        if let Some(bot) = self.listing().facts(session).filter(|facts| facts.kind.0 == "bot").and_then(|facts| facts.bot.clone()) {
            return Some(bot);
        }
        match self.area_view(Area::Bots)?.read(cx).bots() {
            Reading::Ready(bots) => bots.bots.iter().find(|each| each.session == session).map(|each| each.id.clone()),
            _ => None,
        }
    }

    /// The box a session's conversation, composer and panel use: the window's for this computer's,
    /// else that of the computer the setup's list says it's on (none: one this app doesn't reach
    /// now, or that computer is away and its row can't be opened).
    pub fn client_for(&self, session: &str) -> Option<BoxClient> {
        match self.listing.computer_of(session) {
            // (Started here on another computer, before the setup's list has it.)
            None if self.started_on.contains_key(session) => self.client.peer(&self.started_on[session]),
            None => Some(self.client.clone()),
            Some(on) => self.client_for_peer(on),
        }
    }

    fn client_for_peer(&self, on: &kuvo_box::sessions::OnComputer) -> Option<BoxClient> {
        on.opens().then(|| self.client.peer(&on.id)).flatten()
    }

    /// Opens a conversation where it belongs: a bot's own in Bots (that bot picked), any other in
    /// Sessions. Every "open this conversation" goes through here.
    pub fn open_session(&mut self, session: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_session_at(session, None, window, cx);
    }

    fn open_session_on(&mut self, session: &str, client: BoxClient, computer: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(bot) = self.own_bot(session, cx) {
            self.open_bot_chat(&bot, session, None, window, cx);
            return;
        }
        self.area = Area::Sessions;
        self.session_menu.active_popover = None;
        self.session_menu.action_error = None;
        self.record(Nav::Session(session.to_owned()), cx);
        if self.opened() == Some(session) && self.conversation().is_some_and(|view| view.read(cx).client().same_connection(&client)) {
            // Already open (back to it from another area): the keyboard goes back to it.
            if let Some(view) = self.conversation() {
                window.focus(&view.focus_handle(cx), cx);
            }
            cx.notify();
            return;
        }
        self.unsent.remove(session);
        let live = self.live;
        let composer = self.composer_on(session, client.clone(), computer.clone(), window, cx);
        // Only the composer on screen asks again for what the box couldn't say.
        self.show_composer(Some(&composer), cx);
        let view = cx.new(|cx| ConversationView::new(client, &self.client, computer.as_deref(), session.to_owned(), composer, live, cx));
        // (Not a bot's own, as far as the box has said yet: a bot's opens in Bots, above.)
        self.wire_bars(&view, None, cx);
        self.wire_wakes(&view, session, cx);
        self.wire_shown_files(&view, cx);
        self._open_reading = Some(self.follow_reading(&view, session, cx));
        window.focus(&view.focus_handle(cx), cx);
        self.open = Open::Conversation { view, session: session.to_owned() };
        self.show_refuses(cx);
        self.place_panel(cx);
        cx.notify();
    }

    /// Keeps where `session`'s conversation is read up to as it's scrolled: once it's moved on a
    /// row, or a stretch of a long one, from where it was last kept (not a write for each step of
    /// a scroll).
    fn follow_reading(&self, view: &Entity<ConversationView>, session: &str, cx: &mut Context<Self>) -> Subscription {
        let session = session.to_owned();
        cx.observe(view, move |this, view, cx| {
            let now = view.read(cx).read_at();
            let moved = match (now.as_ref(), this.reading_kept.get(&session)) {
                (None, None) => false,
                (Some(now), Some(kept)) => now.row != kept.row || (now.offset - kept.offset).abs() >= READ_AT_STRETCH,
                _ => true,
            };
            if moved {
                this.keep_place(cx);
            }
        })
    }

    /// Gives a conversation this window's computer key (for remote shown-file downloads) and a
    /// preview callback that opens text files in the side panel.
    fn wire_shown_files(&self, view: &Entity<ConversationView>, cx: &mut Context<Self>) {
        let computer_key = self.place_key.clone();
        let workspace = cx.entity().downgrade();
        let preview = std::rc::Rc::new(move |path: &str, window: &mut Window, cx: &mut App| {
            workspace.update(cx, |workspace, cx| workspace.preview_file(path, window, cx)).ok();
        });
        let workspace = cx.entity().downgrade();
        let show_changes = std::rc::Rc::new(move |_: &mut Window, cx: &mut App| {
            workspace.update(cx, |workspace, cx| workspace.show_panel(Kind::Changes, cx)).ok();
        });
        let now = self.now;
        view.update(cx, |view, cx| {
            view.set_computer_key(computer_key, cx);
            view.set_preview_file(preview);
            view.set_show_changes(show_changes);
            if let Some(now) = now {
                view.set_now(now);
            }
        });
    }

    /// A conversation's Activity: the bot it is, and how a row
    /// opens its conversation (here, in Sessions).
    fn wire_bars(&self, view: &Entity<ConversationView>, bot: Option<String>, cx: &mut Context<Self>) {
        let source = cx.entity().downgrade();
        let bot_look: crate::work_bars::BotLook = std::rc::Rc::new(move |session, cx| {
            source.read_with(cx, |workspace, cx| {
                let own_bot = workspace
                    .listing()
                    .facts(session)
                    .filter(|facts| facts.kind.0 == "bot")
                    .and_then(|facts| facts.bot.as_deref());
                let Reading::Ready(bots) = workspace.area_view(Area::Bots)?.read(cx).bots() else { return None };
                let bot = bots.bots.iter().find(|each| each.session == session || Some(each.id.as_str()) == own_bot)?;
                Some((bot.id.clone(), bot.look.clone()))
            }).ok().flatten()
        });
        let workspace = cx.entity().downgrade();
        let opener: crate::work_bars::Opener = std::rc::Rc::new(move |session: &str, computer: Option<&str>, window: &mut Window, cx: &mut App| {
            workspace.update(cx, |workspace, cx| {
                // (Another computer's too: it opens here, through that computer's box.)
                let _ = computer;
                workspace.open_session_now(session, window, cx);
            }).ok();
        });
        let workspace = cx.entity().downgrade();
        let panel: crate::work_bars::PanelOpener = std::rc::Rc::new(move |arrival: panel::Arrival, _: &mut Window, cx: &mut App| {
            workspace.update(cx, |workspace, cx| workspace.arrive(arrival, cx)).ok();
        });
        view.read(cx).bars().clone().update(cx, |bars, cx| {
            bars.set_opener(opener);
            bars.set_panel_opener(panel);
            bars.set_bot_look(bot_look);
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
        self.sidebar_for_area(window);
        let row = if self.elsewhere_bot(bot, cx).is_some() { format!("elsewhere:{bot}") } else { format!("bot:{bot}") };
        if let Some(view) = self.area_view(Area::Bots).cloned() {
            view.update(cx, |view, cx| view.select(Some(row), cx));
        }
        self.record(Nav::BotChat { bot: bot.to_owned() }, cx);
        self.in_bot_chat = true;
        let client = self.bot_client(bot, session, cx);
        let computer = self.elsewhere_bot(bot, cx).map(|there| there.computer_key);
        let composer = self.composer_on(session, client, computer.clone(), window, cx);
        if self.bot_chat.as_ref().is_none_or(|chat| chat.session != session) {
            let (client, live) = (self.bot_client(bot, session, cx), self.live);
            let view = cx.new(|cx| ConversationView::new(client, &self.client, computer.as_deref(), session.to_owned(), composer.clone(), live, cx));
            self.wire_bars(&view, Some(bot.to_owned()), cx);
            self.wire_wakes(&view, session, cx);
            self.wire_shown_files(&view, cx);
            self._bot_reading = Some(self.follow_reading(&view, session, cx));
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

    /// The bot `bot` on another of your computers, as the box lists it.
    fn elsewhere_bot(&self, bot: &str, cx: &App) -> Option<kuvo_box::bots::Elsewhere> {
        match self.area_view(Area::Bots)?.read(cx).bots() {
            Reading::Ready(bots) if !bots.bots.iter().any(|each| each.id == bot) => bots.elsewhere.iter().find(|each| each.id == bot && !each.unlisted).cloned(),
            _ => None,
        }
    }

    /// The box a bot's conversation goes to: its computer's, for a bot on another of yours.
    fn bot_client(&self, bot: &str, session: &str, cx: &App) -> BoxClient {
        self.elsewhere_bot(bot, cx)
            .and_then(|there| self.client.peer(&there.computer_key))
            .or_else(|| self.client_for(session))
            .unwrap_or_else(|| self.client.clone())
    }

    /// The session the box lists as `bot`'s own conversation now.
    fn bot_session(&self, bot: &str, cx: &App) -> Option<String> {
        match self.area_view(Area::Bots)?.read(cx).bots() {
            Reading::Ready(bots) => bots.bots.iter().find(|each| each.id == bot).map(|each| each.session.clone())
                .or_else(|| bots.elsewhere.iter().find(|each| each.id == bot && !each.unlisted && !each.session.is_empty()).map(|each| each.session.clone())),
            _ => None,
        }
    }

    /// The box's bots were read again: a conversation shown for a bot whose conversation is
    /// another session now (a coding agent's model carried it on) goes to that one.
    fn bots_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh_conversation_wakes(cx);
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

    fn wire_wakes(&self, view: &Entity<ConversationView>, session: &str, cx: &mut Context<Self>) {
        let status = self.area_view(Area::Bots).and_then(|area| {
            let area = area.read(cx);
            let Reading::Ready(bots) = area.bots() else { return None };
            let bot = bots.bots.iter().find(|bot| bot.session == session)?;
            let mut status = crate::wakes::Status::from(bot);
            status.waking &= area.has_fresh_bots();
            Some(status)
        });
        let computer = self.info.as_ref().map(|info| info.name.clone()).unwrap_or_else(|| self.box_name().into());
        view.update(cx, |view, cx| view.set_wakes(status, computer, self.now, cx));
    }

    fn tick_wake_labels(&self, cx: &mut Context<Self>) {
        // The landing's sky moves on a step every five minutes: draw it again when it has.
        if self.is_landing() && self.sky_hour.is_none() {
            let now = time::now_ms();
            let step = |ms: f64| kuvo_ui::sky::stepped(time::hour_of_day(ms));
            if step(now) != step(now - 30_000.) {
                cx.notify();
            }
        }
        if self.area == Area::Bots && let Some(area) = self.area_view(Area::Bots) {
            area.update(cx, |_, cx| cx.notify());
        }
        let chat = self.bot_chat().map(|(_, view)| view).or_else(|| self.conversation());
        if let Some(chat) = chat {
            chat.update(cx, |view, cx| view.tick_wakes(cx));
        }
    }

    fn refresh_conversation_wakes(&self, cx: &mut Context<Self>) {
        if let Open::Conversation { view, session } = &self.open {
            self.wire_wakes(view, session, cx);
        }
        if let Some(chat) = &self.bot_chat {
            self.wire_wakes(&chat.view, &chat.session, cx);
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

    /// A session's composer reads its settings from the owner and sends through this box.
    fn composer_on(&mut self, session: &str, client: BoxClient, computer: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Entity<Composer> {
        let delivery = kuvo_box::queue::DeliveryBox::new(self.client.clone(), computer);
        // Its folder, where its project's own settings may add models to what's offered.
        let folder = self.listing.facts(session).map(|facts| facts.directory.clone());
        if let Some((composer, _)) = self.composers.get(session) {
            let composer = composer.clone();
            composer.update(cx, |composer, cx| {
                composer.set_session_client(client, delivery);
                if self.live && !composer.set_folder(folder, cx) {
                    composer.read_models(cx);
                }
            });
            return composer;
        }
        let live = self.live;
        let composer = cx.new(|cx| Composer::in_folder(client, Target::Session { id: session.to_owned(), place: Where::Engine }, folder, live, cx));
        let subscription = cx.subscribe_in(&composer, window, |this, _, event: &ComposerEvent, window, cx| this.composer_event(event, window, cx));
        // Not on screen until its conversation opens.
        let asked = self.computer_asked();
        composer.update(cx, |composer, cx| {
            composer.set_delivery_box(delivery);
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
        match self.listing.start_project() {
            Some(Some(project)) => StartIn::Project(project.to_owned()),
            Some(None) => StartIn::Chat,
            None if self.listing.is_read() => self.listing.sidebar().projects.first().map_or(StartIn::Chat, |group| StartIn::Project(group.directory.clone())),
            None => StartIn::Chat,
        }
    }

    /// Where a new one starts on a computer (none: this one) before one's picked: its newest
    /// project as the lists have it, else a chat.
    fn default_place_on(&self, computer: Option<&str>) -> StartIn {
        let Some(computer) = computer else { return self.default_place() };
        self.landing_projects(Some(computer)).into_iter().next().map_or(StartIn::Chat, |(directory, _)| StartIn::Project(directory))
    }

    /// The projects a new one can start in on a computer (none: this one), newest first, by
    /// their key in the lists and name.
    pub fn landing_projects(&self, computer: Option<&str>) -> Vec<(String, String)> {
        self.listing
            .sidebar()
            .projects
            .iter()
            .filter(|group| match (computer, kuvo_box::sessions::project_elsewhere(&group.directory)) {
                (None, None) => !group.elsewhere,
                (Some(computer), Some((on, _))) => group.elsewhere && on == computer,
                _ => false,
            })
            .map(|group| (group.directory.clone(), group.name.clone()))
            .collect()
    }

    /// Runs on, as the nearest box says it: the landing composer and New Bot offer it.
    fn take_runs_on(&mut self, rows: Vec<kuvo_box::models::RunsOnComputer>, cx: &mut Context<Self>) {
        self.runs_on = rows.clone();
        let now = self.now();
        self.landing.update(cx, |composer, cx| composer.set_runs_on(rows.clone(), now, cx));
        if let Some(view) = self.area_view(Area::Bots).cloned() {
            view.update(cx, |view, cx| view.set_runs_on(rows, now, cx));
        }
        cx.notify();
    }

    /// Runs on, as the nearest box last said it.
    pub fn runs_on(&self) -> &[kuvo_box::models::RunsOnComputer] {
        &self.runs_on
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
            ComposerEvent::Started { session, project, written, files, sent, first, computer, .. } => {
                // What the box said of what was written (or why it didn't go) goes to the new
                // conversation's own composer; the landing one is left empty.
                self.started.insert(session.clone(), project.clone());
                if let Some(computer) = computer {
                    self.started_on.insert(session.clone(), computer.clone());
                }
                let client = self.client_for(session).unwrap_or_else(|| self.client.clone());
                let composer = self.composer_on(session, client, computer.clone(), window, cx);
                composer.update(cx, |composer, cx| {
                    if let Some(sent) = sent {
                        composer.take_sent(written, files.clone(), sent.clone(), cx);
                    } else {
                        composer.queue_first(session, first.clone(), files.clone(), cx);
                    }
                });
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
            // Another computer picked in Runs on: a new one starts where that computer's lists
            // say (its newest project, else a chat), with what it offers read there.
            ComposerEvent::RunsOn(computer) => {
                let place = self.default_place_on(computer.as_deref());
                self.landing_place = Some(place.clone());
                self.landing_picked = false;
                self.place_panel(cx);
                self.landing.update(cx, |composer, cx| {
                    composer.set_start(place, cx);
                    if composer.reads() {
                        composer.read_models(cx);
                        composer.read_approval(cx);
                    } else {
                        // (Tests and shots: read at once, as the app's reads would hand it.)
                        #[cfg(feature = "stand-in")]
                        composer.read_at_once(cx);
                    }
                });
                cx.notify();
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
        self.keep_place(cx);
        cx.notify();
    }

    /// Whether the side panel goes beside what's in view: a conversation in Sessions (or the
    /// landing view), or a bot's conversation in Bots.
    pub fn panel_here(&self) -> bool {
        self.area == Area::Sessions || self.bot_chat().is_some()
    }

    /// The conversation the side panel goes beside: a bot's in Bots, the open one in Sessions.
    fn panel_session(&self) -> Option<&str> {
        match self.bot_chat() {
            Some(_) => self.bot_chat.as_ref().map(|chat| chat.session.as_str()),
            None => self.opened(),
        }
    }

    /// Opens the side panel on `kind`, in the conversation in view: its tab, or a new one.
    pub fn show_panel(&mut self, kind: Kind, cx: &mut Context<Self>) {
        // (In the conversation in view's tabs; opened first: what's asked for is what it starts on.)
        self.place_panel(cx);
        self.panel.update(cx, |panel, cx| panel.open(kind, cx));
        if !self.panel_open {
            self.toggle_panel(cx);
        }
    }

    /// Opens the side panel on what the conversation sent you to (from its Outputs).
    pub fn arrive(&mut self, arrival: panel::Arrival, cx: &mut Context<Self>) {
        self.place_panel(cx);
        self.panel.update(cx, |panel, cx| panel.arrive(arrival, cx));
        if !self.panel_open {
            self.toggle_panel(cx);
        }
    }

    /// Opens `path` in its own tab in the side panel (a file the conversation names or shows).
    pub fn preview_file(&mut self, path: &str, _window: &mut Window, cx: &mut Context<Self>) {
        self.show_panel(Kind::File(path.to_owned()), cx);
    }

    /// The conversation the panel follows, and the folder it shows: the session's in view (its
    /// worktree's, for one in a worktree; a bot's in Bots), or on the landing view the project a
    /// new one starts in.
    fn place_panel(&mut self, cx: &mut Context<Self>) {
        let key = self.panel_session().map(str::to_owned);
        let place = match self.panel_session() {
            Some(session) => self.listing.facts(session).filter(|facts| !facts.kind.is_chat()).map(|facts| Place { directory: facts.directory.clone(), session: Some(session.to_owned()) }),
            None => match self.landing_place() {
                StartIn::Project(directory) => Some(Place { directory, session: None }),
                _ => None,
            },
        };
        // (Another computer's session: its files, terminals, browser and Changes are that one's.)
        let elsewhere = self.bot_chat().map(|(_, view)| view).or_else(|| self.conversation())
            .map(|view| view.read(cx).client())
            .filter(|client| !client.same_connection(&self.client))
            .cloned();
        self.panel.update(cx, |panel, cx| panel.set_conversation_on(key, place, elsewhere, cx));
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
        let widest = self.widest_panel(wide, cx);
        if widest >= panel::NARROWEST {
            self.panel_width = (width + (from - x)).clamp(panel::NARROWEST, widest);
        }
        cx.notify();
    }

    pub fn end_resize(&mut self, cx: &mut Context<Self>) {
        if self.resizing.take().is_some() {
            self.keep_place(cx);
            cx.notify();
        }
    }

    /// Its width as it first opened.
    pub fn reset_panel_width(&mut self, cx: &mut Context<Self>) {
        self.panel_width = panel::WIDTH;
        self.keep_place(cx);
        cx.notify();
    }

    /// The side panel over the conversation, in a window too narrow for it beside it: a sheet that
    /// slides in from the right over a dimmed conversation, which closes it when clicked.
    fn panel_sheet(&self, viewport: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let glass = panel::glass(cx);
        let room = viewport - rail::WIDTH - CARD_EDGE;
        let width = self.panel_width.min(room - px(48.)).max(px(0.));
        div()
            .absolute()
            .inset_0()
            .child(
                div()
                    .id("panel-scrim")
                    .debug_selector(|| "panel-scrim".into())
                    .absolute()
                    .inset_0()
                    .bg(glass.scrim)
                    .occlude()
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.panel_open {
                            this.toggle_panel(cx);
                        }
                    })),
            )
            .child(
                div()
                    .id("panel-sheet")
                    .debug_selector(|| "panel-sheet".into())
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right_0()
                    .w(width)
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .border_l_1()
                    .border_color(glass.sheet_edge)
                    .shadow(theme.elevated_shadow())
                    .occlude()
                    .child(self.panel.clone())
                    // It slides in, in the app (tests and shots see where it comes to rest).
                    .map(|sheet| if self.live {
                        sheet.with_animation(
                            "panel-sheet-in",
                            gpui::Animation::new(kuvo_ui::motion::MotionDuration::Slow.duration()).with_easing(gpui::ease_out_quint()),
                            move |sheet, progress| sheet.right(-(width * (1. - progress))),
                        ).into_any_element()
                    } else {
                        sheet.into_any_element()
                    }),
            )
            .into_any_element()
    }

    /// The side panel, in the raised panel beside the conversation, with its edge to drag.
    fn panel_card(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        // In a window narrower than it was made for, the panel gives way before the conversation
        // does; its width is kept for a wider one.
        let width = self.panel_width.min(self.widest_panel(window.viewport_size().width, cx));
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
            self.find_task = self.search_conversations(query, Self::found, cx);
        }
        self.prune_session_popover(cx);
        cx.notify();
    }

    fn search_conversations(
        &mut self,
        query: String,
        done: fn(&mut Self, &str, kuvo_box::Result<Vec<Found>>, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> Option<Task<()>> {
        let client = self.client.clone();
        if self.live {
            Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SEARCH_PAUSE).await;
                let asked = query.clone();
                let found = cx.background_executor().spawn(async move { client.search(&asked, SEARCH_MOST) }).await;
                this.update(cx, |this, cx| done(this, &query, found, cx)).ok();
            }))
        } else {
            // Tests and shots receive the stand-in's answer immediately.
            let found = client.search(&query, SEARCH_MOST);
            done(self, &query, found, cx);
            None
        }
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
        if !self.sidebar_available(cx) {
            return;
        }
        self.bring_out_sidebar(window);
        if self.open_quick_search(window, cx).is_some() {
            self.keep_place(cx);
            return;
        }
        if self.area != Area::Sessions {
            self.show_area(Area::Sessions, window, cx);
        }
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
            Some(Line::More(directory)) if self.pages_lists() => self.show_more(&directory, cx),
            Some(Line::More(directory)) => self.toggle_all(&directory, cx),
            Some(Line::Fewer(directory)) => self.show_fewer(&directory, cx),
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
        self.waiting.clone()
    }

    /// Your computers as this app reaches them, for the switcher.
    pub fn switching(&self) -> Option<&crate::home::Switching> {
        self.switching.as_ref()
    }

    /// Whether this workspace is the one the window currently shows: background computers keep
    /// their workspaces alive, while only the shown one updates shared bot faces (`bot_face`).
    pub fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        self.image_viewer.update(cx, |viewer, cx| viewer.set_shown(shown, cx));
        if let Some(view) = self.area_view(Area::Bots).cloned() {
            view.update(cx, |view, cx| view.set_shown(shown, cx));
        }
    }

    pub fn set_switching(&mut self, switching: Option<crate::home::Switching>, cx: &mut Context<Self>) {
        if self.switching != switching {
            // Why this app isn't paired, whole, on the Computers pages (the switcher's line is short).
            let pairing = switching.as_ref().and_then(|switching| {
                use kuvo_box::boxes::{Pairing, Reach};
                match &switching.pairing {
                    Pairing::Paired => switching.rows.iter().find_map(|row| match &row.reach {
                        Reach::Readd(refusal) => Some(Pairing::Readd(refusal.clone())),
                        Reach::Waiting(message) => Some(Pairing::Checking(message.clone())),
                        _ => None,
                    }),
                    state => Some(state.clone()),
                }
            });
            if let Some(page) = self.area_view(Area::Computers).and_then(|view| view.read(cx).computers_page().cloned()) {
                page.update(cx, |page, cx| page.set_pairing(pairing, cx));
            }
            self.switching = switching;
            // (Gone with the header, the card doesn't open by itself with the next release.)
            if self.fleet_header().is_none() && self.fleet_card.retrying.is_none() {
                self.fleet_card = FleetCard::default();
            }
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

    pub(crate) fn sidebar_toggle_focus(&self) -> &FocusHandle {
        &self.sidebar_toggle_focus
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

    pub fn open_insights(&mut self, filter: Option<crate::area::UsageFilter>, window: &mut Window, cx: &mut Context<Self>) {
        self.insights.update(cx, |insights, cx| insights.ring(filter, cx));
        self.show_area(Area::Insights, window, cx);
    }

    /// The Library's "Show in conversation": the conversation, brought to the message once it's there.
    pub fn open_session_at(&mut self, session: &str, message: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        self.opening_session = None;
        // A row or a session just started here already has the box's address. Links can arrive
        // before the merged list, so ask the nearest box where to open one it hasn't listed yet.
        if self.live && self.listing.facts(session).is_none() && !self.started_on.contains_key(session) {
            let (session, message) = (session.to_owned(), message.map(str::to_owned));
            let nearest = self.client.clone();
            let id = session.clone();
            let read = cx.background_executor().spawn(async move { nearest.where_is(&id) });
            self.opening_session = Some(cx.spawn_in(window, async move |this, cx| {
                let located = read.await;
                this.update_in(cx, |this, window, cx| {
                    let (client, computer) = match located {
                        Ok(kuvo_box::messages::Location::Peer { on }) => {
                            let Some(client) = this.client_for_peer(&on) else { return };
                            (client, Some(on.id))
                        }
                        _ => (this.client.clone(), None),
                    };
                    this.open_session_on(&session, client, computer, window, cx);
                    this.reveal_in_session(&session, message.as_deref(), cx);
                }).ok();
            }));
            return;
        }
        let Some(client) = self.client_for(session) else { return };
        let computer = self.listing.computer_of(session).map(|on| on.id.clone()).or_else(|| self.started_on.get(session).cloned());
        self.open_session_on(session, client, computer, window, cx);
        self.reveal_in_session(session, message, cx);
    }

    fn reveal_in_session(&self, session: &str, message: Option<&str>, cx: &mut Context<Self>) {
        let view = self.bot_chat().map(|(_, view)| view).or_else(|| self.conversation());
        if let Some(message) = message
            && let Some(view) = view.filter(|view| view.read(cx).session() == session)
        {
            view.update(cx, |view, cx| view.reveal_message(message, cx));
        }
    }

    pub fn library(&self) -> &Entity<crate::library::LibraryView> {
        &self.library
    }

    #[cfg(feature = "stand-in")]
    pub fn library_sample(&mut self, slug: &str, now: f64, window: &mut Window, cx: &mut Context<Self>) {
        self.library.update(cx, |view, cx| view.sample(slug, now, window, cx));
        self.show_area(Area::Library, window, cx);
    }

    pub fn marketplace(&self) -> &Entity<crate::marketplace::MarketplaceView> {
        &self.marketplace
    }

    /// The Marketplace, at one of its areas (New Bot's "ready-made agents").
    pub fn show_marketplace(&mut self, section: crate::marketplace::Section, window: &mut Window, cx: &mut Context<Self>) {
        self.marketplace.update(cx, |view, cx| view.show_section(section, cx));
        self.show_area(Area::Marketplace, window, cx);
    }

    #[cfg(feature = "stand-in")]
    pub fn marketplace_sample(&mut self, slug: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.show_area(Area::Marketplace, window, cx);
        self.marketplace.update(cx, |view, cx| view.sample(slug, window, cx));
    }

    #[cfg(feature = "stand-in")]
    pub fn insights_sample(&mut self, slug: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.insights.update(cx, |view, cx| view.sample(slug, cx));
        self.show_area(Area::Insights, window, cx);
    }

    pub fn show_area(&mut self, area: Area, window: &mut Window, cx: &mut Context<Self>) {
        self.session_with_bot_strip = false;
        if area == Area::Queue { self.queue_opened(cx); }
        self.area = area;
        self.sidebar_for_area(window);
        if area == Area::Insights || area == Area::Library || area == Area::Marketplace {
            self.record(Nav::Area(area, None), cx);
        }
        // The Library's sidebar lists every bot, as the Bots list has them: read it, if it isn't yet.
        if area == Area::Library
            && let Some(bots) = self.area_view(Area::Bots).cloned()
            && !matches!(bots.read(cx).bots(), Reading::Ready(_))
        {
            bots.update(cx, |view, cx| view.read_list(cx));
        }
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

    /// The sidebar as another area opens: a wide window's stays as you left it (folded or not,
    /// one state for every area), a narrow one's comes out with the area's list, as it starts
    /// folded there.
    fn sidebar_for_area(&mut self, window: &Window) {
        self.narrow = window.viewport_size().width < NARROW;
        if self.narrow {
            self.shown_narrow = true;
        }
    }

    /// A press on an area's rail button: the area open folds its sidebar away (or brings it back);
    /// another opens.
    pub fn rail_pressed(&mut self, area: Area, window: &mut Window, cx: &mut Context<Self>) {
        if self.area == Area::Insights && area == Area::Insights { return; }
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
        if !self.sidebar_shown() {
            self.end_sidebar_resize(cx);
        }
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
        // A computer's settings are under Computers › that computer (a section named alone is this
        // computer's); your own, in Settings.
        let (area, place) = match section {
            yours if yours.starts_with("yours/") => (Area::Settings, yours.to_owned()),
            place if place.starts_with("box/") => (Area::Computers, place.to_owned()),
            named => (Area::Computers, crate::settings::box_key("", &named.to_lowercase().replace(' ', "-"))),
        };
        if let Some(view) = self.area_view(area).cloned() {
            view.update(cx, |view, cx| view.select(Some(place), cx));
        }
        self.show_area(area, window, cx);
    }

    fn area_picked(&mut self, view: &Entity<AreaView>, window: &mut Window, cx: &mut Context<Self>) {
        let (area, picked) = view.read_with(cx, |view, _| (view.area(), view.selected().map(str::to_owned)));
        if area == Area::Bots && self.area == Area::Sessions && self.session_with_bot_strip {
            self.area = Area::Bots;
        }
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
            // A bot on another of your computers opens into its chat as one here does, through
            // that computer's box.
            if let Some(there) = picked.as_deref().and_then(|key| key.strip_prefix("elsewhere:")).and_then(|bot| self.elsewhere_bot(bot, cx))
                && !there.session.is_empty()
                && self.client.peer(&there.computer_key).is_some()
            {
                self.open_bot_chat(&there.id, &there.session, None, window, cx);
                return;
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
        let reachability = Reachability::of(&read);
        if self.computers_reachability != reachability {
            self.computers_reachability = reachability;
            cx.notify();
        }
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
        self.restoring.reading = spot.reading;
        // (Opened as it was, not toggled: that would keep a place before this one's restored.)
        if spot.panel_open && !self.panel_open {
            self.panel_open = true;
            self.place_panel(cx);
            self.panel.update(cx, |panel, cx| panel.set_shown(true, cx));
        }
        if let Some(width) = spot.panel_width {
            self.panel_width = px(width).max(panel::NARROWEST);
        }
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
        self.read_from_kept(&session, cx);
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
    /// the bot's still there; gone, Bots shows its own page. Until the list can say (its box not
    /// answering yet, as while it starts, or its other computers' bots not said yet), it waits:
    /// a bot not read yet isn't gone, and its page isn't where the window was.
    fn restore_bot(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bot) = self.restoring.bot.clone() else { return };
        let Some(view) = self.area_view(Area::Bots).cloned() else { return };
        let session = self.bot_session(&bot, cx);
        match view.read(cx).bots() {
            Reading::NotYet | Reading::Failed(_) => return,
            Reading::Ready(_) if self.area != Area::Bots => return,
            Reading::Ready(bots) if session.is_none() => {
                let listed_elsewhere = bots.elsewhere.iter().any(|each| each.id == bot && !each.unlisted);
                if listed_elsewhere || bots.held_unsaid {
                    return;
                }
            }
            Reading::Ready(_) => {}
        }
        self.restoring.bot = None;
        match session {
            Some(session) => {
                let yours = window.focused(cx).filter(|focused| *focused != self.focus && *focused != self.landing.focus_handle(cx));
                self.restoring_now = true;
                let (folded, shown_narrow) = (self.folded, self.shown_narrow);
                self.open_bot_chat(&bot, &session, None, window, cx);
                (self.folded, self.shown_narrow) = (folded, shown_narrow);
                self.restoring_now = false;
                self.read_from_kept(&session, cx);
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

    /// The conversation of `session` just opened again goes to where it was read up to last time.
    fn read_from_kept(&mut self, session: &str, cx: &mut Context<Self>) {
        let Some(at) = self.restoring.reading.remove(session) else { return };
        let view = match &self.bot_chat {
            Some(chat) if chat.session == session => Some(chat.view.clone()),
            _ => self.conversation().filter(|_| self.opened() == Some(session)).cloned(),
        };
        if let Some(view) = view {
            view.update(cx, |view, cx| view.read_from(&at, cx));
        }
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
        // Where each conversation open is read up to; what's still to open, as it was kept.
        let mut reading = std::collections::BTreeMap::new();
        if self.restoring.session.is_some() || self.restoring.bot.is_some() {
            reading = self.restoring.reading.clone();
        }
        let open = self.conversation().zip(self.opened()).map(|(view, session)| (session.to_owned(), view.clone()));
        let bots = self.bot_chat.as_ref().map(|chat| (chat.session.clone(), chat.view.clone()));
        for (session, view) in open.into_iter().chain(bots) {
            match view.read(cx).read_at() {
                Some(at) => reading.insert(session, at),
                None => reading.remove(&session),
            };
        }
        self.reading_kept = reading.clone();
        let panel_width = (self.panel_width != panel::WIDTH).then(|| f32::from(self.panel_width));
        let spot = crate::yours::Spot { sidebar_folded: self.folded, area: self.kept_area.clone(), session, bot, picked, reading, panel_open: self.panel_open, panel_width };
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
        self.opening_session = None;
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

    /// The face of the bot whose conversation shows, for the title bar beside its name (still
    /// while it moves).
    pub fn bot_chat_face(&self, cx: &App) -> Option<AnyElement> {
        let (bot, _) = self.bot_chat()?;
        let target = kuvo_box::moves::Target::new(kuvo_box::moves::Kind::Bot, bot);
        let moving = self.moves.read(cx).row_status_words(&target).is_some();
        let Reading::Ready(bots) = self.area_view(Area::Bots)?.read(cx).bots() else { return None };
        let look = match bots.bots.iter().find(|each| each.id == bot) {
            Some(here) => here.look.clone(),
            None => bots.elsewhere.iter().find(|each| each.id == bot)?.look.clone()?,
        };
        Some(if moving { crate::bot_face::still_bot_face(bot, &look, px(22.)) } else { areas::bot_face(bot, &look, px(22.)) })
    }

    /// A bot's conversation in Bots, under the title bar, which says whose it is (as Sessions has
    /// it: one header).
    fn bot_chat_view(&self, chat: &Entity<ConversationView>) -> AnyElement {
        div().debug_selector(|| "bot-chat".into()).size_full().flex().flex_col().child(div().flex_1().min_h_0().child(chat.clone().cached(gpui::StyleRefinement::default().size_full()))).into_any_element()
    }

    /// The main view's title and what's beside it, as the title bar shows them: a conversation's
    /// title and its project, or the page an area shows.
    pub fn title(&self, cx: &App) -> (Option<SharedString>, Option<SharedString>) {
        if self.area == Area::Insights { return (Some(self.area.name().into()), None); }
        if self.area == Area::Library {
            let (title, beside) = self.library.read(cx).title();
            return (Some(title), beside);
        }
        if self.area == Area::Marketplace {
            let (title, beside) = self.marketplace.read(cx).title();
            return (Some(title), beside);
        }
        // A bot's conversation, titled as Sessions titles one: the bot's name, and beside it what
        // its move is at, or its job.
        if let Some((bot, _)) = self.bot_chat() {
            let target = kuvo_box::moves::Target::new(kuvo_box::moves::Kind::Bot, bot);
            let moving = self.moves.read(cx).row_status_words(&target).map(|words| SharedString::from(words.to_owned()));
            let listed = self.area_view(Area::Bots).and_then(|view| match view.read(cx).bots() {
                // (Here, or on another of your computers.)
                Reading::Ready(bots) => bots.bots.iter().find(|each| each.id == bot).map(|bot| (bot.name.clone(), bot.job.clone()))
                    .or_else(|| bots.there().into_iter().find(|there| there.id == bot).map(|there| (there.name, there.job))),
                _ => None,
            });
            let (name, job) = listed.unzip();
            let job = job.filter(|job| !job.trim().is_empty()).map(SharedString::from);
            return (Some(name.map_or_else(|| SharedString::from(Area::Bots.name()), SharedString::from)), moving.or(job));
        }
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
        // (Another computer's session: its menu is that computer's, not asked of this one's box.)
        if self.client.gate().is_held() || self.listing.computer_of(session).is_some() {
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

    pub fn moves(&self) -> &Entity<crate::moves::MovesView> {
        &self.moves
    }

    pub fn move_chat_menu(&mut self, session: &str, anchor: &PopoverAnchor, window: &mut Window, cx: &mut Context<Self>) {
        let handle = match anchor {
            PopoverAnchor::TitleBar => self.session_menu.title_menu.clone(),
            PopoverAnchor::Row(key) => self.session_menu.row_menu_handle(key),
        };
        let target = kuvo_box::moves::Target::new(kuvo_box::moves::Kind::Chat, session);
        let moves = self.moves.clone();
        window.defer(cx, move |window, cx| {
            handle.open(crate::moves::MovesView::menu(&moves, target, window, cx), window, cx);
        });
    }

    fn is_move_chat(&self, session: &str) -> bool {
        self.listing().facts(session).is_some_and(|facts| facts.kind.is_chat())
    }

    pub fn open_moved(&mut self, target: &kuvo_box::moves::Target, window: &mut Window, cx: &mut Context<Self>) {
        use kuvo_box::moves::Kind;
        if target.kind == Kind::Chat {
            self.open_session(&target.id, window, cx);
            return;
        }
        self.show_area(Area::Bots, window, cx);
        let kind = if target.kind == Kind::Bot { "bot" } else { "group" };
        if let Some(bots) = self.area_view(Area::Bots).cloned() {
            bots.update(cx, |bots, cx| bots.select(Some(format!("{kind}:{}", target.id)), cx));
            self.area_picked(&bots, window, cx);
        }
    }

    pub fn open_row_menu(&mut self, session: &str, key: &str, window: &mut Window, cx: &mut Context<Self>) -> Entity<Menu> {
        let anchor = PopoverAnchor::Row(key.to_owned());
        let move_chat = self.is_move_chat(session);
        if !self.live {
            let result = self.client.session_actions(session);
            if let Ok(actions) = &result {
                self.session_menu.cache_actions(session, actions.clone());
            }
            return session_menu::menu_from_actions(result, session.to_owned(), anchor, cx.entity().downgrade(), move_chat, cx);
        }
        session_menu::open_actions_menu(
            self.client.clone(),
            self.live,
            session.to_owned(),
            anchor,
            cx.entity().downgrade(),
            move_chat,
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
                false,
                cx,
            );
        }
        session_menu::open_actions_menu(
            self.client.clone(),
            self.live,
            session.to_owned(),
            PopoverAnchor::TitleBar,
            cx.entity().downgrade(),
            false,
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

    /// Add Project (the + at Projects, File › Open Folder…): the system's folder picker, then the
    /// folder picked kept as a project by this Mac's box (`PUT kuvo/projects`); the list shows it
    /// as the box says. Another computer's folders can't be picked here: see
    /// [`Workspace::can_add_project`].
    pub fn add_project(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_add_project() {
            return;
        }
        let picked = cx.prompt_for_paths(gpui::PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Add Project".into()) });
        self.tasks.push(cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let Some(folder) = paths.into_iter().next() else { return };
            this.update(cx, |this, cx| this.add_project_folder(folder.to_string_lossy().into_owned(), cx)).ok();
        }));
    }

    /// Whether Add Project can pick a folder: the system's picker shows this Mac's folders, so only
    /// while the window shows this Mac's own sessions.
    pub fn can_add_project(&self) -> bool {
        self.client.here() && !self.client.gate().is_held()
    }

    fn add_project_folder(&mut self, directory: String, cx: &mut Context<Self>) {
        self.session_menu.action_error = None;
        let client = self.client.clone();
        if !self.live {
            match client.add_project(&directory) {
                Ok(()) => self.refresh_list_now(cx),
                Err(error) => self.session_menu.action_error = Some(format!("The project wasn’t added: {}", kuvo_box::accounts::said(&error))),
            }
            cx.notify();
            return;
        }
        self.session_menu.action_tasks.push(cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { client.add_project(&directory) }).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => this.refresh_list_after_action(cx),
                    Err(error) => this.session_menu.action_error = Some(format!("The project wasn’t added: {}", kuvo_box::accounts::said(&error))),
                }
                cx.notify();
            })
            .ok();
        }));
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

    fn sidebar_width(&self, viewport: Pixels, cx: &App) -> Pixels {
        self.sidebar_width_within(self.sidebar_viewport(viewport, cx), cx)
    }

    /// The sidebar's width with `room` for it and the conversation.
    fn sidebar_width_within(&self, room: Pixels, cx: &App) -> Pixels {
        let preferred = self.sidebar_drag.as_ref().map_or_else(
            || sidebar::preferred_width(crate::yours::sidebar_width(cx)),
            |drag| drag.width,
        );
        sidebar::visible_width(sidebar::fit_width(preferred, room, self.can_show_bot_strip()))
    }

    fn sidebar_available(&self, cx: &App) -> bool {
        // (A computer that can't be reached keeps its sidebar, its rows as last read; one held
        // has nothing to list, but in Settings.)
        self.first_run().is_none() && !self.awaits_box()
            && (self.area == Area::Settings || self.blocked(cx).is_none_or(|blocked| blocked.kind != "held"))
    }

    fn visible_sidebar_width(&self, viewport: Pixels, cx: &App) -> Option<Pixels> {
        (self.area != Area::Insights && self.sidebar_shown() && self.sidebar_available(cx)).then(|| self.sidebar_width(viewport, cx))
    }

    fn compact_sidebar_visible(&self, viewport: Pixels, cx: &App) -> bool {
        self.visible_sidebar_width(viewport, cx).is_some_and(sidebar::compact_bots)
    }

    fn panel_shown(&self, cx: &App) -> bool {
        self.panel_here() && self.panel_open && !self.client.gate().is_held()
            && self.sidebar_available(cx)
    }

    fn sidebar_viewport(&self, viewport: Pixels, cx: &App) -> Pixels {
        if self.panel_shown(cx) && self.panel_docks(viewport, cx) {
            viewport - panel::NARROWEST - CARD_EDGE
        } else {
            viewport
        }
    }

    /// Whether a window `wide` has room for the side panel beside the conversation, each at its
    /// narrowest (the sidebar fit beside them); in one that hasn't, it slides over the conversation.
    pub fn panel_docks(&self, wide: Pixels, cx: &App) -> bool {
        let shows_sidebar = self.area != Area::Insights && self.sidebar_shown() && self.sidebar_available(cx);
        let sidebar = if shows_sidebar { self.sidebar_width_within(wide - panel::NARROWEST - CARD_EDGE, cx) } else { px(0.) };
        wide - rail::WIDTH - CARD_EDGE - sidebar - panel::CONVERSATION_NARROWEST - CARD_EDGE >= panel::NARROWEST
    }

    fn can_show_bot_strip(&self) -> bool {
        self.area == Area::Bots || (self.area == Area::Sessions && self.session_with_bot_strip)
    }

    fn end_sidebar_resize(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.sidebar_drag.take() {
            let width = sidebar::visible_width(drag.width);
            if width != drag.initial {
                crate::yours::change(crate::yours::Change::SidebarWidth(f32::from(width)), cx);
            }
            cx.notify();
        }
    }

    fn sidebar_handle(&self, width: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let dragging = self.sidebar_drag.is_some();
        deferred(
            div()
                .id("sidebar-edge")
                .debug_selector(|| "sidebar-edge".into())
                .absolute()
                .right(px(-4.))
                .top_0()
                .bottom_0()
                .w(px(8.))
                .flex()
                .justify_center()
                .cursor_col_resize()
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if event.click_count == 2 {
                        this.sidebar_drag = None;
                        crate::yours::change(crate::yours::Change::SidebarWidth(sidebar::WIDTH), cx);
                    } else {
                        this.sidebar_drag = Some(SidebarDrag { from: event.position.x, initial: width, width });
                    }
                    cx.notify();
                }))
                .child(div().w(px(2.)).h_full().when(dragging, |d| d.bg(c.accent)).hover(|s| s.bg(c.border_strong))),
        ).into_any_element()
    }

    /// The widest the side panel goes in a window `wide`: never leaving the conversation narrower
    /// than its narrowest (the rail, the area's sidebar and the panel's edge apart).
    fn widest_panel(&self, wide: Pixels, cx: &App) -> Pixels {
        let sidebar = self.visible_sidebar_width(wide, cx).unwrap_or(px(0.));
        let room = wide - rail::WIDTH - CARD_EDGE - sidebar - panel::CONVERSATION_NARROWEST - CARD_EDGE;
        // Keep the conversation and left sidebar at their minimums. At the smallest
        // window sizes the right panel can render below its usual minimum.
        room.clamp(px(0.), panel::WIDEST)
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.area != Area::Settings
            && let Some(settings) = self.area_view(Area::Settings).and_then(|view| view.read(cx).settings()).cloned()
        {
            settings.update(cx, |settings, _| settings.hidden());
        }
        // (And a computer's, under Computers, off screen.)
        if let Some(view) = self.area_view(Area::Computers)
            && let Some(settings) = view.read(cx).computer_settings().cloned()
            && (self.area != Area::Computers || view.read(cx).settings().is_none())
        {
            settings.update(cx, |settings, _| settings.hidden());
        }
        let theme = cx.theme().clone();
        let c = &theme.colors;
        let viewport = window.viewport_size().width;
        // A narrow window folds its sidebar away until it is brought out again.
        let narrow = viewport < NARROW;
        if self.narrow != narrow {
            self.shown_narrow = false;
        }
        self.narrow = narrow;
        let compact = self.compact_sidebar_visible(viewport, cx);
        if !compact {
            self.dismiss_hidden_quick_search(window, cx);
        }
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
            let blocked = self.blocked(cx);
            let words: AnyElement = match (blocked, why) {
                (Some(blocked), _) => crate::blocked::render(&blocked, cx),
                (None, Some(why)) => div().debug_selector(|| "window-waiting-why".into()).max_w(px(420.)).text_color(c.text_muted).child(why.clone()).into_any_element(),
                _ => kuvo_ui::small::LoadingLabel::new("window-starting", "Starting…").into_any_element(),
            };
            return div()
                .debug_selector(|| "window-waiting".into())
                .track_focus(&self.focus)
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
        // The side panel shows the tabs of the conversation beside it: a bot's in Bots, the open
        // one in Sessions (nothing changes where it's that one's already).
        if self.panel_shown(cx) {
            self.place_panel(cx);
        }
        let in_insights = self.area == Area::Insights;
        let show_insights = in_insights && self.client.gate().has("usage.insights");
        let insights = self.insights.clone();
        insights.update(cx, |view, cx| view.shown(show_insights, cx));
        // The Library only where this computer's Kuvo has one: its rail button, ⌃7 and View ›
        // Library are hidden elsewhere, and a window left on it goes back to Sessions.
        let library_here = self.client.drawn_gate().has("library");
        if window.is_window_active() {
            crate::app_menu::show_library(library_here, cx);
        }
        if self.area == Area::Library && !library_here && self.client.gate() != kuvo_box::Gate::Unknown {
            cx.defer_in(window, |this, window, cx| {
                if this.area == Area::Library && !this.client.gate().has("library") {
                    this.show_area(Area::Sessions, window, cx);
                }
            });
        }
        // The Marketplace, likewise, only where the box serves it.
        let market_here = self.client.drawn_gate().has("marketplace");
        if self.area == Area::Marketplace && !market_here && self.client.gate() != kuvo_box::Gate::Unknown {
            cx.defer_in(window, |this, window, cx| {
                if this.area == Area::Marketplace && !this.client.gate().has("marketplace") {
                    this.show_area(Area::Sessions, window, cx);
                }
            });
        }
        let in_market = self.area == Area::Marketplace && market_here;
        let marketplace = self.marketplace.clone();
        marketplace.update(cx, |view, cx| view.shown(in_market, cx));
        let in_library = self.area == Area::Library && library_here;
        let library = self.library.clone();
        let show_library = in_library;
        library.update(cx, |view, cx| view.shown(show_library, cx));
        if in_library {
            let bots = self.library_bots(cx);
            library.update(cx, |view, cx| view.set_bots(bots, cx));
        }
        let sessions = self.area == Area::Sessions;
        // A computer whose Kuvo this app can't read (older, held) is each area's title and one
        // quiet line, with what moves it on, in every area but Settings: never a page of its own,
        // never one opened empty. One that can't be reached keeps the rows it last had.
        let blocked = (self.area != Area::Settings).then(|| self.blocked(cx)).flatten().filter(|blocked| blocked.kind == "held");
        let content: AnyElement = if let Some(blocked) = &blocked {
            let title = match self.area_view(self.area) {
                Some(view) if self.area != Area::Sessions => view.read(cx).title(cx),
                _ => self.area.name().into(),
            };
            div().debug_selector(|| "held-area".into()).size_full().child(crate::blocked::quiet(blocked, title, cx)).into_any_element()
        } else if in_insights {
            insights.into_any_element()
        } else if in_library {
            library.clone().into_any_element()
        } else if in_market {
            marketplace.clone().into_any_element()
        } else { match (&self.open, self.area_view(self.area)) {
            (_, Some(view)) if !sessions => match (self.bot_chat(), view.read(cx).settings(), view.read(cx).main()) {
                (Some((_, chat)), _, _) => self.bot_chat_view(chat),
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
        let width = self.visible_sidebar_width(viewport, cx);
        let sidebar_area = if compact && !in_library && !in_market { Area::Bots } else { self.area };
        let area_view = self.area_view(sidebar_area).cloned();
        if let Some(view) = &area_view {
            let mode = match (compact, sessions) {
                (false, _) => areas::SidebarMode::Full,
                (true, false) => areas::SidebarMode::Avatars,
                (true, true) => areas::SidebarMode::SessionAvatars(
                    self.opened().and_then(|id| self.listing.facts(id)).and_then(|facts| facts.bot.clone()),
                ),
            };
            view.update(cx, |view, cx| view.set_sidebar_mode(mode, cx));
        }
        let sidebar = width.map(|width| {
            let content = match area_view {
                _ if in_library => library.update(cx, |view, cx| view.sidebar(cx)),
                _ if in_market => marketplace.update(cx, |view, cx| view.sidebar(cx)),
                Some(view) => view.into_any_element(),
                None => sidebar::render(self, window, cx),
            };
            div()
                .id("area-sidebar")
                .debug_selector(|| "area-sidebar".into())
                .relative()
                .w(width)
                .h_full()
                .flex_none()
                .bg(c.sidebar)
                .border_r_1()
                .border_color(c.border)
                .child(content)
                .child(self.sidebar_handle(width, cx))
        });
        let docks = self.panel_docks(viewport, cx);
        let panel = (self.panel_shown(cx) && docks).then(|| self.panel_card(window, cx));
        let sheet = (self.panel_shown(cx) && !docks).then(|| self.panel_sheet(viewport, cx));
        div()
            .relative()
            .key_context("Workspace")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &NewSession, window, cx| this.new_session(None, window, cx)))
            .on_action(cx.listener(|this, _: &OpenFolder, window, cx| this.add_project(window, cx)))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| this.focus_search(window, cx)))
            .on_action(cx.listener(|this, _: &RenameSession, window, cx| this.run_shortcut("rename", window, cx)))
            .on_action(cx.listener(|this, _: &ArchiveSession, window, cx| this.run_shortcut("archive", window, cx)))
            .on_action(cx.listener(|this, _: &DeleteSession, window, cx| this.run_shortcut("delete", window, cx)))
            .on_action(cx.listener(|this, _: &ToggleSidebar, window, cx| this.toggle_sidebar(window, cx)))
            .on_action(cx.listener(|this, _: &TogglePanel, window, cx| {
                if !this.panel_here() {
                    this.show_area(Area::Sessions, window, cx);
                }
                this.toggle_panel(cx);
            }))
            .on_action(cx.listener(|this, _: &ShowChanges, window, cx| {
                if !this.panel_here() {
                    this.show_area(Area::Sessions, window, cx);
                }
                this.show_panel(Kind::Changes, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowFiles, window, cx| {
                if !this.panel_here() {
                    this.show_area(Area::Sessions, window, cx);
                }
                this.show_panel(Kind::Files, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowSessions, window, cx| this.show_area(Area::Sessions, window, cx)))
            .on_action(cx.listener(|this, _: &ShowBots, window, cx| this.show_area(Area::Bots, window, cx)))
            .on_action(cx.listener(|this, _: &ShowQueue, window, cx| this.show_area(Area::Queue, window, cx)))
            .on_action(cx.listener(|this, _: &ShowComputers, window, cx| this.show_area(Area::Computers, window, cx)))
            .on_action(cx.listener(|this, _: &ShowSettings, window, cx| this.show_area(Area::Settings, window, cx)))
            .on_action(cx.listener(|this, _: &ShowUsage, window, cx| this.show_area(Area::Insights, window, cx)))
            .when(library_here, |d| d.on_action(cx.listener(|this, _: &ShowLibrary, window, cx| this.show_area(Area::Library, window, cx))))
            .when(market_here, |d| d.on_action(cx.listener(|this, _: &ShowMarketplace, window, cx| this.show_area(Area::Marketplace, window, cx))))
            .on_action(cx.listener(|this, filter: &UsageFilter, window, cx| this.open_insights(Some(filter.clone()), window, cx)))
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
            .when(self.sidebar_drag.is_some(), |d| {
                d.cursor_col_resize()
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                        if event.pressed_button != Some(MouseButton::Left) {
                            this.end_sidebar_resize(cx);
                        } else {
                            let bots = this.can_show_bot_strip();
                            let viewport = this.sidebar_viewport(window.viewport_size().width, cx);
                            let Some(drag) = &mut this.sidebar_drag else { return };
                            let width = drag.initial + event.position.x - drag.from;
                            drag.width = sidebar::fit_width(width, viewport, bots);
                            cx.notify();
                        }
                    }))
                    .capture_any_mouse_up(cx.listener(|this, event: &MouseUpEvent, _, cx| {
                        if event.button == MouseButton::Left {
                            this.end_sidebar_resize(cx);
                        }
                    }))
            })
            .on_action(cx.listener(|_, _: &ToggleAppearance, _, cx| {
                let next = cx.theme().appearance.toggled();
                crate::yours::set_theme(next, cx);
            }))
            .flex()
            .flex_col()
            .relative()
            .size_full()
            .bg(c.background)
            .text_ui(TypeSize::Small)
            .text_color(c.text)
            .child(title_bar::render(self, width.map(|width| rail::WIDTH + CARD_EDGE + width), compact, cx))
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
                        .relative()
                        .children(sidebar)
                        .child(div().debug_selector(|| "workspace-content".into()).relative().flex_1().min_w_0().h_full().child(content))
                        .children(panel)
                        .children(sheet)
                        .child(card_corner(c.background)),
                ),
            )
            .children(self.render_quick_search(cx))
            .child(self.moves.clone())
            .child(self.image_viewer.clone())
    }
}

/// A project's list as "Show more" opened it: the cursor it's asked with, the box's page for it
/// once read, whether it's on its way (and asked again meanwhile), and why it couldn't be read.
#[derive(Default)]
pub struct ProjectPage {
    cursor: String,
    page: Option<kuvo_box::sessions::Page>,
    asking: bool,
    again: bool,
    failed: Option<String>,
    _task: Option<gpui::Task<()>>,
}
