//! The sidebars of the areas past Sessions, beside the rail: Bots (its bots, with what each is
//! doing now, and group chats), the Work Queue (its statuses and labels, with the box's
//! counts), Computers (yours, as the box lists them) and Settings (its sections). Each is its own
//! view, kept while another area shows and drawn only while its own does; Bots and the Work Queue
//! ask the box as they open, and the window hands Computers the list its title bar draws too.
//!
//! Bots' main view is its own (`crate::bots`): a bot's page, New Bot; and a group chat's
//! (`crate::groups`): its conversation, its editor, New Group. So is the Work Queue's
//! (`crate::queue`): the queue and an item's page; and Computers' (`crate::computers`).

use gpui::{
    AnyElement, App, AppContext as _, Context, EventEmitter, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, Task, Window, div, prelude::FluentBuilder as _, px, rgb,
};
use kuvo_box::BoxClient;
use kuvo_box::moves::{Kind as MoveKind, Target as MoveTarget};
use kuvo_box::areas::{Bot, BotNow, Computer, Group, Look};
use kuvo_ui::bot::State;
use kuvo_box::work_queue::QueueSaid;
use kuvo_box::bots::{BotsSaid, Elsewhere, Held, Refused};
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, Icon, IconSize, StyledType, TextRole, TypeSize,
    row::{NavRow, SectionHeader},
    menu::{ContextMenuArea, Menu, MenuEntry, PopoverMenu, Tooltip},
    small::{spinner, status_dot, status_words},
    theme::{radius, space},
};

use crate::area::Area;
use crate::bot_face::SidebarMark;
use crate::bots::{BotsEvent, BotsView, There};
use crate::computers::{ComputersEvent, ComputersView};
use crate::groups::{GroupsEvent, GroupsView};
use crate::moves::RemoteSource;
use crate::queue::marks::{label_dot, status_mark};
use crate::queue::{Act as QueueAct, QueueEvent, QueueView};
use crate::settings::{AddMachine, Opened, Pick, SettingsView};
use crate::time;
use kuvo_box::follow::Followed;


/// How an area's list stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Reading<T> {
    NotYet,
    Ready(T),
    Failed(String),
}

/// The latest answer from a source, independently of the data retained for display.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Reachability {
    #[default]
    Unanswered,
    Reached,
    Unreachable,
}

