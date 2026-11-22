//! The side panel beside the conversation (#796). It follows the conversation you're in: each
//! conversation has its own strip of tabs, and every tool opens as one (a file, the changes, a
//! process's output, the subagents, a side chat, a terminal, a page); `+` opens the New tab page.
//! It opens and closes (⌘⌥0, the title bar's button, or its own ×), keeps its width while it's
//! closed and as conversations change, and is resized by its edge; in a window too narrow for it
//! beside the conversation, it slides over the conversation instead.
//!
//! Which tabs are open is the window's; what each shows it reads from the box, and changes through
//! the box's routes.

pub mod branches;
pub mod changes;
pub mod file;
pub mod files;
mod new_tab;
pub mod process;
pub mod tabs;

use std::collections::HashMap;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla, InteractiveElement, IntoElement,
    ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div, prelude::FluentBuilder as _,
    px, rgba,
};
use kuvo_box::BoxClient;
use kuvo_ui::{ActiveTheme, Icon, IconName, IconSize, StyledType, TextField, TypeSize, theme::{radius, space}};

pub use branches::BranchesView;
pub use changes::ChangesView;
pub use file::FileView;
pub use files::FilesView;
pub use process::ProcessView;
pub use tabs::{Kind, Strip, Tab};

/// The panel's width when it first opens, and the narrowest and widest it's dragged to.
pub const WIDTH: Pixels = px(480.);
pub const NARROWEST: Pixels = px(320.);
pub const WIDEST: Pixels = px(900.);
/// The conversation's narrowest beside it: in a window with less room, the panel slides over it.
pub const CONVERSATION_NARROWEST: Pixels = px(360.);
/// The tab strip's height.
pub const STRIP_HEIGHT: Pixels = px(40.);

/// The folder the panel shows: the session's own (a worktree's, for a session in one), and the
/// session in view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub directory: String,
    pub session: Option<String>,
}

/// What a view has of the box: on its way, read, or why it couldn't be.
#[derive(Clone, Debug, PartialEq)]
pub enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Load<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(value) => Some(value),
            _ => None,
        }
    }
}

/// Runs `work` (a request to the box) off the window's thread in the app, or at once (tests and
/// shots, against the stand-in, so nothing waits on a clock), and hands what it gave to `then`.
pub(crate) fn run<V: 'static, T: Send + 'static>(
    view: &mut V,
    live: bool,
    cx: &mut Context<V>,
    work: impl FnOnce() -> T + Send + 'static,
    then: impl FnOnce(&mut V, T, &mut Context<V>) + 'static,
) {
    if live {
        let task = cx.background_executor().spawn(async move { work() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| then(this, result, cx)).ok();
        })
        .detach();
    } else {
        let result = work();
        then(view, result, cx);
    }
}

/// Where the conversation sends you in the panel, from its Outputs.
#[derive(Clone, Debug, PartialEq)]
pub enum Arrival {
    /// What a command it left running prints.
    Process { session: String, command: kuvo_box::background::Command },
}

/// What the panel asks of the window around it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelEvent {
    /// Its × was pressed.
    Close,
}

/// The glass the panel draws its light elements in (never a dark slab): the showing tab's pill,
/// the New tab page's tiles, and the sheet it slides over the conversation in.
pub(crate) struct Glass {
    pub pill: Hsla,
    pub pill_edge: Hsla,
    pub tile: Hsla,
    pub tile_hover: Hsla,
    pub tile_edge: Hsla,
    pub sheet_edge: Hsla,
    pub scrim: Hsla,
}

pub(crate) fn glass(cx: &App) -> Glass {
    if cx.theme().appearance.is_dark() {
        Glass {
            pill: rgba(0xFFFFFF11).into(),
            pill_edge: rgba(0xFFFFFF12).into(),
            tile: rgba(0xFFFFFF08).into(),
            tile_hover: rgba(0xFFFFFF10).into(),
            tile_edge: rgba(0xFFFFFF0D).into(),
            sheet_edge: rgba(0xFFFFFF18).into(),
            scrim: rgba(0x00000052).into(),
        }
    } else {
        Glass {
            pill: rgba(0x2929290C).into(),
            pill_edge: rgba(0x2929290D).into(),
            tile: rgba(0x29292907).into(),
            tile_hover: rgba(0x2929290F).into(),
            tile_edge: rgba(0x2929290A).into(),
            sheet_edge: rgba(0x29292914).into(),
            scrim: rgba(0x2929291F).into(),
        }
    }
}

