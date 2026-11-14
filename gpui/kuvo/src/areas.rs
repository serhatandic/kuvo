//! The sidebars of the areas past Sessions, beside the rail: Bots (its bots, with what each is
//! doing now, and group chats), the Work Queue (In Progress and its categories, with the box's
//! counts), Computers (yours, as the box lists them) and Settings (its sections). Each is its own
//! view, kept while another area shows and drawn only while its own does; Bots and the Work Queue
//! ask the box as they open, and the window hands Computers the list its title bar draws too.
//!
//! Bots' main view is its own (`crate::bots`): a bot's page, New Bot. The Work Queue's and
//! Computers' pages, and a group chat's, aren't in the app yet: picking a row there shows a plain
//! page saying so.

use gpui::{
    AnyElement, App, AppContext as _, Context, EventEmitter, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, Task, Window, div, prelude::FluentBuilder as _, px, rgb,
};
use kuvo_box::BoxClient;
use kuvo_box::areas::{Bot, BotNow, Computer, Group, Look, WorkQueue};
use kuvo_box::bots::{BotsSaid, Elsewhere, Held, Refused};
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, Icon, IconSize, StyledType, TypeSize,
    row::{NavRow, SectionHeader},
    small::{count_badge, spinner, status_dot},
    theme::{radius, space},
};

use crate::area::Area;
use crate::bots::{BotsEvent, BotsView, There, bots_there};
use crate::settings::{Pick, SettingsView};
use crate::time;


/// How an area's list stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Reading<T> {
    NotYet,
    Ready(T),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Bots {
    pub bots: Vec<Bot>,
    /// The bots that live on your other computers, as each last told the box (`kuvo/bots`).
    pub elsewhere: Vec<Elsewhere>,
    /// What the box holds for your other computers, and what they refused (`kuvo/bots`).
    pub held: Vec<Held>,
    pub refused: Vec<Refused>,
    /// The box couldn't say what it holds just now: what it last said is kept.
    pub held_unsaid: bool,
    pub groups: Vec<Group>,
}

impl Bots {
    /// The bots that live on your other computers, as they're drawn: those the box lists, in its
    /// order, then any it only holds an edit for or heard a refusal of.
    pub fn there(&self) -> Vec<There> {
        bots_there(&self.elsewhere, &self.held, &self.refused, &self.bots)
    }
}

/// A row was picked: the window goes to its page.
pub struct Picked;

/// Somewhere the window is to go, asked from an area's page.
#[derive(Clone, Debug, PartialEq)]
pub enum Go {
    /// A bot's own conversation, with `draft` in its composer where the box gave one.
    BotChat { bot: String, session: String, draft: Option<String> },
    /// One of the box's sessions.
    Session(String),
}

/// One area's sidebar.
pub struct AreaView {
    area: Area,
    client: BoxClient,
    live: bool,
    bots: Reading<Bots>,
    queue: Reading<WorkQueue>,
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
    /// Settings' sidebar is drawn here: redrawn as Settings changes (a search, a reading, a sheet).
    _settings_changes: Option<gpui::Subscription>,
    /// Bots' main view: a bot's page and New Bot.
    page: Option<gpui::Entity<BotsView>>,
    _page: Vec<gpui::Subscription>,
}

impl EventEmitter<Picked> for AreaView {}
impl EventEmitter<Go> for AreaView {}