impl Reachability {
    pub(crate) fn of<T>(answer: &kuvo_box::Result<T>) -> Self {
        match answer {
            Ok(_) => Self::Reached,
            Err(_) => Self::Unreachable,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Bots {
    pub pending_computers: Vec<kuvo_box::areas::SetupComputer>,
    pub bots: Vec<Bot>,
    /// The bots that live on your other computers, as each last told the box (`kuvo/bots`).
    pub elsewhere: Vec<Elsewhere>,
    /// What the box holds for your other computers, and what they refused (`kuvo/bots`).
    pub held: Vec<Held>,
    pub refused: Vec<Refused>,
    /// The box couldn't say what it holds just now: what it last said is kept.
    pub held_unsaid: bool,
    pub groups: Vec<Group>,
    /// Why the group chats couldn't be read just now (those last read are kept meanwhile).
    pub groups_failed: Option<String>,
}

impl Bots {
    /// The bots that live on your other computers, as the box lists them, in its order.
    pub fn there(&self) -> Vec<There> {
        self.elsewhere.iter().map(There::of).collect()
    }
}

/// A row was picked: the window goes to its page.
pub struct Picked;

/// Somewhere the window is to go, asked from an area's page.
#[derive(Clone, Debug, PartialEq)]
pub enum Go {
    PairAgain,
    SearchBotsAndSessions,
    /// A bot's own conversation, with `draft` in its composer where the box gave one.
    BotChat { bot: String, session: String, draft: Option<String> },
    /// The selected bot now appears under its new computer.
    BotRehomed,
    /// One of the box's sessions.
    Session(String),
    /// The Sessions area.
    Sessions,
    /// Your computers read again (something was done to them), where they aren't followed.
    ReadComputers,
    /// The Computers area's Add a Machine page.
    AddMachine,
    /// A row in Settings opened this address (it's open already).
    Opened(String),
    /// The box that lists your computers did something: its list read again.
    ReadList,
    /// Update Now for this Mac's own Kuvo (`now`, else once its work ends).
    /// Add a Machine on the box that lists your computers, at this address.
    AddAt(Option<String>),
    /// This computer's last setup was wiped, and its Kuvo can be set up again.
    Reinstall,
    /// Marketplace › Agents (from New Bot).
    Marketplace,
    /// A place in Settings or in a computer's settings (`yours/…`, `box/…`) picked from the other.
    Setting(String),
}

#[derive(PartialEq, Eq)]
pub(crate) enum SidebarMode {
    Full,
    Avatars,
    SessionAvatars(Option<String>),
}

/// One area's sidebar.
pub struct AreaView {
    /// Computers' main view, in the Computers area.
    computers_page: Option<gpui::Entity<ComputersView>>,
    _computers_page: Vec<gpui::Subscription>,
    area: Area,
    sidebar_mode: SidebarMode,
    client: BoxClient,
    moves: Option<gpui::Entity<crate::moves::MovesView>>,
    live: bool,
    bots: Reading<Bots>,
    queue: Reading<QueueSaid>,
    /// Your computers, as the box lists them: the window's, handed here as it reads them.
    computers: Reading<Vec<Computer>>,
    /// The box this window shows, as the Computers list names it first.
    this_box: SharedString,
    /// What it is, as its `kuvo/info` said (`mac`, `machine`); none until it's read.
    this_kind: String,
    selected: Option<String>,
    now: Option<f64>,
    reading: Option<Task<()>>,
    /// Settings' own view: its sidebar drawn here, its pages as the main view.
    settings: Option<gpui::Entity<SettingsView>>,
    _settings_picks: Option<gpui::Subscription>,
    /// A computer's settings, under Computers › that computer (`box/<key>/<section>` picked).
    computer_settings: Option<gpui::Entity<SettingsView>>,
    _computer_settings: Vec<gpui::Subscription>,
    _settings_adds: Option<gpui::Subscription>,
    _settings_opens: Option<gpui::Subscription>,
    /// Settings' sidebar is drawn here: redrawn as Settings changes (a search, a reading, a sheet).
    _settings_changes: Option<gpui::Subscription>,
    /// Bots' main view: a bot's page and New Bot.
    page: Option<gpui::Entity<BotsView>>,
    _page: Vec<gpui::Subscription>,
    /// The group chats' main view, in Bots: a chat, its editor, New Group.
    groups: Option<gpui::Entity<GroupsView>>,
    _groups: Vec<gpui::Subscription>,
    /// The Work Queue's main view: the queue and an item's page.
    queue_page: Option<gpui::Entity<QueueView>>,
    _queue_page: Vec<gpui::Subscription>,
    /// The group chats followed, in Bots (the app): the list as it changes.
    _groups_follow: Option<(kuvo_box::Follow<kuvo_box::follow::Followed<kuvo_box::groups::Groups>>, Task<()>)>,
    /// The bots followed, in Bots (the app): what each is doing, as it changes.
    _bots_follow: Option<(kuvo_box::Follow<kuvo_box::follow::Followed<BotsSaid>>, Task<()>)>,
    reachability: Reachability,
    /// Whether this workspace is the one the window currently shows: background computers keep
    /// their workspaces alive, but only the shown workspace writes to global `bot_face` states.
    shown: bool,
    /// The bot whose conversation the window shows, and what the box was last told it saw there.
    chat_open: Option<String>,
    seen_told: Option<(String, Option<u64>)>,
    followed_meanwhile: Option<FollowedDuringRead>,
}

struct FollowedDuringRead {
    said: BotsSaid,
    /// A line from before a replacement read is useful if that read fails.
    newer_than_read: bool,
}



pub struct QueueDelivered(pub Option<QueueSaid>);
impl EventEmitter<QueueDelivered> for AreaView {}
impl EventEmitter<Picked> for AreaView {}
impl EventEmitter<Go> for AreaView {}

impl AreaView {
    pub fn new(area: Area, client: BoxClient, live: bool, cx: &mut Context<Self>) -> AreaView {
        let settings = (area == Area::Settings).then(|| cx.new(|cx| SettingsView::new(client.clone(), live, cx)));
        // Settings opens on your own, where its view starts.
        let selected = settings.as_ref().map(|settings| settings.read(cx).place());
        // A section picked in Settings' own sidebar is picked here, as any area's row is.
        let picks = settings.as_ref().map(|settings| {
            cx.subscribe(settings, |this: &mut AreaView, _, pick: &Pick, cx| {
                // (A computer's settings are under Computers.)
                if pick.place.starts_with("box/") {
                    cx.emit(Go::Setting(pick.place.clone()));
                } else if pick.record {
                    this.pick(pick.place.clone(), cx);
                } else {
                    this.select(Some(pick.place.clone()), cx);
                }
            })
        });
        let adds = settings.as_ref().map(|settings| cx.subscribe(settings, |_: &mut AreaView, _, _: &AddMachine, cx| cx.emit(Go::AddMachine)));
        let opens = settings.as_ref().map(|settings| cx.subscribe(settings, |_: &mut AreaView, _, opened: &Opened, cx| cx.emit(Go::Opened(opened.address.clone()))));
        let changes = settings.as_ref().map(|settings| cx.observe(settings, |_, _, cx| cx.notify()));
        let page = (area == Area::Bots).then(|| cx.new(|cx| BotsView::new(client.clone(), live, cx)));
        let page_subscriptions = page
            .as_ref()
            .map(|page| {
                vec![
                    cx.subscribe(page, |this: &mut AreaView, _, event: &BotsEvent, cx| match event {
                        BotsEvent::Chat { bot, session, draft } => cx.emit(Go::BotChat { bot: bot.clone(), session: session.clone(), draft: draft.clone() }),
                        BotsEvent::Session(session) => cx.emit(Go::Session(session.clone())),
                        BotsEvent::Select(Some(key)) => this.pick(key.clone(), cx),
                        BotsEvent::Select(None) => {
                            this.select(None, cx);
                            cx.emit(Picked);
                        }
                        // The list as the box says it now (what a change answered shows meanwhile).
                        BotsEvent::Changed => this.shown(cx),
                        BotsEvent::Marketplace => cx.emit(Go::Marketplace),
                    }),
                    // Its sheet dims the sidebar too: redrawn as it opens and closes.
                    cx.observe(page, |_, _, cx| cx.notify()),
                ]
            })
            .unwrap_or_default();
        let groups = (area == Area::Bots).then(|| cx.new(|cx| GroupsView::new(client.clone(), live, cx)));
        let groups_subscriptions = groups
            .as_ref()
            .map(|groups| {
                vec![
                    cx.subscribe(groups, |this: &mut AreaView, _, event: &GroupsEvent, cx| match event {
                        GroupsEvent::BotChat { bot, session } => cx.emit(Go::BotChat { bot: bot.clone(), session: session.clone(), draft: None }),
                        GroupsEvent::Select(Some(key)) => this.pick(key.clone(), cx),
                        GroupsEvent::Select(None) => {
                            this.select(None, cx);
                            cx.emit(Picked);
                        }
                        // The chats as the box lists them now (a change's answer shows meanwhile).
                        GroupsEvent::Changed => this.read_list(cx),
                    }),
                    cx.observe(groups, |_, _, cx| cx.notify()),
                ]
            })
            .unwrap_or_default();
        let queue_page = (area == Area::Queue).then(|| cx.new(|cx| QueueView::new(client.clone(), live, cx)));
        let queue_subscriptions = queue_page
            .as_ref()
            .map(|page| {
                vec![
                    cx.subscribe(page, |this: &mut AreaView, _, event: &QueueEvent, cx| match event {
                        QueueEvent::Session(session) => cx.emit(Go::Session(session.clone())),
                        QueueEvent::Select(Some(key)) => this.pick(key.clone(), cx),
                        QueueEvent::Select(None) => {
                            this.select(None, cx);
                            cx.emit(Picked);
                        }
                        // The view as the box says it now (what a change answered shows meanwhile).
                        QueueEvent::Changed => {
                            // What the box answered is what's known now: a read that fails after
                            // it keeps that, not what was read before the change.
                            if let Some(page) = &this.queue_page {
                                this.queue = page.read(cx).said().clone();
                            }
                            this.read_list(cx);
                        }
                    }),
                    cx.observe(page, |_, _, cx| cx.notify()),
                ]
            })
            .unwrap_or_default();
        let computer_settings = (area == Area::Computers).then(|| cx.new(|cx| SettingsView::for_computers(client.clone(), live, cx)));
        let computer_settings_subscriptions = computer_settings
            .as_ref()
            .map(|settings| {
                vec![
                    cx.subscribe(settings, |this: &mut AreaView, _, pick: &Pick, cx| {
                        // (Your own settings are Settings'.)
                        if pick.place.starts_with("yours/") {
                            cx.emit(Go::Setting(pick.place.clone()));
                        } else if pick.record {
                            this.pick(pick.place.clone(), cx);
                        } else {
                            this.select(Some(pick.place.clone()), cx);
                        }
                    }),
                    cx.subscribe(settings, |_: &mut AreaView, _, _: &AddMachine, cx| cx.emit(Go::AddMachine)),
                    cx.subscribe(settings, |_: &mut AreaView, _, opened: &Opened, cx| cx.emit(Go::Opened(opened.address.clone()))),
                    cx.observe(settings, |_, _, cx| cx.notify()),
                ]
            })
            .unwrap_or_default();
        let this_mac = client.here();
        let computers_page = (area == Area::Computers).then(|| cx.new(|cx| ComputersView::new(client.clone(), live, this_mac, cx)));
        let computers_subscriptions = computers_page
            .as_ref()
            .map(|page| {
                vec![cx.subscribe(page, |this: &mut AreaView, _, event: &ComputersEvent, cx| match event {
                    ComputersEvent::Select(Some(key)) => this.pick(key.clone(), cx),
                    ComputersEvent::Select(None) => {
                        this.select(None, cx);
                        cx.emit(Picked);
                    }
                    ComputersEvent::Sessions => cx.emit(Go::Sessions),
                    ComputersEvent::PairAgain => cx.emit(Go::PairAgain),
                    ComputersEvent::ReadAgain => cx.emit(Go::ReadComputers),
                    ComputersEvent::ListerChanged => cx.emit(Go::ReadList),
                    ComputersEvent::AddAt(address) => cx.emit(Go::AddAt(address.clone())),
                    ComputersEvent::Reinstall => cx.emit(Go::Reinstall),
                })]
            })
            .unwrap_or_default();
        let groups_follow = (area == Area::Bots && live).then(|| {
            let follow = client.follow_groups();
            let updates = follow.updates().clone();
            let task = cx.spawn(async move |this, cx| {
                while let Ok(update) = updates.recv().await {
                    if let kuvo_box::follow::Followed::Now(list) = update
                        && this.update(cx, |this, cx| this.take_groups(list.groups, cx)).is_err()
                    {
                        break;
                    }
                }
            });
            (follow, task)
        });
        let bots_follow = (area == Area::Bots && live).then(|| {
            let follow = client.follow_bots();
            let updates = follow.updates().clone();
            let task = cx.spawn(async move |this, cx| {
                while let Ok(update) = updates.recv().await {
                    if this.update(cx, |this, cx| this.take_followed_bots(update, cx)).is_err() { break; }
                }
            });
            (follow, task)
        });
        AreaView { moves: None, _bots_follow: bots_follow, reachability: Reachability::Unanswered, shown: true, chat_open: None, seen_told: None, followed_meanwhile: None, _groups_follow: groups_follow, groups, _groups: groups_subscriptions, computers_page, _computers_page: computers_subscriptions, queue_page, _queue_page: queue_subscriptions, page, _page: page_subscriptions, area, sidebar_mode: SidebarMode::Full, client, live, bots: Reading::NotYet, queue: Reading::NotYet, computers: Reading::NotYet, this_box: "This Mac".into(), this_kind: String::new(), selected, now: None, reading: None, settings, _settings_picks: picks, _settings_adds: adds, _settings_opens: opens, _settings_changes: changes, computer_settings, _computer_settings: computer_settings_subscriptions }
    }

    /// Settings' view, in the Settings area; a computer's settings, in Computers while they're
    /// picked (`box/<key>/<section>`).
    pub fn settings(&self) -> Option<&gpui::Entity<SettingsView>> {
        if self.selected.as_deref().is_some_and(|key| key.starts_with("box/")) && self.computer_settings.is_some() {
            return self.computer_settings.as_ref();
        }
        self.settings.as_ref()
    }

    /// A computer's settings, under Computers, whether or not they're picked now.
    pub fn computer_settings(&self) -> Option<&gpui::Entity<SettingsView>> {
        self.computer_settings.as_ref()
    }

    /// Bots' main view, in the Bots area.
    pub fn page(&self) -> Option<&gpui::Entity<BotsView>> {
        self.page.as_ref()
    }

    /// Runs on, as the nearest box says it: New Bot offers it.
    pub fn set_runs_on(&mut self, computers: Vec<kuvo_box::models::RunsOnComputer>, now: f64, cx: &mut Context<Self>) {
        if let Some(page) = &self.page {
            page.update(cx, |page, cx| page.set_runs_on(computers, now, cx));
        }
    }

    pub fn set_moves(&mut self, moves: gpui::Entity<crate::moves::MovesView>, cx: &mut Context<Self>) {
        self._page.push(cx.observe(&moves, |_, _, cx| cx.notify()));
        if let Some(page) = &self.page {
            page.update(cx, |page, cx| page.set_moves(moves.clone(), cx));
        }
        if let Some(groups) = &self.groups {
            groups.update(cx, |groups, cx| groups.set_moves(moves.clone(), cx));
        }
        self.moves = Some(moves);
    }

    fn move_menu(&self, key: &str, row: impl IntoElement, target: MoveTarget, source: Option<RemoteSource>) -> AnyElement {
        let Some(moves) = self.moves.clone() else { return row.into_any_element() };
        ContextMenuArea::new(SharedString::from(format!("move-menu-{key}")), row, move |window, cx| {
            crate::moves::MovesView::menu_from(&moves, target.clone(), source.clone(), window, cx)
        }).into_any_element()
    }

    /// The group chats' main view, in the Bots area.
    pub fn groups(&self) -> Option<&gpui::Entity<GroupsView>> {
        self.groups.as_ref()
    }

    /// The group chats as the box lists them now (followed): drawn in the sidebar and handed to
    /// their view. Before the bots are read, they wait for the list's next read.
    fn take_groups(&mut self, groups: Vec<Group>, cx: &mut Context<Self>) {
        let Reading::Ready(bots) = &mut self.bots else { return };
        if bots.groups == groups && bots.groups_failed.is_none() {
            return;
        }
        bots.groups = groups.clone();
        bots.groups_failed = None;
        let all = bots.bots.clone();
        if let Some(view) = &self.groups {
            view.update(cx, |view, cx| view.set_listed(groups, all, cx));
        }
        cx.notify();
    }

    fn take_followed_bots(&mut self, update: Followed<BotsSaid>, cx: &mut Context<Self>) {
        let bots = match update {
            Followed::Now(said) => {
                if self.reading.is_some() {
                    self.followed_meanwhile = Some(FollowedDuringRead { said, newer_than_read: true });
                    return;
                }
                let before = ready(&self.bots);
                let groups = before.map(|bots| bots.groups.clone()).unwrap_or_default();
                let groups_failed = before.and_then(|bots| bots.groups_failed.clone());
                let bots = Bots { pending_computers: said.pending_computers, bots: said.bots, elsewhere: said.elsewhere, held: said.held, refused: said.refused, held_unsaid: said.held_unsaid, groups, groups_failed };
                if self.has_fresh_bots() && matches!(&self.bots, Reading::Ready(before) if before == &bots) {
                    return;
                }
                Ok(bots)
            }
            // Let a pending list read settle. Either route can supply a current snapshot.
            Followed::Failed(_) if self.reading.is_some() => return,
            Followed::Failed(error) => Err(error),
        };
        self.take(Read::Bots(bots), cx);
    }

    /// Only the shown workspace writes to the window's shared bot faces.
    fn tell_faces(&mut self, cx: &mut Context<Self>) {
        if !self.shown {
            return;
        }
        let Reading::Ready(bots) = &self.bots else { return };
        for bot in &bots.bots {
            crate::bot_face::tell(&bot.id, self.bot_state(bot), cx);
        }
        for there in &bots.elsewhere {
            if !bots.bots.iter().any(|bot| bot.id == there.id) {
                crate::bot_face::tell(&there.id, self.elsewhere_state(there), cx);
            }
        }
        self.tell_seen(cx);
    }

    fn bot_state(&self, bot: &Bot) -> State {
        if self.has_fresh_bots() { crate::bot_face::state_of(&bot.now) } else { State::Offline }
    }

    fn elsewhere_state(&self, listed: &Elsewhere) -> State {
        if listed.away {
            State::Offline
        } else {
            listed.now.as_ref().map_or(State::Idle, crate::bot_face::state_of)
        }
    }

    /// While its computer is away, as the box says it with the row: when it was last heard from
    /// ("seen 2h"), drawn on its name; "Updating…" while it only switches to a new release (its row
    /// as it is, not away).
    fn away_words(&self, listed: &Elsewhere) -> Option<String> {
        if listed.updating && !listed.away {
            return Some("Updating…".into());
        }
        listed.away.then(|| listed.last_seen.map(|seen| format!("seen {}", crate::time::ago(seen, self.now()))).unwrap_or_default())
    }

    pub(crate) fn reachability(&self) -> Reachability {
        self.reachability
    }

    /// Whether an active turn is backed by a current bot snapshot.
    pub(crate) fn has_fresh_bots(&self) -> bool {
        self.reachability == Reachability::Reached
    }

    /// Whether this workspace is the one the window currently shows: becoming shown immediately
    /// restores its bots' live states in `bot_face`.
    pub fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        if self.shown != shown {
            self.shown = shown;
            if shown {
                self.tell_faces(cx);
                cx.notify();
            }
        }
    }

    /// The bot whose conversation the window shows now, if any: what it finished or stopped on is
    /// seen, and the box is told so (`POST kuvo/bots/<id>/seen`), for every client.
    pub fn set_chat_open(&mut self, bot: Option<String>, cx: &mut Context<Self>) {
        if self.chat_open != bot {
            self.chat_open = bot;
            // (Opened again later, it's told again.)
            self.seen_told = None;
            self.tell_seen(cx);
        }
    }

    /// The box told what you've seen of the bot whose conversation shows: as it opens, and again as
    /// it finishes or stops while you watch. What that clears is the box's to say. One that
    /// couldn't be told is told again at the next chance.
    fn tell_seen(&mut self, cx: &mut Context<Self>) {
        let (Some(open), Reading::Ready(bots)) = (&self.chat_open, &self.bots) else { return };
        let Some(bot) = bots.bots.iter().find(|bot| &bot.id == open) else { return };
        let told = (bot.id.clone(), bot.now.since.map(|since| since as u64));
        let news = matches!(bot.now.state, kuvo_box::areas::BotState::Done | kuvo_box::areas::BotState::Blocked);
        match &self.seen_told {
            Some(before) if *before == told || (before.0 == told.0 && !news) => return,
            _ => {}
        }
        self.seen_told = Some(told.clone());
        let (client, id) = (self.client.clone(), bot.id.clone());
        if !self.live {
            if client.bot_seen(&id).is_err() {
                self.seen_told = None;
            }
            return;
        }
        cx.spawn(async move |this, cx| {
            let answer = cx.background_executor().spawn(async move { client.bot_seen(&id) }).await;
            if answer.is_err() {
                this.update(cx, |this, _| {
                    if this.seen_told.as_ref() == Some(&told) {
                        this.seen_told = None;
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    /// Whether Bots' main view is on screen (Bots shown, no bot's conversation over it): a group
    /// chat is followed and marked read only then.
    pub fn set_on_screen(&mut self, on: bool, cx: &mut Context<Self>) {
        if let Some(groups) = &self.groups {
            groups.update(cx, |groups, cx| groups.set_on_screen(on, cx));
        }
    }

    /// The Work Queue's main view, in the Work Queue area.
    pub fn queue_page(&self) -> Option<&gpui::Entity<QueueView>> {
        self.queue_page.as_ref()
    }

    /// Computers' main view, in the Computers area.
    pub fn computers_page(&self) -> Option<&gpui::Entity<ComputersView>> {
        self.computers_page.as_ref()
    }

    /// The area's own main view, for what's picked: a bot's page or New Bot, a group chat, its
    /// editor or New Group, the Work Queue or one of its items, your computers or one of them.
    pub fn main(&self) -> Option<gpui::AnyView> {
        if let Some(page) = &self.computers_page {
            return Some(page.clone().into());
        }
        if let Some(page) = &self.queue_page {
            return Some(page.clone().into());
        }
        if self.selected.as_deref().is_some_and(is_group_key)
            && let Some(groups) = &self.groups
        {
            return Some(groups.clone().into());
        }
        self.page.as_ref().map(|page| page.clone().into())
    }

    pub fn area(&self) -> Area {
        self.area
    }

    pub fn set_now(&mut self, now: f64, cx: &mut Context<Self>) {
        self.now = Some(now);
        for settings in self.settings.iter().chain(&self.computer_settings) {
            settings.update(cx, |settings, _| settings.set_now(now));
        }
        if let Some(page) = &self.page {
            page.update(cx, |page, _| page.set_now(now));
        }
        if let Some(groups) = &self.groups {
            groups.update(cx, |groups, cx| groups.set_now(now, cx));
        }
        if let Some(page) = &self.queue_page {
            page.update(cx, |page, _| page.set_now(now));
        }
        if let Some(page) = &self.computers_page {
            page.update(cx, |page, cx| page.set_now(now, cx));
        }
    }

    pub fn set_this_box(&mut self, name: SharedString, kind: &str, cx: &mut Context<Self>) {
        if let Some(page) = &self.computers_page {
            page.update(cx, |page, cx| page.set_this_box(&name, kind, cx));
        }
        if self.this_kind != kind {
            self.this_kind = kind.to_owned();
            cx.notify();
        }
        if self.this_box != name {
            for settings in self.settings.iter().chain(&self.computer_settings) {
                settings.update(cx, |settings, cx| settings.set_this_box(&name, cx));
            }
            if let Some(page) = &self.page {
                page.update(cx, |page, cx| page.set_this_box(&name, cx));
            }
            if let Some(groups) = &self.groups {
                groups.update(cx, |groups, cx| groups.set_this_box(&name, cx));
            }
            if let Some(page) = &self.queue_page {
                page.update(cx, |page, cx| page.set_this_box(&name, cx));
            }
            self.this_box = name;
            cx.notify();
        }
    }

    /// Your computers, as the window read them.
    pub fn set_computers(&mut self, computers: Reading<Vec<Computer>>, cx: &mut Context<Self>) {
        if self.computers != computers {
            if let Some(page) = &self.computers_page {
                let computers = computers.clone();
                page.update(cx, |page, cx| page.set_computers(computers, cx));
            }
            for settings in self.settings.iter().chain(&self.computer_settings) {
                let computers = computers.clone();
                settings.update(cx, |settings, cx| settings.set_computers(computers, cx));
            }
            self.computers = computers;
            self.tell_faces(cx);
            cx.notify();
        }
    }

    /// It's shown: its list is read from the box (in the app, on another thread; in tests and
    /// shots, at once). What was read before stays on screen meanwhile.
    pub fn shown(&mut self, cx: &mut Context<Self>) {
        if let Some(settings) = self.settings().cloned() {
            settings.update(cx, |settings, cx| settings.shown(cx));
            if self.area == Area::Settings {
                return;
            }
        }
        // The page open reads what it's drawn from again, as the area comes on screen.
        if let Some(page) = &self.page {
            page.update(cx, |page, cx| page.read(cx));
        }
        if let Some(groups) = &self.groups {
            groups.update(cx, |groups, cx| groups.read(cx));
        }
        if let Some(page) = &self.queue_page {
            page.update(cx, |page, cx| page.read(cx));
        }
        self.read_list(cx);
    }

    pub fn refresh_after_move(&mut self, cx: &mut Context<Self>) {
        if self.reading.is_none() {
            self.read_list(cx);
        }
    }

    /// Reads the area's list from the box, what's on screen staying meanwhile.
    pub(crate) fn read_list(&mut self, cx: &mut Context<Self>) {
        if let Some(update) = &mut self.followed_meanwhile {
            update.newer_than_read = false;
        }
        let (area, client) = (self.area, self.client.clone());
        if !self.live {
            let read = read(area, &client);
            self.take(read, cx);
            return;
        }
        self.reading = Some(cx.spawn(async move |this, cx| {
            let read = cx.background_executor().spawn(async move { read(area, &client) }).await;
            this.update(cx, |this, cx| { this.reading = None; this.take(read, cx); }).ok();
        }));
    }

    fn take(&mut self, read: Read, cx: &mut Context<Self>) {
        match read {
            Read::Bots(bots) => {
                self.reachability = Reachability::of(&bots);
                let followed = self.followed_meanwhile.take()
                    .filter(|update| update.newer_than_read || bots.is_err());
                // What the box couldn't say it holds just now stays as it last said it.
                let bots = bots.map(|mut bots| {
                    // Chats that couldn't be read just now stay as they were last read.
                    if bots.groups_failed.is_some()
                        && let Reading::Ready(before) = &self.bots
                    {
                        bots.groups = before.groups.clone();
                    }
                    if bots.held_unsaid
                        && let Reading::Ready(before) = &self.bots
                    {
                        bots.held = before.held.clone();
                        bots.refused = before.refused.clone();
                        bots.elsewhere = before.elsewhere.clone();
                    }
                    bots
                });
                self.bots = reading(bots, &self.bots);
                // A line followed while this was read is newer than it.
                if let Some(FollowedDuringRead { said, .. }) = followed {
                    self.reachability = Reachability::Reached;
                    let before = ready(&self.bots);
                    let kept = before.map(|bots| BotsSaid { pending_computers: bots.pending_computers.clone(), bots: Vec::new(), elsewhere: bots.elsewhere.clone(), held: bots.held.clone(), refused: bots.refused.clone(), held_unsaid: false });
                    let said = said.keeping(kept.as_ref());
                    self.bots = Reading::Ready(Bots {
                        pending_computers: said.pending_computers,
                        bots: said.bots, elsewhere: said.elsewhere, held: said.held, refused: said.refused,
                        held_unsaid: said.held_unsaid,
                        groups: before.map(|bots| bots.groups.clone()).unwrap_or_default(),
                        groups_failed: before.and_then(|bots| bots.groups_failed.clone()),
                    });
                }
                let rehomed = match (self.selected.as_deref().and_then(|key| key.split_once(':')), &self.bots) {
                    (Some((kind, id)), Reading::Ready(bots)) => {
                        let here = bots.bots.iter().any(|bot| bot.id == id);
                        match kind {
                            "bot" if !here && bots.elsewhere.iter().any(|bot| bot.id == id && !bot.unlisted) => Some(format!("elsewhere:{id}")),
                            "elsewhere" if here => Some(format!("bot:{id}")),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                self.tell_faces(cx);
                // The same reading, handed to the page.
                let said = match &self.bots {
                    Reading::NotYet => Reading::NotYet,
                    Reading::Failed(why) => Reading::Failed(why.clone()),
                    Reading::Ready(bots) => Reading::Ready(BotsSaid { pending_computers: bots.pending_computers.clone(), bots: bots.bots.clone(), elsewhere: bots.elsewhere.clone(), held: bots.held.clone(), refused: bots.refused.clone(), held_unsaid: bots.held_unsaid }),
                };
                if let Some(page) = &self.page {
                    page.update(cx, |page, cx| page.set_said(said, cx));
                }
                // (Chats that couldn't be read aren't said to be gone: what was there stays.)
                if let (Some(groups), Reading::Ready(bots)) = (&self.groups, &self.bots)
                    && bots.groups_failed.is_none()
                {
                    let (chats, all) = (bots.groups.clone(), bots.bots.clone());
                    groups.update(cx, |groups, cx| groups.set_listed(chats, all, cx));
                }
                if let Some(key) = rehomed {
                    self.select(Some(key), cx);
                    cx.emit(Go::BotRehomed);
                }
            }
            Read::Queue(queue) => {
                self.reachability = Reachability::of(&queue);
                cx.emit(QueueDelivered(queue.as_ref().as_ref().ok().cloned()));
                self.queue = reading(*queue, &self.queue);
                // The same reading, handed to the page.
                if let Some(page) = &self.queue_page {
                    let said = self.queue.clone();
                    page.update(cx, |page, cx| page.set_said(said, cx));
                }
            }
            Read::Nothing => {}
        }
        cx.notify();
    }

    pub fn bots(&self) -> &Reading<Bots> {
        &self.bots
    }

    pub(crate) fn still_bot_avatar(&self, id: &str, size: Pixels) -> AnyElement {
        if let Some(bots) = ready(&self.bots) {
            if let Some(bot) = bots.bots.iter().find(|bot| bot.id == id) {
                return crate::bot_face::still_avatar(id, &bot.look, self.bot_state(bot), size);
            }
            if let Some(bot) = bots.elsewhere.iter().find(|bot| bot.id == id) {
                let state = self.elsewhere_state(bot);
                let look = bot.look.clone().unwrap_or_else(unknown_bot_look);
                return crate::bot_face::still_avatar(id, &look, state, size);
            }
        }
        crate::bot_face::still_avatar(id, &unknown_bot_look(), State::Offline, size)
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// Whether the row `key` names is still in the area's list, as the box last said it (none:
    /// not read yet). One the list can't say of (a filter, this computer, a list that couldn't
    /// be read) counts as there.
    pub fn still_there(&self, key: &str) -> Option<bool> {
        let (kind, id) = key.split_once(':').unwrap_or((key, ""));
        match (self.area, kind) {
            (Area::Bots, "bot" | "elsewhere" | "group") => match &self.bots {
                Reading::NotYet => None,
                Reading::Failed(_) => Some(true),
                Reading::Ready(bots) => Some(match kind {
                    "bot" => bots.bots.iter().any(|bot| bot.id == id),
                    "elsewhere" => bots.elsewhere.iter().any(|bot| bot.id == id),
                    _ => bots.groups.iter().any(|group| group.id == id),
                }),
            },
            (Area::Queue, "item" | "status" | "label") => match &self.queue {
                Reading::NotYet => None,
                Reading::Failed(_) => Some(true),
                Reading::Ready(said) => Some(match kind {
                    "item" => said.items.iter().chain(&said.removed).any(|item| item.id == id),
                    "status" => said.statuses.iter().any(|status| status.id == id),
                    _ => said.labels.iter().any(|label| label.id == id),
                }),
            },
            (Area::Computers, "computer") => match &self.computers {
                Reading::NotYet => None,
                Reading::Failed(_) => Some(true),
                Reading::Ready(computers) => Some(computers.iter().any(|computer| computer.key == id)),
            },
            _ => Some(true),
        }
    }

    /// Picks a row (or none: the area's own page), as going back or forward does.
    pub fn select(&mut self, key: Option<String>, cx: &mut Context<Self>) {
        self.selected = key;
        self.show_section(cx);
        cx.notify();
    }

    pub(crate) fn pick(&mut self, key: String, cx: &mut Context<Self>) {
        self.selected = Some(key);
        self.show_section(cx);
        cx.emit(Picked);
        cx.notify();
    }

    /// Settings shows the section picked (its first, Clock, for none); Bots, what's picked.
    fn show_section(&mut self, cx: &mut Context<Self>) {
        if let (Some(settings), Some(place)) = (&self.settings, self.selected.clone()) {
            if place.starts_with("yours/") {
                settings.update(cx, |settings, cx| settings.set_place(&place, cx));
            } else {
                // (A computer's, under Computers › that computer: Settings stays on your own.)
                let place = if place.starts_with("box/") { place } else { crate::settings::box_key("", &place.to_lowercase().replace(' ', "-")) };
                cx.emit(Go::Setting(place));
            }
            // (A section named alone, "Accounts", is kept as the place it came to.)
            self.selected = Some(settings.read(cx).place());
        }
        // A computer's settings, under Computers.
        if let (Some(settings), Some(place)) = (&self.computer_settings, self.selected.clone().filter(|key| key.starts_with("box/"))) {
            settings.update(cx, |settings, cx| settings.set_place(&place, cx));
            self.selected = Some(settings.read(cx).place());
            return;
        }
        if let Some(page) = &self.page {
            let key = self.selected.clone().filter(|key| !is_group_key(key));
            page.update(cx, |page, cx| page.show(key.as_deref(), cx));
        }
        if let Some(groups) = &self.groups {
            let key = self.selected.clone();
            groups.update(cx, |groups, cx| groups.show(key.as_deref(), cx));
        }
        if let Some(page) = &self.queue_page {
            let key = self.selected.clone();
            page.update(cx, |page, cx| page.show(key.as_deref(), cx));
        }
        if let Some(page) = &self.computers_page {
            let key = self.selected.clone();
            page.update(cx, |page, cx| page.show(key.as_deref(), cx));
        }
    }

    /// The page's title: what's picked, named as its row is.
    pub fn title(&self, cx: &App) -> SharedString {
        if let Some(settings) = self.settings() {
            return settings.read(cx).title();
        }
        let Some(key) = self.selected.as_deref() else { return self.area.name().into() };
        let (kind, id) = key.split_once(':').unwrap_or((key, ""));
        let named = match kind {
            "bot" => ready(&self.bots).and_then(|bots| bots.bots.iter().find(|bot| bot.id == id).map(|bot| bot.name.clone())),
            "elsewhere" => ready(&self.bots).and_then(|bots| bots.there().into_iter().find(|there| there.id == id).map(|there| there.name)),
            _ if key == "new" => Some("New Bot".into()),
            _ if key == "new-group" => Some("New Group".into()),
            "edit-group" => ready(&self.bots).and_then(|bots| bots.groups.iter().find(|group| group.id == id).map(|group| format!("Edit {}", group.name))),
            _ if key == "add" => Some("Add a Machine".into()),
            "group" => ready(&self.bots).and_then(|bots| bots.groups.iter().find(|group| group.id == id).map(|group| group.name.clone())),
            "status" => ready(&self.queue).and_then(|queue| queue.statuses.iter().find(|status| status.id == id).map(|status| status.name.clone())),
            "label" => ready(&self.queue).and_then(|queue| queue.labels.iter().find(|label| label.id == id).map(|label| label.name.clone())),
            "computer" => ready(&self.computers).and_then(|computers| computers.iter().find(|computer| computer.key == id).map(|computer| computer.name.clone())),
            "item" => ready(&self.queue).and_then(|queue| queue.items.iter().chain(&queue.removed).find(|item| item.id == id).map(|item| item.title.clone())),
            "this" => Some(self.this_box.to_string()),
            _ => Some(key.to_owned()),
        };
        named.unwrap_or_else(|| self.area.name().to_owned()).into()
    }

    fn now(&self) -> f64 {
        self.now.unwrap_or_else(time::now_ms)
    }
}

enum Read {
    Bots(kuvo_box::Result<Bots>),
    Queue(Box<kuvo_box::Result<QueueSaid>>),
    Nothing,
}

fn read(area: Area, client: &BoxClient) -> Read {
    match area {
        Area::Bots => Read::Bots(client.bots_said().map(|said| {
            // Group chats come from the same box; a box whose chats can't be read still lists its bots.
            let (groups, groups_failed) = match client.groups() {
                Ok(groups) => (groups, None),
                Err(error) => (Vec::new(), Some(kuvo_box::accounts::said(&error))),
            };
            Bots { pending_computers: said.pending_computers, bots: said.bots, elsewhere: said.elsewhere, held: said.held, refused: said.refused, held_unsaid: said.held_unsaid, groups, groups_failed }
        })),
        Area::Queue => Read::Queue(Box::new(client.work_queue_view())),
        _ => Read::Nothing,
    }
}

/// What a read makes of the list: what it read, or (failing) what was there, else why.
pub fn reading<T: Clone + Default>(read: kuvo_box::Result<T>, before: &Reading<T>) -> Reading<T> {
    match (read, before) {
        (Ok(value), _) => Reading::Ready(value),
        (Err(_), Reading::Ready(value)) => Reading::Ready(value.clone()),
        (Err(kuvo_box::Error::NeedsUpdating { .. }), _) => Reading::Ready(T::default()),
        (Err(error), _) => Reading::Failed(error.to_string()),
    }
}

/// Whether a sidebar key picks one of the group chats' pages.
pub fn is_group_key(key: &str) -> bool {
    key == "new-group" || key.starts_with("group:") || key.starts_with("edit-group:")
}

fn ready<T>(reading: &Reading<T>) -> Option<&T> {
    match reading {
        Reading::Ready(value) => Some(value),
        _ => None,
    }
}

impl Render for AreaView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(settings) = self.settings().cloned() {
            return div().id("area-settings").size_full().child(settings.update(cx, |settings, cx| settings.sidebar(cx)));
        }
        if self.client.gate().is_held() {
            return div().id("area-frame").debug_selector(|| "held-area-sidebar".into()).size_full();
        }
        let compact = self.sidebar_mode != SidebarMode::Full;
        let rows = match self.area {
            Area::Bots if compact => self.bot_avatars(cx),
            Area::Bots => self.bots_rows(cx),
            Area::Queue => self.queue_rows(cx),
            Area::Computers => self.computer_rows(cx),
            Area::Settings | Area::Insights | Area::Library | Area::Marketplace => Vec::new(),
            Area::Sessions => Vec::new(),
        };
        // A sheet open on the area's page: the sidebar waits under the same dimming.
        let dimmed = self.page.as_ref().is_some_and(|page| page.read(cx).sheet().is_some()) || self.queue_page.as_ref().is_some_and(|page| page.read(cx).sheet().is_some());
        div()
            .id("area-frame")
            .relative()
            .size_full()
            .child(
                div()
                    .id(SharedString::from(if compact { "bot-avatars".into() } else { format!("area-{}", self.area.slug()) }))
                    .size_full()
                    .overflow_y_scroll()
                    .px(if compact { space::S1 } else { space::S2 })
                    .pt(space::S2)
                    .pb(space::S4)
                    .when(compact, |d| d.flex().flex_col().items_center().gap(space::S1))
                    .children(rows),
            )
            .when(dimmed, |d| d.child(div().id("area-sidebar-dim").debug_selector(|| "area-sidebar-dim".into()).absolute().inset_0().bg(crate::bots::dim(cx)).occlude()))
    }
}

impl AreaView {
    pub(crate) fn set_sidebar_mode(&mut self, mode: SidebarMode, cx: &mut Context<Self>) {
        if self.sidebar_mode != mode {
            self.sidebar_mode = mode;
            cx.notify();
        }
    }

    fn row_click(&self, key: String, cx: &mut Context<Self>) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static {
        cx.listener(move |this, _, _, cx| this.pick(key.clone(), cx))
    }

    fn is_selected(&self, key: &str) -> bool {
        self.selected.as_deref() == Some(key)
    }

    fn bot_avatars(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = cx.theme().colors.clone();
        let search = Button::icon_only("bot-avatar-search", kuvo_ui::IconName::Search, "Search bots and sessions")
            .icon_size(IconSize::Large)
            .on_click(cx.listener(|_, _, _, cx| cx.emit(Go::SearchBotsAndSessions)));
        let mut rows = vec![search.into_any_element()];
        let Some(bots) = ready(&self.bots) else { return rows };
        let now = self.now();
        for bot in &bots.bots {
            let state = self.bot_state(bot);
            let mark = SidebarMark::of(bot, self.has_fresh_bots(), now);
            let tooltip = mark.as_ref().map_or_else(|| bot.name.clone(), |mark| mark.tooltip(&bot.name));
            let gutter = mark.as_ref().map(|mark| mark.gutter(&c));
            let face = crate::bot_face::still_avatar(&bot.id, &bot.look, state, COMPACT_FACE);
            rows.push(self.avatar_row(format!("bot:{}", bot.id), &bot.id, tooltip, face, gutter, None, cx));
        }
        for listed in &bots.elsewhere {
            let there = There::of(listed);
            let state = self.elsewhere_state(listed);
            let face = there.look.as_ref().map_or_else(
                || crate::bot_face::still_avatar(&there.id, &unknown_bot_look(), state, COMPACT_FACE),
                |look| crate::bot_face::still_avatar(&there.id, look, state, COMPACT_FACE),
            );
            let mark = self.elsewhere_mark(listed, &there, now);
            let tooltip = if there.waiting && state == State::Offline {
                format!("{} · Waiting for {}", there.name, there.computer)
            } else if there.refused && state == State::Offline {
                format!("{} · Change declined on {}", there.name, there.computer)
            } else {
                mark.as_ref().map_or_else(|| there.name.clone(), |mark| mark.tooltip(&there.name))
            };
            let gutter = mark.as_ref().map(|mark| mark.gutter(&c));
            rows.push(self.avatar_row(format!("elsewhere:{}", there.id), &there.id, tooltip, face, gutter, RemoteSource::of(listed), cx));
        }
        rows
    }

    fn avatar_row(
        &self,
        key: String,
        bot: &str,
        tooltip: String,
        face: AnyElement,
        gutter: Option<AnyElement>,
        source: Option<RemoteSource>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &cx.theme().colors;
        let selected = match &self.sidebar_mode {
            SidebarMode::SessionAvatars(owner) => owner.as_deref() == Some(bot),
            _ => self.is_selected(&key),
        };
        let fill = if selected { c.selected } else { c.hover };
        let ring = c.focus_ring;
        let selector = format!("bot-avatar-{key}");
        let face_selector = format!("bot-avatar-face-{key}");
        let selected_selector = format!("{selector}-selected");
        let target = MoveTarget::new(MoveKind::Bot, bot);
        let tooltip = self.moves.as_ref()
            .and_then(|moves| moves.read(cx).row_status_words(&target))
            .map_or(tooltip.clone(), |words| format!("{tooltip} · {words}"));
        let row = div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .flex()
            .flex_none()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(3.))
            .w(px(48.))
            .h(px(54.))
            .rounded(radius::CARD)
            .when(selected, |d| d.bg(fill))
            .hover(move |d| d.bg(fill))
            .focus_visible(move |d| d.shadow(kuvo_ui::button::focus_ring(ring)))
            .cursor_pointer()
            .tab_index(0)
            .tooltip(move |_, cx| Tooltip::new(tooltip.clone()).build(cx))
            .on_click(self.row_click(key.clone(), cx))
            .child(
                div()
                    .flex_none()
                    .when(selected, |d| d.debug_selector(move || selected_selector))
                    .child(div().flex_none().debug_selector(move || face_selector).child(face)),
            )
            .child(
                div()
                    .h(px(10.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .children(gutter),
            );
        if source.is_some() || key.starts_with("bot:") {
            self.move_menu(&key, row, target, source)
        } else {
            row.into_any_element()
        }
    }

    /// Status mark shared by full rows and the compact avatar strip for a bot on another computer.
    /// Offline bots always show the Offline hollow circle; pending/refused remote edits never reuse
    /// the "Waiting on you" blue dot.
    fn elsewhere_mark(&self, listed: &Elsewhere, there: &There, now: f64) -> Option<SidebarMark> {
        if self.elsewhere_state(listed) == State::Offline {
            return Some(SidebarMark::Offline);
        }
        if there.refused {
            return Some(SidebarMark::ChangeRefused(format!("Change declined on {}", there.computer)));
        }
        if let Some(activity) = &listed.now
            && let Some(mark) = SidebarMark::of_activity(activity, listed.paused, listed.wakes, now)
        {
            return Some(mark);
        }
        if there.waiting {
            return Some(SidebarMark::ChangeWaiting(format!("Waiting for {}", there.computer)));
        }
        None
    }

    fn bots_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = cx.theme().colors.clone();
        // The area's name, with New Bot beside it.
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(32.))
            .pl(space::S2)
            .pr(space::S1)
            .mb(space::S1)
            .child(div().text_role(TextRole::PaneTitle).text_color(c.text).child("Bots"))
            .child(Button::icon_only("new-bot", kuvo_ui::IconName::Plus, "New Bot").ghost().size(ButtonSize::Small).icon_size(kuvo_ui::IconSize::Medium).selected(self.is_selected("new")).tooltip("New Bot").disabled(self.client.gate().is_held()).on_click(self.row_click("new".into(), cx)))
            .into_any_element();
        let bots = match &self.bots {
            Reading::NotYet => return vec![header, quiet("Loading bots…", true, cx)],
            Reading::Failed(why) => return vec![header, quiet(&format!("Bots couldn’t be read: {why}"), false, cx)],
            Reading::Ready(bots) => bots,
        };
        let mut rows = vec![header];
        if bots.bots.is_empty() {
            rows.push(quiet("No bots on this computer yet.", false, cx));
        }
        for bot in &bots.bots {
            let key = format!("bot:{}", bot.id);
            let selected = self.is_selected(&key);
            let now = self.now();
            let status = SidebarMark::of(bot, self.has_fresh_bots(), now);
            let detail_color = status.as_ref().map_or(c.text_muted, |mark| mark.detail_color(&c));
            let waking = self.has_fresh_bots() && bot.now.waking;
            let detail = if waking && bot.now.asking == 0 && bot.now.state != kuvo_box::areas::BotState::Blocked {
                div().flex().items_center().gap(space::S1).text_role(TextRole::RowDetail)
                    .text_color(c.text_muted)
                    .child(div().size(px(5.)).flex_none().rounded(radius::PILL).bg(c.accent))
                    .child("Waking up…")
            } else {
                // What it's doing as the box last said, while the box can't be heard (restarting,
                // reconnecting): kept, faint as last received, never swapped for its job and back.
                let fresh = self.has_fresh_bots();
                let doing = doing_parts(&bot.now, now);
                let (words, waiting) = doing.map(Doing::into_parts).unwrap_or_else(|| (bot.job.clone(), None));
                let (waiting, detail_color) = if fresh { (waiting, detail_color) } else { (None, c.text_faint) };
                div().truncate().text_role(TextRole::RowDetail).text_color(detail_color).child(status_words(words, waiting, cx))
            };
            let (hover, selected_bg) = (c.hover, c.selected);
            let selector = format!("bot-{}", bot.id);
            let target = MoveTarget::new(MoveKind::Bot, &bot.id);
            let move_status = self.moves.as_ref().and_then(|moves| moves.read(cx).row_status(&target, cx));
            let face = if move_status.is_some() {
                crate::bot_face::still_bot_face(&bot.id, &bot.look, px(BOT_FACE))
            } else {
                bot_face(&bot.id, &bot.look, px(BOT_FACE))
            };
            let avatar = crate::bot_face::sidebar_avatar(&bot.id, &bot.name, face, status.as_ref());
            let trailing = status.as_ref().map(|mark| mark.trailing(&c));
            let detail = move_status.unwrap_or_else(|| detail.into_any_element());
            let row = div()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector)
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .px(space::S2)
                    .py(space::S2)
                    .mb(space::S0_5)
                    .rounded(radius::NAV)
                    .cursor_pointer()
                    .when(selected, |r| r.bg(selected_bg))
                    .hover(move |s| s.bg(if selected { selected_bg } else { hover }))
                    .on_click(self.row_click(key.clone(), cx))
                    .child(avatar)
                    .child(two_lines(bot.name.clone(), detail, &c))
                    .children(trailing);
            rows.push(self.move_menu(&key, row, target, None));
        }
        // The bots that live on your other computers, as the box says each last told it.
        if !bots.elsewhere.is_empty() {
            rows.push(SectionHeader::new("elsewhere", "On Other Computers").into_any_element());
            for listed in &bots.elsewhere {
                let there = There::of(listed);
                let key = format!("elsewhere:{}", there.id);
                let selected = self.is_selected(&key);
                let (hover, selected_bg) = (c.hover, c.selected);
                let selector = format!("elsewhere-{}", there.id);
                let computer = there.computer.as_str();
                // What waits for it or was refused, else where it lives.
                let line = if there.waiting {
                    format!("Waiting for {computer}")
                } else if there.refused {
                    format!("{computer} didn’t take a change")
                } else {
                    format!("On {computer}")
                };
                // Its computer away: the box's word on its name, muted, never a pill.
                let away = self.away_words(listed).filter(|words| !words.is_empty() && !there.waiting && !there.refused);
                let target = MoveTarget::new(MoveKind::Bot, &there.id);
                let move_status = self.moves.as_ref().and_then(|moves| moves.read(cx).row_status(&target, cx));
                let face = match &there.look {
                    Some(look) if move_status.is_some() => crate::bot_face::still_bot_face(&there.id, look, px(BOT_FACE)),
                    Some(look) => bot_face(&there.id, look, px(BOT_FACE)),
                    None => div().flex().flex_none().items_center().justify_center().size(px(BOT_FACE)).rounded(radius::PILL).bg(c.sunken).child(Icon::new(kuvo_ui::IconName::Bot).color(c.icon)).into_any_element(),
                };
                let mark = self.elsewhere_mark(listed, &there, self.now());
                let detail_color = mark.as_ref().map_or(c.text_muted, |mark| mark.detail_color(&c));
                let avatar = crate::bot_face::sidebar_avatar(&there.id, &there.name, face, mark.as_ref());
                let trailing = mark.as_ref().map(|mark| mark.trailing(&c));
                let detail = move_status.unwrap_or_else(|| {
                    div().min_w_0().flex().items_center().gap(px(4.))
                        .child(div().min_w_0().truncate().text_role(TextRole::RowDetail).text_color(detail_color).child(line))
                        .children(away.map(|words| {
                            // (What it says, for tests: "seen 2h".)
                            let said = format!("bot-said-{}-{words}", there.id);
                            div().debug_selector(move || said).child(kuvo_ui::notice::on_name(words, cx))
                        }))
                        .into_any_element()
                });
                let row = div()
                        .id(SharedString::from(selector.clone()))
                        .debug_selector(move || selector)
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .px(space::S2)
                        .py(space::S2)
                        .mb(space::S0_5)
                        .rounded(radius::NAV)
                        .cursor_pointer()
                        .when(selected, |r| r.bg(selected_bg))
                        .hover(move |s| s.bg(if selected { selected_bg } else { hover }))
                        .on_click(self.row_click(key.clone(), cx))
                        .child(avatar)
                        .child(two_lines(there.name.clone(), detail, &c))
                        .children(trailing);
                let row = match RemoteSource::of(listed) {
                    Some(source) => self.move_menu(&key, row, target, Some(source)),
                    None => row.into_any_element(),
                };
                rows.push(row);
            }
        }
        // Group chats: those that live on this box, and New Group beside their header.
        let new_group = Button::icon_only("new-group", kuvo_ui::IconName::Plus, "New Group").ghost().size(ButtonSize::Small).icon_size(kuvo_ui::IconSize::Medium).tooltip("New Group").selected(self.is_selected("new-group")).disabled(self.client.gate().is_held()).on_click(self.row_click("new-group".into(), cx));
        rows.push(SectionHeader::new("groups", "Group Chats").action(div().debug_selector(|| "new-group".into()).child(new_group)).into_any_element());
        if let (true, Some(why)) = (bots.groups.is_empty(), &bots.groups_failed) {
            rows.push(div().debug_selector(|| "groups-failed".into()).child(quiet(&format!("Group chats couldn’t be read: {why}"), false, cx)).into_any_element());
        }
        let now = self.now();
        for group in &bots.groups {
            let key = format!("group:{}", group.id);
            let when = group.last.as_ref().map(|last| time::ago(last.at_ms(), now)).unwrap_or_default();
            let selector = format!("group-{}", group.id);
            let when_selector = format!("group-{}-when-{when}", group.id);
            let leading = if group.unread {
                div().debug_selector({ let id = group.id.clone(); move || format!("group-{id}-unread") }).flex().items_center().justify_center().size(px(16.)).child(status_dot(c.accent)).into_any_element()
            } else {
                Icon::new(kuvo_ui::IconName::Message).color(c.icon).into_any_element()
            };
            let selected = self.is_selected(&key) || self.is_selected(&format!("edit-group:{}", group.id));
            let target = MoveTarget::new(MoveKind::Group, &group.id);
            let detail = self.moves.as_ref().and_then(|moves| moves.read(cx).row_status(&target, cx));
            let row = NavRow::new(SharedString::from(selector.clone()), group.name.clone())
                .leading(leading)
                .trailing(detail.unwrap_or_else(|| div().debug_selector(move || when_selector).text_role(TextRole::Meta).text_color(c.text_faint).child(when).into_any_element()))
                .selected(selected)
                .on_click(self.row_click(key.clone(), cx));
            rows.push(div().debug_selector(move || selector).pb(space::S0_5).child(self.move_menu(&key, row, target, None)).into_any_element());
        }
        rows
    }

    fn queue_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = cx.theme().colors.clone();
        let queue = match &self.queue {
            Reading::NotYet => return vec![quiet("Loading the Work Queue…", true, cx)],
            Reading::Failed(why) => return vec![quiet(&format!("The Work Queue couldn’t be read: {why}"), false, cx)],
            Reading::Ready(queue) => queue,
        };
        // The counts are the box's (`counts`, across your computers): none is counted here.
        let counts = &queue.counts;
        let count = |n: usize| div().text_role(TextRole::Meta).text_color(c.text_faint).child(n.to_string());
        let mut rows = Vec::new();
        let open = counts.open;
        let all = NavRow::new("queue-all", "All Items").icon(kuvo_ui::IconName::Queue).trailing(div().debug_selector(move || format!("queue-open-{open}")).text_role(TextRole::Meta).text_color(c.text_faint).child(format!("{open} open"))).selected(self.selected.is_none()).on_click(cx.listener(|this, _, _, cx| {
            this.select(None, cx);
            cx.emit(Picked);
        }));
        rows.push(div().debug_selector(|| "queue-all".into()).pb(space::S0_5).child(all).into_any_element());
        // Where items stand: the box's statuses, in its order, each with its count.
        rows.push(SectionHeader::new("statuses", "Status").into_any_element());
        for status in &queue.statuses {
            let key = format!("status:{}", status.id);
            let selector = format!("status-{}", status.id);
            let counted = counts.statuses.get(&status.id).copied().unwrap_or(0);
            let count_selector = format!("status-{}-count-{counted}", status.id);
            let row = NavRow::new(SharedString::from(selector.clone()), status.name.clone())
                .leading(status_mark(&status.id, cx))
                .trailing(div().debug_selector(move || count_selector).child(count(counted)))
                .selected(self.is_selected(&key))
                .on_click(self.row_click(key, cx));
            rows.push(div().debug_selector(move || selector).pb(space::S0_5).child(row).into_any_element());
        }
        // Labels are the box's, and all yours: made from here, and each changed or removed from
        // its own menu (a right-click), through the box's route.
        let page = self.queue_page.clone();
        let new_label = Button::icon_only("new-label", kuvo_ui::IconName::Plus, "New Label").ghost().size(ButtonSize::Small).icon_size(kuvo_ui::IconSize::Medium).tooltip("New Label").disabled(self.client.gate().is_held()).on_click(move |_, _, cx| {
            if let Some(page) = &page {
                page.update(cx, |page, cx| page.act(QueueAct::NewLabel, cx));
            }
        });
        rows.push(SectionHeader::new("labels", "Labels").action(div().debug_selector(|| "new-label".into()).child(new_label)).into_any_element());
        if queue.labels.is_empty() {
            rows.push(div().debug_selector(|| "labels-none".into()).child(quiet("No labels yet. Make one with +, to say what kind of work an item is.", false, cx)).into_any_element());
        }
        for label in &queue.labels {
            let key = format!("label:{}", label.id);
            let selector = format!("label-{}", label.id);
            let counted = counts.labels.get(&label.id).copied().unwrap_or(0);
            let count_selector = format!("label-{}-count-{counted}", label.id);
            let row = NavRow::new(SharedString::from(selector.clone()), label.name.clone())
                .leading(div().flex().items_center().justify_center().size(px(16.)).child(label_dot(&label.color)))
                .trailing(div().debug_selector(move || count_selector).child(count(counted)))
                .selected(self.is_selected(&key))
                .on_click(self.row_click(key, cx));
            // Its menu: from a ••• that shows under the pointer, and from a right-click.
            let (page, id) = (self.queue_page.clone(), label.id.clone());
            let build = move |_: &mut Window, cx: &mut App| {
                let (page, id) = (page.clone(), id.clone());
                cx.new(|cx| {
                    let act = |act: QueueAct| {
                        let page = page.clone();
                        move |_: &mut Window, cx: &mut App| {
                            if let Some(page) = &page {
                                page.update(cx, |page, cx| page.act(act.clone(), cx));
                            }
                        }
                    };
                    Menu::new(cx)
                        .width(px(200.))
                        .entry(MenuEntry::new("Edit Label…").icon(kuvo_ui::IconName::Pencil).on_select(act(QueueAct::EditLabel(id.clone()))))
                        .entry(MenuEntry::new("Remove Label…").icon(kuvo_ui::IconName::Trash).destructive().on_select(act(QueueAct::RemoveLabel(id.clone()))))
                })
            };
            let more_selector = format!("label-{}-more", label.id);
            let hover = c.pressed;
            let trigger = div()
                .id(SharedString::from(more_selector.clone()))
                .debug_selector(move || more_selector)
                .flex()
                .items_center()
                .justify_center()
                .size(px(22.))
                .rounded(radius::SMALL)
                .hover(move |s| s.bg(hover))
                // (Its press is its own: the row under it isn't picked.)
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(Icon::new(kuvo_ui::IconName::More).color(c.icon));
            let more = PopoverMenu::new(SharedString::from(format!("label-more-{}", label.id)), trigger, build.clone()).align_right();
            let row = row.hover_trailing(more);
            let menu = ContextMenuArea::new(SharedString::from(format!("label-menu-{}", label.id)), row, build);
            rows.push(div().debug_selector(move || selector).pb(space::S0_5).child(menu).into_any_element());
        }
        rows
    }

    /// Your computers as the box lists them (`kuvo/computers`), the one this window shows first;
    /// machines that Macs of different setups share, under their own header.
    fn computer_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = cx.theme().colors.clone();
        // All of them, side by side: the area's own page.
        let all = NavRow::new("computers-all", "All Computers").icon(kuvo_ui::IconName::Computer).selected(self.selected.is_none()).on_click(cx.listener(|this, _, _, cx| {
            this.select(None, cx);
            cx.emit(Picked);
        }));
        let mut rows = vec![
            div().debug_selector(|| "computers-all".into()).pb(space::S0_5).child(all).into_any_element(),
            SectionHeader::new("computers", "Your Computers").into_any_element(),
        ];
        let add_row = |this: &Self, cx: &mut Context<Self>| {
            let row = NavRow::new("computers-add-row", "Add a Machine…").leading(Icon::new(kuvo_ui::IconName::Plus).color(c.icon)).muted(true).selected(this.is_selected("add")).on_click(this.row_click("add".into(), cx));
            div().debug_selector(|| "computers-add-row".into()).pt(space::S1).child(row).into_any_element()
        };
        let computers = match &self.computers {
            // (The computer this window shows is known without the list: its row stays.)
            Reading::NotYet => {
                rows.push(self.this_row(cx));
                rows.push(quiet("Loading your computers…", true, cx));
                rows.push(add_row(self, cx));
                return rows;
            }
            Reading::Failed(why) => {
                rows.push(self.this_row(cx));
                rows.push(quiet(&format!("Your other computers couldn’t be read: {why}"), false, cx));
                rows.push(add_row(self, cx));
                return rows;
            }
            Reading::Ready(computers) => computers,
        };
        if !computers.iter().any(|computer| computer.here) {
            rows.push(self.this_row(cx));
        }
        rows.extend(computers.iter().map(|computer| self.computer_row(computer, cx)));
        rows.push(add_row(self, cx));
        rows
    }

    /// The row of the computer this window shows, while the box's list isn't read: the window
    /// knows which it is, and what its `kuvo/info` said it is; nothing is claimed of its reach
    /// (no dot), which only the list says.
    fn this_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let this = Computer { name: self.this_box.to_string(), kind: self.this_kind.clone(), here: true, via: "here".into(), ..Default::default() };
        self.computer_row(&this, cx)
    }

    fn computer_row(&self, computer: &Computer, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        // The one this window shows is picked as "this", by the window's own name for it.
        let (key, id, name) = if computer.here { ("this".to_owned(), "this".to_owned(), self.this_box.to_string()) } else { (computer_key(computer), computer.key.clone(), computer.name.clone()) };
        let selected = self.is_selected(&key);
        let (hover, selected_bg) = (c.hover, c.selected);
        let selector = format!("computer-{id}");
        let working = computer.working.filter(|working| *working > 0).map(|working| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(space::S1)
                .child(spinner(SharedString::from(format!("computer-spin-{id}")), IconSize::Nav, c.text_muted))
                .child(div().text_role(TextRole::Meta).text_color(c.text_faint).child(working.to_string()))
        });
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .flex()
            .items_center()
            .gap(space::S2)
            .px(space::S2)
            .py(space::S1_5)
            .mb(space::S0_5)
            .rounded(radius::NAV)
            .cursor_pointer()
            .when(selected, |r| r.bg(selected_bg))
            .hover(move |s| s.bg(if selected { selected_bg } else { hover }))
            .on_click(self.row_click(key, cx))
            .child(crate::computers::tile(computer, px(28.), cx))
            .child(two_lines(name, {
                // Its update to the setup's release, only where it can't update for a reason you
                // can fix ("Can’t update", amber); else how it stands.
                let update = (computer.here || computer.reach == "connected").then(|| crate::fleet::standing(computer)).flatten()
                    .and_then(|standing| crate::fleet::short_words(&standing.line).map(|words| (words.to_owned(), crate::fleet::color(&standing.line, cx))));
                let (line, color) = match update {
                    Some((words, color)) => (words.to_owned(), color),
                    None => (computer_line(computer), if computer.is_older() { c.warning } else { c.text_muted }),
                };
                div().truncate().text_role(TextRole::RowDetail).text_color(color).child(line)
            }, &c))
            .children(working)
            .into_any_element()
    }
}

/// What a computer's row is picked by.
fn computer_key(computer: &Computer) -> String {
    format!("computer:{}", computer.key)
}

/// A computer's line under its name, from the box's facts: the one this window shows says so; one
/// that isn't reached says that, one the box only knows of through another says through which; else its system, or Kuvo's version there, as it said them.
pub fn computer_line(computer: &Computer) -> String {
    if computer.here {
        // (Window state, not the box's: which computer this window shows is this window's own
        // choice, and another window on the same box shows another. So the words are drawn here.)
        return "Shown in this window".into();
    }
    if computer.is_older() {
        return computer.reach_title("held");
    }
    if computer.reach == "connected" {
        computer.system.clone().or_else(|| computer.kuvo_version().map(|version| format!("Kuvo {version}"))).unwrap_or_else(|| computer.title.clone())
    } else {
        computer.title.clone()
    }
}

/// What a bot is doing, in words, from the box's facts: "1 at work, 1 waiting on you", "Waiting
/// on you", "2 at work"; stopped where nothing waits on you ("Hit a usage limit", "Stopped on an
/// error"); with none of them, "Finished 3h ago" from when its newest session last changed; none
/// where the box says none of them (its row shows its job).
#[cfg(test)]
pub(crate) fn doing(now: &BotNow, at: f64) -> Option<String> {
    doing_parts(now, at).map(|doing| doing.words)
}

/// A bot's [`doing`] words, with where in them what waits on you is: the only part drawn in the
/// attention colour.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Doing {
    pub words: String,
    pub waiting: Option<std::ops::Range<usize>>,
}

impl Doing {
    pub fn into_parts(self) -> (String, Option<std::ops::Range<usize>>) {
        (self.words, self.waiting)
    }
}

pub(crate) fn doing_parts(now: &BotNow, at: f64) -> Option<Doing> {
    // Stopped, as the box says (its words in PROTOCOL.md), where nothing waits on you.
    if now.state == kuvo_box::areas::BotState::Blocked {
        return Some(Doing { words: if now.blocked_by.as_deref() == Some("limit") { "Hit a usage limit" } else { "Stopped on an error" }.to_owned(), waiting: None });
    }
    let at_work = (now.working > 0).then(|| format!("{} at work", now.working));
    let waiting = (now.asking > 0).then(|| if now.working > 0 { format!("{} waiting on you", now.asking) } else { "Waiting on you".to_owned() });
    Some(match (at_work, waiting) {
        (Some(at_work), Some(waiting)) => {
            let words = format!("{at_work}, {waiting}");
            Doing { waiting: Some(words.len() - waiting.len()..words.len()), words }
        }
        (None, Some(waiting)) => Doing { waiting: Some(0..waiting.len()), words: waiting },
        (Some(at_work), None) => Doing { words: at_work, waiting: None },
        (None, None) => Doing {
            words: match time::ago(now.finished?, at).as_str() {
                "now" => "Finished just now".to_owned(),
                ago => format!("Finished {ago} ago"),
            },
            waiting: None,
        },
    })
}

/// A bot's face in the Bots sidebar, in points: its rows are this plus even padding above and
/// below. Other places keep their own sizes (the header's 22, a mention's 16).
const BOT_FACE: f32 = 40.;
const COMPACT_FACE: Pixels = px(36.);

pub(crate) fn unknown_bot_look() -> Look {
    Look { color: "graphite".into(), shape: "round".into(), face: "calm".into() }
}

/// A two-line row's words: its name over the line under it, set as the type ramp says
/// (`TextRole::RowTitle` over `RowDetail`).
pub(crate) fn two_lines(name: impl Into<SharedString>, detail: impl IntoElement, c: &kuvo_ui::theme::Colors) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(div().min_w_0().truncate().text_role(TextRole::RowTitle).text_color(c.text).child(name.into()))
        .child(detail)
        .into_any_element()
}

/// A look's colour, by its name (graphite for one this doesn't know).
pub fn look_tint(color: &str) -> Hsla {
    rgb(match color {
        "mint" => 0x5EC7B8,
        "sky" => 0x4F9CF5,
        "indigo" => 0x5C5CF0,
        "violet" => 0x8A5CF5,
        "pink" => 0xED70B5,
        "coral" => 0xF2735C,
        "orange" => 0xF5993D,
        "amber" => 0xF2C24F,
        "lime" => 0x8CCC4F,
        _ => 0x6E7380,
    })
    .into()
}

/// A bot's face, live (`bot_face`): the creature of the bot `id` in `look`.
pub fn bot_face(id: &str, look: &Look, size: Pixels) -> AnyElement {
    crate::bot_face::bot_face(id, look, size)
}

/// Quiet words in a sidebar's place: loading, nothing yet, or what's wrong.
pub fn quiet(words: &str, busy: bool, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    if busy {
        return div().w_full().h(px(28.)).px(space::S2).flex().items_center()
            .child(div().w(px(96.)).h(px(6.)).rounded(px(3.)).bg(c.text_faint).opacity(0.2))
            .into_any_element();
    }
    div()
        .w_full()
        .flex()
        .items_start()
        .gap(space::S2)
        .px(space::S2)
        .pt(space::S3)
        .text_ui(TypeSize::Small)
        .text_color(c.text_muted)
        .child(div().flex_1().min_w_0().child(words.to_owned()))
        .into_any_element()
}

/// The main view of an area whose pages aren't in the app yet: what was picked, and that.
pub fn placeholder(area: Area, title: SharedString, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .debug_selector(|| "placeholder".into())
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(space::S3)
        .px(space::S8)
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(48.))
                .rounded(radius::PILL)
                .bg(c.sunken)
                .child(Icon::new(area.icon()).size(IconSize::Card).color(c.icon)),
        )
        .child(div().text_ui(TypeSize::Title).medium().text_color(c.text).child(title))
        .child(
            div()
                .max_w(px(420.))
                .text_center()
                .text_ui(TypeSize::Body)
                .text_color(c.text_muted)
                .child("Coming to the desktop app."),
        )
        .into_any_element()
}

/// A new view for each area past Sessions.
pub fn views(client: &BoxClient, live: bool, cx: &mut App) -> Vec<(Area, gpui::Entity<AreaView>)> {
    [Area::Bots, Area::Queue, Area::Computers, Area::Settings].into_iter().map(|area| (area, cx.new(|cx| AreaView::new(area, client.clone(), live, cx)))).collect()
}

#[cfg(test)]
mod tests {
    use super::{computer_line, doing, doing_parts};
    use kuvo_box::areas::BotNow;