/// A tab's own view, kept while the tab's open.
#[derive(Clone)]
enum View {
    File(Entity<FileView>),
    Process(Entity<ProcessView>),
    Changes(Entity<ChangesView>),
    Branches(Entity<BranchesView>),
}

/// One conversation's side of the panel: its tabs, its folder, and their views.
struct Conversation {
    strip: Strip,
    /// Its panel has shown, or been asked for something.
    started: bool,
    place: Option<Place>,
    /// The folder's files, shared by its Files and file tabs.
    files: Option<Entity<FilesView>>,
    views: HashMap<u64, View>,
    subscriptions: Vec<Subscription>,
}

pub struct SidePanel {
    client: BoxClient,
    live: bool,
    /// Each conversation's, by its session (none: a session not started yet).
    conversations: HashMap<Option<String>, Conversation>,
    /// The conversation in view.
    key: Option<String>,
    shown: bool,
    /// The last tab number given.
    next: u64,
    /// Commands as the Outputs popover had them, for their tabs (by their ids).
    commands: HashMap<String, kuvo_box::background::Command>,
    /// The New tab page's search, and what the box found for it.
    search: Entity<TextField>,
    found: Option<(String, Load<kuvo_box::find::Found>)>,
    focus_search: bool,
    focus: FocusHandle,
    _search: Subscription,
}

impl EventEmitter<PanelEvent> for SidePanel {}

impl SidePanel {
    pub fn new(client: BoxClient, live: bool, cx: &mut Context<Self>) -> SidePanel {
        let search = cx.new(|cx| TextField::new(cx).bare().placeholder("Search files or enter a URL"));
        let watch = cx.subscribe(&search, |this: &mut SidePanel, _, event, cx| this.searched(event, cx));
        let mut panel = SidePanel {
            client,
            live,
            conversations: HashMap::new(),
            key: None,
            shown: false,
            next: 0,
            commands: HashMap::new(),
            search,
            found: None,
            focus_search: false,
            focus: cx.focus_handle(),
            _search: watch,
        };
        panel.conversation(cx);
        panel
    }

    /// The conversation in view's strip, made as the panel first shows it: on the New tab page,
    /// or on the tab Your settings › Panel starts each with.
    fn conversation(&mut self, _cx: &mut Context<Self>) -> &mut Conversation {
        if !self.conversations.contains_key(&self.key) {
            let strip = Strip::new(&mut self.next);
            self.conversations.insert(self.key.clone(), Conversation { strip, started: false, place: None, files: None, views: HashMap::new(), subscriptions: Vec::new() });
        }
        self.conversations.get_mut(&self.key).expect("made above")
    }

    /// The conversation in view's panel showing for the first time, nothing asked for: it starts on
    /// the tab Your settings › Panel starts each with.
    fn start(&mut self, cx: &mut Context<Self>) {
        if !self.shown {
            return;
        }
        let start = crate::yours::now(cx).panel.starts_with;
        let next = &mut self.next;
        let conversation = self.conversations.entry(self.key.clone()).or_insert_with(|| Conversation { strip: Strip::new(next), started: false, place: None, files: None, views: HashMap::new(), subscriptions: Vec::new() });
        if std::mem::replace(&mut conversation.started, true) || !conversation.strip.is_fresh() {
            return;
        }
        let kind = match start {
            crate::yours::PanelStart::NewTab => return,
            crate::yours::PanelStart::Changes => Kind::Changes,
            crate::yours::PanelStart::Files => Kind::Files,
            crate::yours::PanelStart::Branches => Kind::Branches,
        };
        conversation.strip.open(kind, &mut self.next);
    }

    /// Marks the conversation in view's panel as asked for: what you open is what it starts on.
    fn asked(&mut self, cx: &mut Context<Self>) {
        self.conversation(cx).started = true;
    }

    fn current(&self) -> Option<&Conversation> {
        self.conversations.get(&self.key)
    }

    /// The conversation in view's tabs.
    pub fn strip(&self) -> Option<&Strip> {
        self.current().map(|conversation| &conversation.strip)
    }

    /// The tab showing.
    pub fn active(&self) -> Option<&Tab> {
        self.strip().map(Strip::active)
    }

    /// The folder the conversation in view works in.
    pub fn place(&self) -> Option<&Place> {
        self.current().and_then(|conversation| conversation.place.as_ref())
    }

    pub fn is_shown(&self) -> bool {
        self.shown
    }