impl AreaView {
    pub fn new(area: Area, client: BoxClient, live: bool, cx: &mut Context<Self>) -> AreaView {
        let selected = (area == Area::Settings).then(|| "General".to_owned());
        let settings = (area == Area::Settings).then(|| cx.new(|cx| SettingsView::new(client.clone(), live, cx)));
        // A section picked in Settings' own sidebar is picked here, as any area's row is.
        let picks = settings.as_ref().map(|settings| {
            cx.subscribe(settings, |this: &mut AreaView, _, pick: &Pick, cx| {
                if pick.record {
                    this.pick(pick.section.clone(), cx);
                } else {
                    this.select(Some(pick.section.clone()), cx);
                }
            })
        });
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
                    }),
                    // Its sheet dims the sidebar too: redrawn as it opens and closes.
                    cx.observe(page, |_, _, cx| cx.notify()),
                ]
            })
            .unwrap_or_default();
        AreaView { page, _page: page_subscriptions, area, client, live, bots: Reading::NotYet, queue: Reading::NotYet, computers: Reading::NotYet, this_box: "This Mac".into(), this_kind: String::new(), selected, now: None, reading: None, settings, _settings_picks: picks, _settings_changes: changes }
    }

    /// Settings' view, in the Settings area.
    pub fn settings(&self) -> Option<&gpui::Entity<SettingsView>> {
        self.settings.as_ref()
    }

    /// Bots' main view, in the Bots area.
    pub fn page(&self) -> Option<&gpui::Entity<BotsView>> {
        self.page.as_ref()
    }

    /// The area's own main view, for what's picked: a bot's page or New Bot. None where the page
    /// isn't in the app yet (a group chat, the Work Queue, a computer).
    pub fn main(&self) -> Option<gpui::AnyView> {
        let page = self.page.as_ref()?;
        let other = self.selected.as_deref().is_some_and(|key| key.starts_with("group:"));
        (!other).then(|| page.clone().into())
    }

    pub fn area(&self) -> Area {
        self.area
    }

    pub fn set_now(&mut self, now: f64, cx: &mut Context<Self>) {
        self.now = Some(now);
        if let Some(settings) = &self.settings {
            settings.update(cx, |settings, _| settings.set_now(now));
        }
        if let Some(page) = &self.page {
            page.update(cx, |page, _| page.set_now(now));
        }
    }

    pub fn set_this_box(&mut self, name: SharedString, kind: &str, cx: &mut Context<Self>) {
        if self.this_kind != kind {
            self.this_kind = kind.to_owned();
            cx.notify();
        }
        if self.this_box != name {
            if let Some(settings) = &self.settings {
                settings.update(cx, |settings, cx| settings.set_this_box(&name, cx));
            }
            if let Some(page) = &self.page {
                page.update(cx, |page, cx| page.set_this_box(&name, cx));
            }
            self.this_box = name;
            cx.notify();
        }
    }

    /// Your computers, as the window read them.
    pub fn set_computers(&mut self, computers: Reading<Vec<Computer>>, cx: &mut Context<Self>) {
        if self.computers != computers {
            self.computers = computers;
            cx.notify();
        }
    }

    /// It's shown: its list is read from the box (in the app, on another thread; in tests and
    /// shots, at once). What was read before stays on screen meanwhile.
    pub fn shown(&mut self, cx: &mut Context<Self>) {
        let (area, client) = (self.area, self.client.clone());
        if let Some(settings) = &self.settings {
            settings.update(cx, |settings, cx| settings.shown(cx));
            return;
        }
        // The page open reads what it's drawn from again, as the area comes on screen.
        if let Some(page) = &self.page {
            page.update(cx, |page, cx| page.read(cx));
        }
        if !self.live {
            let read = read(area, &client);
            self.take(read, cx);
            return;
        }
        self.reading = Some(cx.spawn(async move |this, cx| {
            let read = cx.background_executor().spawn(async move { read(area, &client) }).await;
            this.update(cx, |this, cx| this.take(read, cx)).ok();
        }));
    }

    fn take(&mut self, read: Read, cx: &mut Context<Self>) {
        match read {
            Read::Bots(bots) => {
                // What the box couldn't say it holds just now stays as it last said it.
                let bots = bots.map(|mut bots| {
                    if bots.held_unsaid
                        && let Reading::Ready(before) = &self.bots
                    {
                        bots.held = before.held.clone();
                        bots.refused = before.refused.clone();
                    }
                    bots
                });
                self.bots = reading(bots, &self.bots);
                // The same reading, handed to the page.
                let said = match &self.bots {
                    Reading::NotYet => Reading::NotYet,
                    Reading::Failed(why) => Reading::Failed(why.clone()),
                    Reading::Ready(bots) => Reading::Ready(BotsSaid { bots: bots.bots.clone(), elsewhere: bots.elsewhere.clone(), held: bots.held.clone(), refused: bots.refused.clone(), held_unsaid: bots.held_unsaid }),
                };
                if let Some(page) = &self.page {
                    page.update(cx, |page, cx| page.set_said(said, cx));
                }
            }
            Read::Queue(queue) => self.queue = reading(queue, &self.queue),
            Read::Nothing => {}
        }
        cx.notify();
    }

    pub fn bots(&self) -> &Reading<Bots> {
        &self.bots
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// Picks a row (or none: the area's own page), as going back or forward does.
    pub fn select(&mut self, key: Option<String>, cx: &mut Context<Self>) {
        self.selected = key;
        self.show_section(cx);
        cx.notify();
    }

    fn pick(&mut self, key: String, cx: &mut Context<Self>) {
        self.selected = Some(key);
        self.show_section(cx);
        cx.emit(Picked);
        cx.notify();
    }

    /// Settings shows the section picked (its first, General, for none); Bots, what's picked.
    fn show_section(&mut self, cx: &mut Context<Self>) {
        let section = self.selected.clone().unwrap_or_else(|| "General".into());
        if let Some(settings) = &self.settings {
            settings.update(cx, |settings, cx| settings.set_section(&section, cx));
        }
        if let Some(page) = &self.page {
            let key = self.selected.clone();
            page.update(cx, |page, cx| page.show(key.as_deref(), cx));
        }
    }

    /// The page's title: what's picked, named as its row is.
    pub fn title(&self) -> SharedString {
        let Some(key) = self.selected.as_deref() else { return self.area.name().into() };
        let (kind, id) = key.split_once(':').unwrap_or(("", key));
        let named = match kind {
            "bot" => ready(&self.bots).and_then(|bots| bots.bots.iter().find(|bot| bot.id == id).map(|bot| bot.name.clone())),
            "elsewhere" => ready(&self.bots).and_then(|bots| bots.there().into_iter().find(|there| there.id == id).map(|there| there.name)),
            _ if key == "new" => Some("New Bot".into()),
            "group" => ready(&self.bots).and_then(|bots| bots.groups.iter().find(|group| group.id == id).map(|group| group.name.clone())),
            "category" => ready(&self.queue).and_then(|queue| queue.categories.iter().find(|category| category.id == id).map(|category| category.name.clone())),
            "computer" => ready(&self.computers).and_then(|computers| computers.iter().find(|computer| computer.key == id).map(|computer| computer.name.clone())),
            "progress" => Some("In Progress".into()),
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
    Queue(kuvo_box::Result<WorkQueue>),
    Nothing,
}

fn read(area: Area, client: &BoxClient) -> Read {
    match area {
        Area::Bots => Read::Bots(client.bots_said().map(|said| {
            // Group chats come from the same box; a box whose chats can't be read still lists its bots.
            let groups = client.groups().unwrap_or_default();
            Bots { bots: said.bots, elsewhere: said.elsewhere, held: said.held, refused: said.refused, held_unsaid: said.held_unsaid, groups }
        })),
        Area::Queue => Read::Queue(client.work_queue()),
        _ => Read::Nothing,
    }
}

/// What a read makes of the list: what it read, or (failing) what was there, else why.
pub fn reading<T: Clone>(read: kuvo_box::Result<T>, before: &Reading<T>) -> Reading<T> {
    match (read, before) {
        (Ok(value), _) => Reading::Ready(value),
        (Err(_), Reading::Ready(value)) => Reading::Ready(value.clone()),
        (Err(error), _) => Reading::Failed(error.to_string()),
    }
}

fn ready<T>(reading: &Reading<T>) -> Option<&T> {
    match reading {
        Reading::Ready(value) => Some(value),
        _ => None,
    }
}

impl Render for AreaView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(settings) = self.settings.clone() {
            return div().id("area-settings").size_full().child(settings.update(cx, |settings, cx| settings.sidebar(cx)));
        }
        let rows = match self.area {
            Area::Bots => self.bots_rows(cx),
            Area::Queue => self.queue_rows(cx),
            Area::Computers => self.computer_rows(cx),
            Area::Settings => Vec::new(),
            Area::Sessions => Vec::new(),
        };
        // A sheet open on the area's page: the sidebar waits under the same dimming.
        let dimmed = self.page.as_ref().is_some_and(|page| page.read(cx).sheet().is_some());
        div()
            .id("area-frame")
            .relative()
            .size_full()
            .child(div().id(SharedString::from(format!("area-{}", self.area.slug()))).size_full().overflow_y_scroll().px(space::S2).pt(space::S2).pb(space::S4).children(rows))
            .when(dimmed, |d| d.child(div().id("area-sidebar-dim").debug_selector(|| "area-sidebar-dim".into()).absolute().inset_0().bg(crate::bots::dim(cx)).occlude()))
    }
}