    #[test]
    fn a_held_read_is_empty_without_an_area_error() {
        let read = super::reading::<super::Bots>(Err(kuvo_box::Error::NeedsUpdating { missing: vec!["typed".into()] }), &super::Reading::NotYet);
        assert!(matches!(read, super::Reading::Ready(bots) if bots.bots.is_empty()));
    }

    #[cfg(feature = "stand-in")]
    #[gpui::test]
    fn a_bots_answer_survives_an_overlapping_failure(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext as _;

        use super::{Area, AreaView, Bots, BoxClient, Followed, Read, State};
        use crate::stand_in::{Replies, StandIn};

        let stand_in = StandIn::at(Replies::Never, 1_794_650_000_000.);
        let client = BoxClient::new(stand_in.clone());
        let said = client.bots_said().unwrap();
        let failure = || kuvo_box::Error::Unreachable("offline".into());
        cx.update(|cx| {
            let view = cx.new(|cx| AreaView::new(Area::Bots, client, false, cx));
            view.update(cx, |view, cx| {
                // The stream answers while a list request is in flight. Reopening Bots
                // must keep that answer even if the request itself fails.
                view.reading = Some(cx.spawn(async |_, _| std::future::pending().await));
                view.take_followed_bots(Followed::Now(said.clone()), cx);
                stand_in.bots_readable(false);
                view.shown(cx);
                view.reading = None;
                assert_eq!(crate::bot_face::faces(cx).borrow().shown("bot_docs"), Some(State::Done));

                // Conversely, an in-flight list that succeeds can replace a stream failure.
                view.reading = Some(cx.spawn(async |_, _| std::future::pending().await));
                view.take_followed_bots(Followed::Failed(failure()), cx);
                view.reading = None;
                view.take(Read::Bots(Ok(Bots { pending_computers: said.pending_computers, bots: said.bots, ..Bots::default() })), cx);
                assert_eq!(crate::bot_face::faces(cx).borrow().shown("bot_docs"), Some(State::Done));
            });
        });
    }