    /// The conversation in view's Changes, while its tab's open.
    pub fn changes(&self) -> Option<Entity<ChangesView>> {
        self.current()?.views.values().find_map(|view| match view {
            View::Changes(view) => Some(view.clone()),
            _ => None,
        })
    }

    /// The conversation in view's Branches, while its tab's open.
    pub fn branches(&self) -> Option<Entity<BranchesView>> {
        self.current()?.views.values().find_map(|view| match view {
            View::Branches(view) => Some(view.clone()),
            _ => None,
        })
    }

    /// The conversation in view's files, once a Files or file tab has opened.
    pub fn files(&self) -> Option<Entity<FilesView>> {
        self.current()?.files.clone()
    }

    /// The file tab showing.
    pub fn file(&self) -> Option<Entity<FileView>> {
        let conversation = self.current()?;
        match conversation.views.get(&conversation.strip.active().id) {
            Some(View::File(view)) => Some(view.clone()),
            _ => None,
        }
    }

    /// What the New tab page's search found.
    pub fn found(&self) -> Option<&(String, Load<kuvo_box::find::Found>)> {
        self.found.as_ref()
    }

    pub fn search(&self) -> &Entity<TextField> {
        &self.search
    }

    /// The conversation in view, and its folder (none: a session with no folder, or none open).
    pub fn set_conversation(&mut self, key: Option<String>, place: Option<Place>, cx: &mut Context<Self>) {
        if self.key == key && self.place() == place.as_ref() {
            return;
        }
        self.key = key;
        let words = if place.is_some() { "Search files or enter a URL" } else { "Search files" };
        self.conversation(cx).place = place;
        self.search.update(cx, |field, cx| {
            field.set_placeholder(words);
            field.set_text("", cx);
        });
        self.found = None;
        self.start(cx);
        self.tell(cx);
        cx.notify();
    }