impl AreaView {
    fn row_click(&self, key: String, cx: &mut Context<Self>) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static {
        cx.listener(move |this, _, _, cx| this.pick(key.clone(), cx))
    }

    fn is_selected(&self, key: &str) -> bool {
        self.selected.as_deref() == Some(key)
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
            .child(div().text_ui(TypeSize::Body).medium().text_color(c.text).child("Bots"))
            .child(Button::icon_only("new-bot", kuvo_ui::IconName::Plus, "New Bot").ghost().size(ButtonSize::Small).selected(self.is_selected("new")).tooltip("New Bot").on_click(self.row_click("new".into(), cx)))
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
            // What it's doing now is the box's (`now`): its sessions at work and waiting on you,
            // or when the newest finished; with none of them, its job.
            let (working, asking) = (bot.now.working, bot.now.asking);
            let words = doing(&bot.now, self.now()).unwrap_or_else(|| bot.job.clone());
            let id = bot.id.clone();
            let trailing: Option<AnyElement> = if asking > 0 {
                Some(div().debug_selector(move || format!("bot-asking-{id}")).child(count_badge(asking, cx)).into_any_element())
            } else if working > 0 {
                Some(div().debug_selector(move || format!("bot-working-{id}")).child(spinner(SharedString::from(format!("bot-spin-{}", bot.id)), IconSize::Nav, c.text_muted)).into_any_element())
            } else {
                None
            };
            let (hover, selected_bg) = (c.hover, c.selected_card);
            let selector = format!("bot-{}", bot.id);
            rows.push(
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
                    .map(|r| kuvo_ui::row::selected_card(r, selected, cx))
                    .hover(move |s| s.bg(if selected { selected_bg } else { hover }))
                    .on_click(self.row_click(key, cx))
                    // Still, as the user asked: the Mac's animated faces aren't brought over yet.
                    .child(bot_face(&bot.look, px(28.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().text_ui(TypeSize::Body).text_color(c.text).child(bot.name.clone()))
                            .child(div().truncate().text_ui(TypeSize::Small).text_color(if asking > 0 { c.accent } else { c.text_muted }).child(words)),
                    )
                    .children(trailing.map(|trailing| div().flex_none().child(trailing)))
                    .into_any_element(),
            );
        }
        // The bots that live on your other computers, as the box says each last told it.
        let elsewhere = bots.there();
        if !elsewhere.is_empty() {
            rows.push(SectionHeader::new("elsewhere", "On Other Computers").into_any_element());
            for there in elsewhere {
                let key = format!("elsewhere:{}", there.id);
                let selected = self.is_selected(&key);
                let (hover, selected_bg) = (c.hover, c.selected_card);
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
                let face = match &there.look {
                    Some(look) => bot_face(look, px(28.)),
                    None => div().flex().flex_none().items_center().justify_center().size(px(28.)).rounded(radius::PILL).bg(c.sunken).child(Icon::new(kuvo_ui::IconName::Bot).color(c.icon)).into_any_element(),
                };
                rows.push(
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
                        .map(|r| kuvo_ui::row::selected_card(r, selected, cx))
                        .hover(move |s| s.bg(if selected { selected_bg } else { hover }))
                        .on_click(self.row_click(key, cx))
                        .child(face)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().truncate().text_ui(TypeSize::Body).text_color(c.text).child(there.name.clone()))
                                .child(div().truncate().text_ui(TypeSize::Small).text_color(c.text_muted).child(line)),
                        )
                        .into_any_element(),
                );
            }
        }
        if !bots.groups.is_empty() {
            rows.push(SectionHeader::new("groups", "Group Chats").into_any_element());
            let now = self.now();
            for group in &bots.groups {
                let key = format!("group:{}", group.id);
                let when = group.last.as_ref().map(|last| time::ago(last.time, now)).unwrap_or_default();
                let selector = format!("group-{}", group.id);
                let row = NavRow::new(SharedString::from(selector.clone()), group.name.clone())
                    .leading(if group.unread { status_dot(c.accent).into_any_element() } else { Icon::new(kuvo_ui::IconName::Message).color(c.icon).into_any_element() })
                    .trailing(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(when))
                    .selected(self.is_selected(&key))
                    .on_click(self.row_click(key, cx));
                rows.push(div().debug_selector(move || selector).pb(space::S0_5).child(row).into_any_element());
            }
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
        let count = |n: usize| div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(n.to_string());
        let mut rows = Vec::new();
        let progress = NavRow::new("queue-progress", "In Progress")
            .leading(if counts.in_progress > 0 { spinner("queue-spin", IconSize::Nav, c.text_muted).into_any_element() } else { Icon::new(kuvo_ui::IconName::Clock).color(c.icon).into_any_element() })
            .trailing(count(counts.in_progress))
            .selected(self.is_selected("progress"))
            .on_click(self.row_click("progress".into(), cx));
        rows.push(div().debug_selector(|| "queue-progress".into()).pb(space::S0_5).child(progress).into_any_element());
        rows.push(SectionHeader::new("categories", "Categories").into_any_element());
        for category in &queue.categories {
            let key = format!("category:{}", category.id);
            let selector = format!("category-{}", category.id);
            let row = NavRow::new(SharedString::from(selector.clone()), category.name.clone())
                .leading(div().size(px(8.)).rounded(radius::PILL).bg(category_color(&category.id, cx)))
                .trailing(count(counts.categories.get(&category.id).copied().unwrap_or(0)))
                .selected(self.is_selected(&key))
                .on_click(self.row_click(key, cx));
            rows.push(div().debug_selector(move || selector).pb(space::S0_5).child(row).into_any_element());
        }
        rows
    }

    /// Your computers as the box lists them (`kuvo/computers`), the one this window shows first;
    /// machines that Macs of different setups share, under their own header.
    fn computer_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![SectionHeader::new("computers", "Your Computers").into_any_element()];
        let computers = match &self.computers {
            // (The computer this window shows is known without the list: its row stays.)
            Reading::NotYet => {
                rows.push(self.this_row(cx));
                rows.push(quiet("Loading your computers…", true, cx));
                return rows;
            }
            Reading::Failed(why) => {
                rows.push(self.this_row(cx));
                rows.push(quiet(&format!("Your other computers couldn’t be read: {why}"), false, cx));
                return rows;
            }
            Reading::Ready(computers) => computers,
        };
        let (shared, yours): (Vec<&Computer>, Vec<&Computer>) = computers.iter().partition(|computer| computer.shared || computer.apart);
        rows.extend(yours.into_iter().map(|computer| self.computer_row(computer, cx)));
        if !shared.is_empty() {
            rows.push(div().debug_selector(|| "header-shared".into()).child(SectionHeader::new("shared", "Shared Machines")).into_any_element());
            rows.extend(shared.into_iter().map(|computer| self.computer_row(computer, cx)));
        }
        rows
    }

    /// The row of the computer this window shows, while the box's list isn't read: the window
    /// knows which it is, and what its `kuvo/info` said it is; nothing is claimed of its reach
    /// (no dot), which only the list says.
    fn this_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let this = Computer { key: String::new(), name: self.this_box.to_string(), kind: self.this_kind.clone(), here: true, id: None, shared: false, apart: false, reach: String::new(), version: None, engine: None, system: None, via: "here".into(), through: None, working: 0, limits: Vec::new() };
        self.computer_row(&this, cx)
    }

    fn computer_row(&self, computer: &Computer, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        // The one this window shows is picked as "this", by the window's own name for it.
        let (key, id, name) = if computer.here { ("this".to_owned(), "this".to_owned(), self.this_box.to_string()) } else { (computer_key(computer), computer.key.clone(), computer.name.clone()) };
        let selected = self.is_selected(&key);
        let dot = match computer.reach.as_str() {
            "connected" => c.success,
            "connecting" => c.warning,
            "away" => c.danger,
            // Known by another's word, or a word a newer box names: no claim of this app's about it.
            "unknown" => c.text_faint,
            _ => c.text_faint,
        };
        // None said (the list isn't read yet): no dot.
        let dot = (!computer.reach.is_empty()).then_some(dot);
        let dot_selector = format!("computer-dot-{id}");
        let (hover, selected_bg) = (c.hover, c.selected_card);
        let selector = format!("computer-{id}");
        let working = (computer.working > 0).then(|| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(space::S1)
                .child(spinner(SharedString::from(format!("computer-spin-{id}")), IconSize::Nav, c.text_muted))
                .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(computer.working.to_string()))
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
            .map(|r| kuvo_ui::row::selected_card(r, selected, cx))
            .hover(move |s| s.bg(if selected { selected_bg } else { hover }))
            .on_click(self.row_click(key, cx))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(28.))
                    .rounded(radius::NAV)
                    .bg(c.sunken)
                    .border_1()
                    .border_color(c.border)
                    .child(Icon::new(if computer.is_mac() { kuvo_ui::IconName::Computer } else { kuvo_ui::IconName::Terminal }).color(c.icon))
                    .children(dot.map(|dot| div().debug_selector(move || dot_selector).absolute().bottom(px(-2.)).right(px(-2.)).size(px(9.)).rounded(radius::PILL).border_2().border_color(c.surface).bg(dot))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().text_ui(TypeSize::Body).text_color(c.text).child(name))
                    .child(div().truncate().text_ui(TypeSize::Small).text_color(c.text_muted).child(computer_line(computer))),
            )
            .children(working)
            .into_any_element()
    }
}