    /// A computer’s line uses its box’s reach and system details.
    #[test]
    fn a_computers_line_says_how_the_box_reaches_it() {
        let computer = |json: &str| serde_json::from_str::<kuvo_box::areas::Computer>(json).unwrap();
        let line = |json: &str| computer_line(&computer(json));
        assert_eq!(line(r#"{"key":"a","name":"devbox","kind":"machine","reach":"connected","system":"Ubuntu 24.04.1 LTS","via":"relay","working":0,"limits":[]}"#), "Ubuntu 24.04.1 LTS");
        assert_eq!(line(r#"{"key":"a","name":"devbox","kind":"machine","reach":"connecting","title":"Connecting","via":"relay","working":0,"limits":[]}"#), "Connecting");
        assert_eq!(line(r#"{"key":"a","name":"devbox","kind":"machine","reach":"away","title":"Can’t be reached","via":"relay","working":0,"limits":[]}"#), "Can’t be reached");
    }

    #[test]
    fn a_bot_says_what_its_doing_from_the_boxs_facts() {
        let now = |working, asking, finished| BotNow { working, asking, finished, ..BotNow::default() };
        let hour = 3_600_000.;
        assert_eq!(doing(&now(1, 1, None), 0.).as_deref(), Some("1 at work, 1 waiting on you"));
        assert_eq!(doing(&now(0, 1, None), 0.).as_deref(), Some("Waiting on you"));
        assert_eq!(doing(&now(2, 0, None), 0.).as_deref(), Some("2 at work"));
        assert_eq!(doing(&now(0, 0, Some(hour)), 4. * hour).as_deref(), Some("Finished 3h ago"));
        assert_eq!(doing(&now(0, 0, Some(hour)), hour).as_deref(), Some("Finished just now"));
        assert_eq!(doing(&now(0, 0, None), 0.), None, "a bot with no session listed shows its job");
        // Only the part that waits on you is coloured: "1 at work" stays quiet.
        let words = |n| doing_parts(&n, 0.).map(|doing| doing.waiting.map(|range| doing.words[range].to_owned()));
        assert_eq!(words(now(1, 1, None)), Some(Some("1 waiting on you".into())));
        assert_eq!(words(now(0, 1, None)), Some(Some("Waiting on you".into())));
        assert_eq!(words(now(2, 0, None)), Some(None));
        assert_eq!(words(now(0, 0, Some(hour))), Some(None));
    }
}