    /// Whether it's on screen: what's hidden asks the box nothing.
    pub fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        if self.shown == shown {
            return;
        }
        self.shown = shown;
        self.start(cx);
        self.tell(cx);
        cx.notify();
    }

    /// Shows `kind` in the conversation in view: its tab, or a new one.
    pub fn open(&mut self, kind: Kind, cx: &mut Context<Self>) {
        self.asked(cx);
        let next = &mut self.next;
        self.conversations.get_mut(&self.key).expect("made above").strip.open(kind, next);
        self.after_move(cx);
    }

    /// Puts `kind` in the showing tab's place (a tile on the New tab page, a file picked in Files).
    pub fn replace_active(&mut self, kind: Kind, cx: &mut Context<Self>) {
        let next = &mut self.next;
        let conversation = self.conversations.get_mut(&self.key).expect("a strip shows");
        let was = conversation.strip.active().id;
        conversation.strip.replace_active(kind, next);
        if conversation.strip.active().id != was {
            conversation.views.remove(&was);
        }
        self.after_move(cx);
    }

    /// Shows what the conversation sent you to.
    pub fn arrive(&mut self, arrival: Arrival, cx: &mut Context<Self>) {
        match arrival {
            Arrival::Process { session, command } => {
                let id = command.id.clone();
                self.commands.insert(id.clone(), command);
                self.open(Kind::Process { session, command: id }, cx);
            }
        }
    }

    /// The process tab showing.
    pub fn process(&self) -> Option<Entity<ProcessView>> {
        let conversation = self.current()?;
        match conversation.views.get(&conversation.strip.active().id) {
            Some(View::Process(view)) => Some(view.clone()),
            _ => None,
        }
    }

    /// Opens the New tab page.
    pub fn open_new(&mut self, cx: &mut Context<Self>) {
        let next = &mut self.next;
        self.conversations.get_mut(&self.key).expect("a strip shows").strip.open_new(next);
        self.after_move(cx);
    }

    /// Shows the tab numbered `id`.
    pub fn select(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.conversations.get_mut(&self.key).is_some_and(|conversation| conversation.strip.select(id)) {
            self.after_move(cx);
        }
    }

    /// Closes the tab numbered `id`, and what it had open.
    pub fn close(&mut self, id: u64, cx: &mut Context<Self>) {
        let next = &mut self.next;
        let Some(conversation) = self.conversations.get_mut(&self.key) else { return };
        if conversation.strip.close(id, next).is_some() {
            conversation.views.remove(&id);
            if !conversation.strip.tabs().iter().any(|tab| matches!(tab.kind, Kind::Files | Kind::File(_))) {
                conversation.files = None;
            }
            self.after_move(cx);
        }
    }

    /// The showing tab moved: its view made where it's new, each told whether it's on screen, and
    /// what it needs read.
    fn after_move(&mut self, cx: &mut Context<Self>) {
        self.ensure_view(cx);
        self.tell(cx);
        // The New tab page takes the keyboard as it shows (once drawn).
        self.focus_search = self.current().is_some_and(|conversation| conversation.strip.active().kind == Kind::New);
        cx.notify();
    }

    /// Makes the showing tab's view, where it has none yet.
    fn ensure_view(&mut self, cx: &mut Context<Self>) {
        let (client, live) = (self.client.clone(), self.live);
        let commands = self.commands.clone();
        let Some(conversation) = self.conversations.get_mut(&self.key) else { return };
        let tab = conversation.strip.active().clone();
        if matches!(tab.kind, Kind::Files | Kind::File(_)) && conversation.files.is_none() {
            let tree = cx.new(|cx| FilesView::new(client.clone(), live, cx));
            // A file picked: its own tab (in the Files tab's place, from there).
            conversation.subscriptions.push(cx.subscribe(&tree, |this: &mut SidePanel, _, picked: &files::OpenFile, cx| {
                let kind = Kind::File(picked.0.clone());
                if this.active().is_some_and(|tab| tab.kind == Kind::Files) {
                    this.replace_active(kind, cx);
                } else {
                    this.open(kind, cx);
                }
            }));
            conversation.files = Some(tree);
        }
        match tab.kind {
            Kind::File(path) if !conversation.views.contains_key(&tab.id) => {
                let tree = conversation.files.clone().expect("made above");
                conversation.views.insert(tab.id, View::File(cx.new(|cx| FileView::new(client, live, path, tree, cx))));
            }
            Kind::Process { session, command } if !conversation.views.contains_key(&tab.id) => {
                let known = commands.get(&command).cloned();
                let view = cx.new(|cx| ProcessView::new(client, live, session, command, known, cx));
                conversation.subscriptions.push(cx.observe(&view, |_, _, cx| cx.notify()));
                conversation.views.insert(tab.id, View::Process(view));
            }
            Kind::Changes if !conversation.views.contains_key(&tab.id) => {
                let view = cx.new(|cx| ChangesView::new(client, live, cx));
                conversation.subscriptions.push(cx.subscribe(&view, |this: &mut SidePanel, _, event: &changes::ChangesEvent, cx| match event {
                    changes::ChangesEvent::ShowBranches => this.open(Kind::Branches, cx),
                }));
                conversation.views.insert(tab.id, View::Changes(view));
            }
            Kind::Branches if !conversation.views.contains_key(&tab.id) => {
                conversation.views.insert(tab.id, View::Branches(cx.new(|cx| BranchesView::new(client, live, cx))));
            }
            _ => {}
        }
    }

    /// Hands each view its conversation's folder, and whether it's the one on screen.
    fn tell(&mut self, cx: &mut Context<Self>) {
        self.ensure_view(cx);
        let key = self.key.clone();
        let shown = self.shown;
        for (at, conversation) in &self.conversations {
            let here = shown && *at == key;
            let active = conversation.strip.active();
            for (id, view) in &conversation.views {
                let on = here && *id == active.id;
                match view {
                    View::File(view) => view.update(cx, |view, cx| view.set_place(conversation.place.clone(), on, cx)),
                    View::Process(view) => view.update(cx, |view, cx| view.set_shown(on, cx)),
                    View::Changes(view) => view.update(cx, |view, cx| view.set_place(conversation.place.clone(), on, cx)),
                    View::Branches(view) => view.update(cx, |view, cx| view.set_place(conversation.place.clone(), on, cx)),
                }
            }
            if let Some(files) = &conversation.files {
                let on = here && matches!(active.kind, Kind::Files | Kind::File(_));
                files.update(cx, |view, cx| view.set_place(conversation.place.clone(), on, cx));
            }
        }
    }

    /// The New tab page's search: the folder's files the box finds for it, and the page it reads
    /// as; Enter opens the first.
    fn searched(&mut self, event: &kuvo_ui::text_field::TextFieldEvent, cx: &mut Context<Self>) {
        use kuvo_ui::text_field::TextFieldEvent;
        match event {
            TextFieldEvent::Changed => self.find(cx),
            TextFieldEvent::Submit(_) => {
                let first = self.found.as_ref().and_then(|(_, found)| found.ready()).and_then(|found| found.files.first()).map(|file| file.path.clone());
                if let Some(path) = first {
                    self.replace_active(Kind::File(path), cx);
                }
            }
            TextFieldEvent::Cancel => self.search.update(cx, |field, cx| field.set_text("", cx)),
        }
    }

    fn find(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).text().trim().to_owned();
        let place = self.place().cloned();
        let (Some(place), false) = (place, query.is_empty()) else {
            self.found = None;
            cx.notify();
            return;
        };
        if !self.client.gate().has(kuvo_box::find::FILES_FIND) {
            return;
        }
        self.found = Some((query.clone(), Load::Loading));
        let client = self.client.clone();
        let asked = query.clone();
        run(self, self.live, cx, move || client.find_files(&place.directory, &asked), move |this, found, cx| {
            if this.found.as_ref().map(|(at, _)| at) != Some(&query) {
                return;
            }
            this.found = Some((query, match found {
                Ok(found) => Load::Ready(found),
                Err(error) => Load::Failed(error.to_string()),
            }));
            cx.notify();
        });
        cx.notify();
    }

    /// A tab's title: the file's name, the command, or what the tool's called.
    fn title(conversation: &Conversation, tab: &Tab, cx: &App) -> SharedString {
        match (&tab.kind, conversation.views.get(&tab.id)) {
            (_, Some(View::Process(view))) => view.read(cx).title(),
            _ => Self::named(tab),
        }
    }

    fn named(tab: &Tab) -> SharedString {
        match &tab.kind {
            Kind::New => "New tab".into(),
            Kind::Files => "Files".into(),
            Kind::File(path) => path.rsplit('/').next().unwrap_or(path).to_owned().into(),
            Kind::Changes => "Changes".into(),
            Kind::Branches => "Branches".into(),
            Kind::Process { .. } => "Output".into(),
            Kind::Subagents => "Subagents".into(),
            Kind::SideChat(_) => "Side chat".into(),
            Kind::Terminal(_) => "Terminal".into(),
            Kind::Browser(_) => "New page".into(),
        }
    }

    fn icon(kind: &Kind) -> IconName {
        match kind {
            Kind::New => IconName::Plus,
            Kind::Files => IconName::Folder,
            Kind::File(_) => IconName::File,
            Kind::Changes | Kind::Branches => IconName::Branch,
            Kind::Process { .. } => IconName::Activity,
            Kind::Subagents => IconName::Bot,
            Kind::SideChat(_) => IconName::Message,
            Kind::Terminal(_) => IconName::TerminalWindow,
            Kind::Browser(_) => IconName::Globe,
        }
    }

    /// The strip: each tab (the showing one a soft glass pill), `+`, and the panel's ×.
    fn strip_bar(&self, conversation: &Conversation, cx: &mut Context<Self>) -> AnyElement {
        let strip = &conversation.strip;
        let c = cx.theme().colors.clone();
        let glass = glass(cx);
        let fresh = strip.is_fresh();
        let dense = strip.tabs().len() >= 4;
        let active = strip.active().id;
        let tabs = strip.tabs().iter().map(|tab| {
            let selected = tab.id == active;
            let id = tab.id;
            let closes = !fresh;
            let group = SharedString::from(format!("panel-tab-{id}"));
            div()
                .id(SharedString::from(format!("panel-tab-{id}")))
                .debug_selector(move || format!("panel-tab-{id}"))
                .group(group.clone())
                .cursor_pointer()
                .flex()
                .items_center()
                .gap(space::S1_5)
                .h(px(28.))
                .pl(space::S2)
                .pr(if closes { space::S1 } else { px(10.) })
                .rounded(radius::NAV)
                .border_1()
                .border_color(gpui::transparent_black())
                .when(dense && selected, |d| d.flex_none().min_w(px(108.)).max_w(px(152.)))
                .when(dense && !selected, |d| d.min_w(px(46.)).max_w(px(98.)).flex_shrink_1())
                .when(!dense, |d| d.min_w(px(96.)).max_w(px(176.)).flex_shrink_1())
                .when(selected, |d| d.bg(glass.pill).border_color(glass.pill_edge))
                .when(!selected, |d| d.hover(|s| s.bg(c.hover)))
                .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
                .on_mouse_down(gpui::MouseButton::Middle, cx.listener(move |this, _, _, cx| {
                    if closes {
                        this.close(id, cx);
                    }
                }))
                .child(match conversation.views.get(&tab.id) {
                    // A process still running: a calm dot in its icon's place.
                    Some(View::Process(view)) if view.read(cx).running() => div().w(px(12.)).flex_none().flex().items_center().justify_center().child(kuvo_ui::small::status_dot(c.success)).into_any_element(),
                    _ => Icon::new(Self::icon(&tab.kind)).size(IconSize::XSmall).color(if selected { c.icon_on } else { c.text_faint }).into_any_element(),
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_ui(TypeSize::Caption)
                        .when(selected, |d| d.medium())
                        .text_color(if selected { c.text } else { c.text_muted })
                        .child(Self::title(conversation, tab, cx)),
                )
                .when(closes, |d| {
                    d.child(
                        div()
                            .id(SharedString::from(format!("panel-tab-close-{id}")))
                            .debug_selector(move || format!("panel-tab-close-{id}"))
                            .cursor_pointer()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(px(18.))
                            .rounded(radius::SMALL)
                            .when(!selected, |d| d.opacity(0.).group_hover(group.clone(), |s| s.opacity(1.)))
                            .hover(|s| s.bg(c.hover))
                            .tooltip(|_, cx| kuvo_ui::menu::Tooltip::new("Close Tab").build(cx))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close(id, cx);
                            }))
                            .child(Icon::new(IconName::Close).size(IconSize::Indicator).color(c.text_faint)),
                    )
                })
        });
        let button = |id: &'static str, icon: IconName, size: IconSize, tip: &'static str| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .cursor_pointer()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(26.))
                .rounded(radius::NAV)
                .hover(|s| s.bg(c.hover))
                .tooltip(move |_, cx| kuvo_ui::menu::Tooltip::new(tip).build(cx))
                .child(Icon::new(icon).size(size).color(c.icon))
        };
        div()
            .id("panel-strip")
            .debug_selector(|| "panel-strip".into())
            .flex()
            .flex_none()
            .items_center()
            .gap(space::S1_5)
            .h(STRIP_HEIGHT)
            .px(space::S2)
            .border_b_1()
            .border_color(c.border)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(px(2.))
                    .overflow_hidden()
                    .children(tabs)
                    .when(!fresh, |d| d.child(button("panel-new-tab", IconName::Plus, IconSize::Nav, "New Tab").on_click(cx.listener(|this, _, _, cx| this.open_new(cx))))),
            )
            .child(button("panel-close", IconName::Close, IconSize::XSmall, "Close Panel").on_click(cx.listener(|_, _, _, cx| cx.emit(PanelEvent::Close))))
            .into_any_element()
    }

    fn body(&self, conversation: &Conversation, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let tab = conversation.strip.active();
        match (&tab.kind, conversation.views.get(&tab.id)) {
            (Kind::New, _) => new_tab::render(self, conversation.place.as_ref(), window, cx),
            (Kind::Files, _) => match (&conversation.place, &conversation.files) {
                (Some(_), Some(files)) => files.clone().into_any_element(),
                _ => no_folder(cx),
            },
            (_, Some(View::File(view))) => view.clone().into_any_element(),
            (_, Some(View::Process(view))) => view.clone().into_any_element(),
            (Kind::Changes | Kind::Branches, _) if conversation.place.is_none() => no_folder(cx),
            (_, Some(View::Changes(view))) => view.clone().into_any_element(),
            (_, Some(View::Branches(view))) => view.clone().into_any_element(),
            _ => new_tab::render(self, conversation.place.as_ref(), window, cx),
        }
    }
}

/// A conversation with no folder, on a tab that shows one's (nothing reaches it there today).
fn no_folder(cx: &App) -> AnyElement {
    changes::empty("No Folder Here", "This conversation doesn’t work in a folder, so it has no files or changes to show.", None, cx)
}

impl Focusable for SidePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SidePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.conversation(cx);
        if std::mem::take(&mut self.focus_search) {
            window.focus(&self.search.focus_handle(cx), cx);
        }
        let c = cx.theme().colors.clone();
        let conversation = self.conversations.remove(&self.key).expect("made above");
        let strip = self.strip_bar(&conversation, cx);
        let body = self.body(&conversation, window, cx);
        self.conversations.insert(self.key.clone(), conversation);
        div()
            .id("side-panel")
            .debug_selector(|| "side-panel".into())
            .key_context("SidePanel")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(c.surface)
            .text_ui(TypeSize::Small)
            .text_color(c.text)
            .child(strip)
            .child(div().flex().flex_col().flex_1().min_h_0().child(body))
    }
}