/// What a computer's row is picked by.
fn computer_key(computer: &Computer) -> String {
    format!("computer:{}", computer.key)
}

/// A computer's line under its name, from the box's facts: the one this window shows says so; one
/// that isn't reached says that, one the box only knows of through another says through which; else its system, or Kuvo Code's version there, as it said them.
pub fn computer_line(computer: &Computer) -> String {
    if computer.here {
        return "Shown in this window".into();
    }
    match computer.reach.as_str() {
        "connected" => computer.system.clone().or_else(|| computer.version.as_ref().map(|version| format!("Kuvo Code {version}"))).unwrap_or_else(|| "Connected".into()),
        "connecting" => "Connecting…".into(),
        // The box knows it only by another computer's word (`via: peer`): it says who, never that it's away.
        "unknown" => computer.through.as_ref().map(|through| format!("Known through {through}")).unwrap_or_else(|| "Known through another computer".into()),
        "away" => "Can’t be reached".into(),
        // A word a newer box names: what it runs, with no claim about its reach.
        _ => computer.system.clone().or_else(|| computer.version.as_ref().map(|version| format!("Kuvo Code {version}"))).unwrap_or_else(|| computer.name.clone()),
    }
}

/// What a bot is doing, in words, from the box's facts: "1 at work, 1 waiting on you", "Waiting
/// on you", "2 at work"; with neither, "Finished 3h ago" from when its newest session last changed;
/// none where the box says none of them (its row shows its job).
pub(crate) fn doing(now: &BotNow, at: f64) -> Option<String> {
    let at_work = (now.working > 0).then(|| format!("{} at work", now.working));
    let waiting = (now.asking > 0).then(|| if now.working > 0 { format!("{} waiting on you", now.asking) } else { "Waiting on you".to_owned() });
    match (at_work, waiting) {
        (Some(at_work), Some(waiting)) => Some(format!("{at_work}, {waiting}")),
        (None, None) => now.finished.map(|finished| match time::ago(finished, at).as_str() {
            "now" => "Finished just now".to_owned(),
            ago => format!("Finished {ago} ago"),
        }),
        (at_work, waiting) => at_work.or(waiting),
    }
}

