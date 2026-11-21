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
pub mod files;
mod new_tab;
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
pub use files::FilesView;
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
    Changes(Entity<ChangesView>),
    Branches(Entity<BranchesView>),
}

/// One conversation's side of the panel: its tabs, its folder, and their views.
struct Conversation {
    strip: Strip,
    place: Option<Place>,
    /// The folder's files, shared by its Files and file tabs.
    files: Option<Entity<FilesView>>,
    views: HashMap<u64, View>,
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
    /// The New tab page's search.
    search: Entity<TextField>,
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
            search,
            focus: cx.focus_handle(),
            _search: watch,
        };
        panel.conversation(cx);
        panel
    }

    /// The conversation in view's strip, made as the panel first shows it: on the New tab page,
    /// or on the tab Your settings › Panel starts each with.
    fn conversation(&mut self, cx: &mut Context<Self>) -> &mut Conversation {
        if !self.conversations.contains_key(&self.key) {
            let mut strip = Strip::new(&mut self.next);
            match crate::yours::now(cx).panel.starts_with {
                crate::yours::PanelStart::NewTab => {}
                crate::yours::PanelStart::Changes => {
                    strip.open(Kind::Changes, &mut self.next);
                }
                crate::yours::PanelStart::Files => {
                    strip.open(Kind::Files, &mut self.next);
                }
                crate::yours::PanelStart::Branches => {
                    strip.open(Kind::Branches, &mut self.next);
                }
            }
            self.conversations.insert(self.key.clone(), Conversation { strip, place: None, files: None, views: HashMap::new() });
        }
        self.conversations.get_mut(&self.key).expect("made above")
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

    /// The conversation in view, and its folder (none: a session with no folder, or none open).
    pub fn set_conversation(&mut self, key: Option<String>, place: Option<Place>, cx: &mut Context<Self>) {
        if self.key == key && self.place() == place.as_ref() {
            return;
        }
        self.key = key;
        self.conversation(cx).place = place;
        self.tell(cx);
        cx.notify();
    }

    /// Whether it's on screen: what's hidden asks the box nothing.
    pub fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        if self.shown == shown {
            return;
        }
        self.shown = shown;
        self.tell(cx);
        cx.notify();
    }

    /// Shows `kind` in the conversation in view: its tab, or a new one.
    pub fn open(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        self.conversation(cx);
        let next = &mut self.next;
        self.conversations.get_mut(&self.key).expect("made above").strip.open(kind, next);
        self.after_move(window, cx);
    }

    /// Puts `kind` in the showing tab's place (a tile on the New tab page, a file picked in Files).
    pub fn replace_active(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        let next = &mut self.next;
        let conversation = self.conversations.get_mut(&self.key).expect("a strip shows");
        let was = conversation.strip.active().id;
        conversation.strip.replace_active(kind, next);
        if conversation.strip.active().id != was {
            conversation.views.remove(&was);
        }
        self.after_move(window, cx);
    }

    /// Opens the New tab page.
    pub fn open_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = &mut self.next;
        self.conversations.get_mut(&self.key).expect("a strip shows").strip.open_new(next);
        self.after_move(window, cx);
    }

    /// Shows the tab numbered `id`.
    pub fn select(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.conversations.get_mut(&self.key).is_some_and(|conversation| conversation.strip.select(id)) {
            self.after_move(window, cx);
        }
    }

    /// Closes the tab numbered `id`, and what it had open.
    pub fn close(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let next = &mut self.next;
        let Some(conversation) = self.conversations.get_mut(&self.key) else { return };
        if conversation.strip.close(id, next).is_some() {
            conversation.views.remove(&id);
            if !conversation.strip.tabs().iter().any(|tab| matches!(tab.kind, Kind::Files | Kind::File(_))) {
                conversation.files = None;
            }
            self.after_move(window, cx);
        }
    }

    /// The showing tab moved: its view made where it's new, each told whether it's on screen, and
    /// what it needs read.
    fn after_move(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_view(cx);
        self.tell(cx);
        // A file tab shows its file in the folder's files (one viewer for now).
        if let Some(conversation) = self.current()
            && let Kind::File(path) = &conversation.strip.active().kind
            && let Some(files) = conversation.files.clone()
        {
            let path = path.clone();
            if files.read(cx).opened().map(|(at, _)| at) != Some(&path) {
                files.update(cx, |files, cx| files.open_file(&path, window, cx));
            }
        } else if let Some(conversation) = self.current()
            && conversation.strip.active().kind == Kind::Files
            && let Some(files) = conversation.files.clone()
            && files.read(cx).opened().is_some()
        {
            files.update(cx, |files, cx| files.close_file(window, cx));
        }
        if self.current().is_some_and(|conversation| conversation.strip.active().kind == Kind::New) {
            window.focus(&self.search.focus_handle(cx), cx);
        }
        cx.notify();
    }

    /// Makes the showing tab's view, where it has none yet.
    fn ensure_view(&mut self, cx: &mut Context<Self>) {
        let (client, live) = (self.client.clone(), self.live);
        let Some(conversation) = self.conversations.get_mut(&self.key) else { return };
        let tab = conversation.strip.active().clone();
        match tab.kind {
            Kind::Files | Kind::File(_) if conversation.files.is_none() => {
                conversation.files = Some(cx.new(|cx| FilesView::new(client, live, cx)));
            }
            Kind::Changes if !conversation.views.contains_key(&tab.id) => {
                conversation.views.insert(tab.id, View::Changes(cx.new(|cx| ChangesView::new(client, live, cx))));
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

    fn searched(&mut self, _event: &kuvo_ui::text_field::TextFieldEvent, cx: &mut Context<Self>) {
        cx.notify();
    }

    /// A tab's title: the file's name, the command, or what the tool's called.
    fn title(&self, tab: &Tab) -> SharedString {
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
    fn strip_bar(&self, strip: &Strip, cx: &mut Context<Self>) -> AnyElement {
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
                .on_click(cx.listener(move |this, _, window, cx| this.select(id, window, cx)))
                .on_mouse_down(gpui::MouseButton::Middle, cx.listener(move |this, _, window, cx| {
                    if closes {
                        this.close(id, window, cx);
                    }
                }))
                .child(Icon::new(Self::icon(&tab.kind)).size(IconSize::XSmall).color(if selected { c.icon_on } else { c.text_faint }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_ui(TypeSize::Caption)
                        .when(selected, |d| d.medium())
                        .text_color(if selected { c.text } else { c.text_muted })
                        .child(self.title(tab)),
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
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close(id, window, cx);
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
                    .when(!fresh, |d| d.child(button("panel-new-tab", IconName::Plus, IconSize::Nav, "New Tab").on_click(cx.listener(|this, _, window, cx| this.open_new(window, cx))))),
            )
            .child(button("panel-close", IconName::Close, IconSize::XSmall, "Close Panel").on_click(cx.listener(|_, _, _, cx| cx.emit(PanelEvent::Close))))
            .into_any_element()
    }

    fn body(&self, conversation: &Conversation, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let tab = conversation.strip.active();
        match (&tab.kind, conversation.views.get(&tab.id)) {
            (Kind::New, _) => new_tab::render(self, conversation.place.as_ref(), window, cx),
            (Kind::Files | Kind::File(_), _) => match (&conversation.place, &conversation.files) {
                (Some(_), Some(files)) => files.clone().into_any_element(),
                _ => no_folder(cx),
            },
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
        let c = cx.theme().colors.clone();
        let conversation = self.conversations.remove(&self.key).expect("made above");
        let strip = self.strip_bar(&conversation.strip, cx);
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