/// A category's dot, by KuvoClient's rule (`WorkItemChip.color(of:)`: Next teal, Later purple,
/// Parked grey, Done green, a custom one's by its id), in the theme's nearest colours.
fn category_color(id: &str, cx: &App) -> Hsla {
    let c = &cx.theme().colors;
    match id {
        "next" => c.syntax.property,
        "later" => c.syntax.type_name,
        "parked" => c.text_faint,
        "done" => c.success,
        _ => c.syntax.number,
    }
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

/// A bot's still face: its look's colour and shape, with calm eyes (the Mac's `BotAvatar` drawn
/// without moving; its faces and animation aren't brought over yet). A look this doesn't know is
/// graphite and round, as the Mac shows one.
pub fn bot_face(look: &Look, size: Pixels) -> AnyElement {
    let tint = look_tint(&look.color);
    // Deep tints get light eyes, as the Mac's (`isDeep`).
    let deep = !matches!(look.color.as_str(), "mint" | "sky" | "pink" | "coral" | "orange" | "amber" | "lime");
    let eye = if deep { gpui::hsla(0., 0., 1., 0.92) } else { gpui::hsla(0., 0., 0.12, 0.9) };
    let corner = match look.shape.as_str() {
        "squircle" => size * 0.3,
        "pebble" => size * 0.42,
        "gem" => size * 0.18,
        "bloom" => size * 0.36,
        _ => size / 2.,
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(size * 0.16)
        .size(size)
        .rounded(corner)
        .bg(tint)
        .child(div().w(size * 0.11).h(size * 0.2).rounded(radius::PILL).bg(eye))
        .child(div().w(size * 0.11).h(size * 0.2).rounded(radius::PILL).bg(eye))
        .into_any_element()
}

/// Quiet words in a sidebar's place: loading, nothing yet, or what's wrong.
pub fn quiet(words: &str, busy: bool, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .w_full()
        .flex()
        .items_start()
        .gap(space::S2)
        .px(space::S2)
        .pt(space::S3)
        .text_ui(TypeSize::Small)
        .text_color(c.text_muted)
        .children(busy.then(|| div().pt(px(3.)).child(spinner("area-loading", IconSize::Nav, c.text_muted))))
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
                .child("This page isn’t in this app yet. For now, open it in Kuvo Code on your Mac or iPhone."),
        )
        .into_any_element()
}

/// A new view for each area past Sessions.
pub fn views(client: &BoxClient, live: bool, cx: &mut App) -> Vec<(Area, gpui::Entity<AreaView>)> {
    [Area::Bots, Area::Queue, Area::Computers, Area::Settings].into_iter().map(|area| (area, cx.new(|cx| AreaView::new(area, client.clone(), live, cx)))).collect()
}

#[cfg(test)]
mod tests {
    use super::{computer_line, doing};
    use kuvo_box::areas::BotNow;

    /// A computer's line says what the box says of it: one it only knows of through another is
    /// never said to be away.
    #[test]
    fn a_computers_line_says_how_the_box_reaches_it() {
        let computer = |json: &str| serde_json::from_str::<kuvo_box::areas::Computer>(json).unwrap();
        let line = |json: &str| computer_line(&computer(json));
        assert_eq!(line(r#"{"key":"a","name":"devbox","kind":"machine","shared":false,"reach":"connected","system":"Ubuntu 24.04.1 LTS","via":"relay","working":0,"limits":[]}"#), "Ubuntu 24.04.1 LTS");
        assert_eq!(line(r#"{"key":"a","name":"devbox","kind":"machine","shared":false,"reach":"connecting","via":"relay","working":0,"limits":[]}"#), "Connecting…");
        assert_eq!(line(r#"{"key":"a","name":"devbox","kind":"machine","shared":false,"reach":"away","via":"relay","working":0,"limits":[]}"#), "Can’t be reached");
        assert_eq!(line(r#"{"key":"b","name":"Studio","kind":"mac","shared":false,"reach":"unknown","via":"peer","through":"devbox","working":0,"limits":[]}"#), "Known through devbox");
    }

    #[test]
    fn a_bot_says_what_its_doing_from_the_boxs_facts() {
        let now = |working, asking, finished| BotNow { working, asking, finished };
        let hour = 3_600_000.;
        assert_eq!(doing(&now(1, 1, None), 0.).as_deref(), Some("1 at work, 1 waiting on you"));
        assert_eq!(doing(&now(0, 1, None), 0.).as_deref(), Some("Waiting on you"));
        assert_eq!(doing(&now(2, 0, None), 0.).as_deref(), Some("2 at work"));
        assert_eq!(doing(&now(0, 0, Some(hour)), 4. * hour).as_deref(), Some("Finished 3h ago"));
        assert_eq!(doing(&now(0, 0, Some(hour)), hour).as_deref(), Some("Finished just now"));
        assert_eq!(doing(&now(0, 0, None), 0.), None, "a bot with no session listed shows its job");
    }
}
