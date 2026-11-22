//! The Library (#423): what was handed over to you across your computers — files an agent showed
//! you as a result, pictures it generated, files you attached — in one gallery. The box decides what
//! counts, keeps the list and filters it (`kuvo/library`); this view draws what it returns, by
//! source (the sidebar: All, each bot, Your sessions) and kind (the tabs), as a grid or a list, and
//! sends what you do: open, show in conversation, remove from the Library.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, App, Bounds, Context, Entity, EventEmitter, Hsla, Image, IntoElement, ObjectFit, Pixels, Render,
    SharedString, Subscription, Task, UniformListScrollHandle, WeakEntity, Window, canvas, div, hsla, img, point,
    prelude::*, px, size, uniform_list,
};
use kuvo_box::areas::Look;
use kuvo_box::follow::Followed;
use kuvo_box::library::{Library, LibraryItem, LibraryKind, LibraryQuery, PartState};
use kuvo_box::{BoxClient, Follow, Timing};
use kuvo_ui::menu::{ContextMenuArea, Menu, MenuEntry, PopoverMenu, PopoverMenuHandle};
use kuvo_ui::text_field::{TextField, TextFieldEvent};
use kuvo_ui::theme::{radius, space};
use kuvo_ui::{ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TextRole, TypeSize};

use crate::areas::{Reachability, Reading};

/// The sidebar's sources: everything, one bot's, or your own sessions'.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    All,
    Bot(String),
    Yours,
}

impl Source {
    fn query(&self) -> Option<String> {
        match self {
            Source::All => None,
            Source::Bot(id) => Some(id.clone()),
            Source::Yours => Some("you".into()),
        }
    }
}

/// The type tabs along the top.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    All,
    Images,
    Documents,
    Pages,
    Other,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::All, Tab::Images, Tab::Documents, Tab::Pages, Tab::Other];

    pub fn label(self) -> &'static str {
        match self {
            Tab::All => "All",
            Tab::Images => "Images",
            Tab::Documents => "Documents",
            Tab::Pages => "Pages",
            Tab::Other => "Other",
        }
    }

    fn kind(self) -> Option<LibraryKind> {
        match self {
            Tab::All => None,
            Tab::Images => Some(LibraryKind::Image),
            Tab::Documents => Some(LibraryKind::Document),
            Tab::Pages => Some(LibraryKind::Page),
            Tab::Other => Some(LibraryKind::Other),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Grid,
    List,
}

/// A bot as the sidebar shows it: its name and look, from the Bots list.
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryBot {
    pub id: String,
    pub name: String,
    pub look: Look,
}

/// "Show in conversation": the conversation an item was handed over in, at its message, on the
/// computer it's on (none: this window's).
#[derive(Clone, Debug, PartialEq)]
pub struct ShowAt {
    pub session: String,
    pub message: Option<String>,
    pub computer: Option<String>,
}

impl EventEmitter<ShowAt> for LibraryView {}

/// A document or page to open in the side panel, in a tab of its own, beside the conversation it
/// was handed over in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenInPanel {
    pub path: String,
    pub session: Option<String>,
}

impl EventEmitter<OpenInPanel> for LibraryView {}

/// A tile's picture: being read, read, or none to be had (its kind is drawn).
#[derive(Clone)]
enum Thumb {
    Reading,
    Ready(Arc<Image>),
    None,
}

/// What the menu does to an item.
#[derive(Clone, Debug)]
enum Act {
    Open,
    Show,
    Remove,
}

/// The longest side a tile's picture is kept at: two of a grid tile's width, for a Retina screen.
const TILE_SIDE: u32 = 480;
/// A grid tile's least width, and the gap between tiles: how many fit a row.
const TILE_MIN: f32 = 196.;
const GRID_GAP: f32 = 16.;
/// A grid row's height: the picture, its name and where it came from, and the gap below.
const THUMB_H: f32 = 136.;
const GRID_ROW: f32 = THUMB_H + 7. + 20. + 2. + 18. + 22.;
const LIST_ROW: f32 = 54.;
/// The main area's padding beside the grid or the list.
const PAD_X: f32 = 28.;

pub struct LibraryView {
    client: BoxClient,
    /// Followed as it changes (a window's own box); else read once each time (a test's).
    live: bool,
    reading: Option<Task<()>>,
    visible: bool,
    source: Source,
    tab: Tab,
    mode: Mode,
    search: Entity<TextField>,
    words: String,
    library: Reading<Library>,
    reachability: Reachability,
    follow: Option<(Follow<Followed<Library>>, Task<()>)>,
    typing: Option<Task<()>>,
    bots: Vec<LibraryBot>,
    thumbs: HashMap<String, (f64, Thumb)>,
    /// Removed here, before the box says so (put back if it refuses).
    removed: HashSet<String>,
    /// What went wrong with the last thing you did, until the next.
    problem: Option<String>,
    menus: HashMap<String, PopoverMenuHandle>,
    scroll: UniformListScrollHandle,
    /// The main area's width as last drawn: how many tiles fit a row.
    width: Rc<std::cell::Cell<f32>>,
    /// The time "14m ago" counts from (a stand-in's own clock; none: now).
    now: Option<f64>,
    /// The item whose menu a shot shows open, once it's drawn.
    #[cfg(feature = "stand-in")]
    menu_open: Option<String>,
    _search: Subscription,
}

impl LibraryView {
    pub fn new(client: BoxClient, live: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextField::new(cx).bare().placeholder("Search"));
        let subscription = cx.subscribe_in(&search, window, |this, field, event: &TextFieldEvent, _, cx| {
            if matches!(event, TextFieldEvent::Changed) {
                let words = field.read(cx).text().to_owned();
                this.typed(words, cx);
            }
        });
        Self {
            client,
            live,
            reading: None,
            visible: false,
            source: Source::All,
            tab: Tab::All,
            mode: Mode::Grid,
            search,
            words: String::new(),
            library: Reading::NotYet,
            reachability: Reachability::Unanswered,
            follow: None,
            typing: None,
            bots: Vec::new(),
            thumbs: HashMap::new(),
            removed: HashSet::new(),
            problem: None,
            menus: HashMap::new(),
            scroll: UniformListScrollHandle::new(),
            width: Rc::new(std::cell::Cell::new(960.)),
            now: None,
            #[cfg(feature = "stand-in")]
            menu_open: None,
            _search: subscription,
        }
    }

    pub(crate) fn reachability(&self) -> Reachability {
        self.reachability
    }

    /// Shown or not: followed only while it's shown (another computer's part is asked by mail).
    pub fn shown(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if visible {
            self.follow_now(cx);
        } else {
            self.follow = None;
        }
    }

    pub fn read_again(&mut self, cx: &mut Context<Self>) {
        if self.visible {
            self.follow_now(cx);
        }
    }

    /// The bots the sidebar lists, as the Bots list has them (this computer's and your others').
    pub fn set_bots(&mut self, bots: Vec<LibraryBot>, cx: &mut Context<Self>) {
        if self.bots != bots {
            self.bots = bots;
            cx.notify();
        }
    }

    /// The window's title: "Library", or the source picked beside it ("Foreman", "Library").
    pub fn title(&self) -> (SharedString, Option<SharedString>) {
        match &self.source {
            Source::All => ("Library".into(), None),
            Source::Yours => ("Your sessions".into(), Some("Library".into())),
            Source::Bot(id) => (self.bot_name(id).into(), Some("Library".into())),
        }
    }

    fn query(&self) -> LibraryQuery {
        LibraryQuery { source: self.source.query(), kind: self.tab.kind(), words: self.words.clone() }
    }

    fn follow_now(&mut self, cx: &mut Context<Self>) {
        if !self.client.gate().has("library") {
            self.follow = None;
            return;
        }
        if !self.live {
            let client = self.client.clone();
            let query = self.query();
            let read = cx.background_executor().spawn(async move { client.library(&query) });
            self.reading = Some(cx.spawn(async move |this, cx| {
                let result = read.await;
                this.update(cx, |this, cx| this.take(result.map_or_else(Followed::Failed, Followed::Now), cx)).ok();
            }));
            return;
        }
        let follow = self.client.follow_library(&self.query(), Timing::default());
        let updates = follow.updates().clone();
        let task = cx.spawn(async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                if this.update(cx, |this, cx| this.take(update, cx)).is_err() {
                    break;
                }
            }
        });
        self.follow = Some((follow, task));
    }

    fn take(&mut self, update: Followed<Library>, cx: &mut Context<Self>) {
        match update {
            Followed::Now(library) => {
                self.reachability = Reachability::Reached;
                // (What the box no longer lists needn't stay removed here.)
                self.removed.retain(|id| library.items.iter().any(|item| &item.id == id));
                self.library = Reading::Ready(library);
            }
            Followed::Failed(error) => {
                self.reachability = Reachability::Unreachable;
                // (What it said last stays while it can't say more.)
                if !matches!(self.library, Reading::Ready(_)) {
                    self.library = Reading::Failed(kuvo_box::accounts::said(&error));
                }
            }
        }
        cx.notify();
    }

    fn typed(&mut self, words: String, cx: &mut Context<Self>) {
        if words == self.words {
            return;
        }
        self.words = words;
        // (Asked once typing pauses: another computer's part is asked by mail.)
        self.typing = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            this.update(cx, |this, cx| this.changed(cx)).ok();
        }));
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.problem = None;
        self.scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
        if self.visible {
            self.follow_now(cx);
        }
        cx.notify();
    }

    fn pick_source(&mut self, source: Source, cx: &mut Context<Self>) {
        if self.source != source {
            self.source = source;
            self.changed(cx);
        }
    }

    fn pick_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.tab = tab;
            self.changed(cx);
        }
    }

    fn pick_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if self.mode != mode {
            self.mode = mode;
            cx.notify();
        }
    }

    /// The items to draw: the box's, but those removed here.
    fn items(&self) -> Vec<LibraryItem> {
        match &self.library {
            Reading::Ready(library) => library.items.iter().filter(|item| !self.removed.contains(&item.id)).cloned().collect(),
            _ => Vec::new(),
        }
    }

    /// A computer is named beside each item only when there's more than one.
    fn several_computers(&self) -> bool {
        matches!(&self.library, Reading::Ready(library) if library.computers.len() > 1)
    }

    fn bot_name(&self, id: &str) -> String {
        if let Some(bot) = self.bots.iter().find(|bot| bot.id == id) {
            return bot.name.clone();
        }
        if let Reading::Ready(library) = &self.library
            && let Some(name) = library.sources.bots.iter().find(|bot| bot.id == id).and_then(|bot| bot.name.clone())
        {
            return name;
        }
        "A bot".into()
    }

    /// The sidebar's bots: the Bots list's, then any other the Library names.
    fn sidebar_bots(&self) -> Vec<LibraryBot> {
        let mut bots = self.bots.clone();
        if let Reading::Ready(library) = &self.library {
            for bot in &library.sources.bots {
                if !bots.iter().any(|each| each.id == bot.id) {
                    bots.push(LibraryBot { id: bot.id.clone(), name: bot.name.clone().unwrap_or_else(|| "A bot".into()), look: bot.look.clone().unwrap_or_else(crate::areas::unknown_bot_look) });
                }
            }
        }
        bots
    }

    fn look_of(&self, id: &str) -> Look {
        if let Some(bot) = self.bots.iter().find(|bot| bot.id == id) {
            return bot.look.clone();
        }
        match &self.library {
            Reading::Ready(library) => library.sources.bots.iter().find(|bot| bot.id == id).and_then(|bot| bot.look.clone()),
            _ => None,
        }
        .unwrap_or_else(crate::areas::unknown_bot_look)
    }

    fn now_ms(&self) -> f64 {
        self.now.unwrap_or_else(crate::time::now_ms)
    }

    // ---- What you do ----

    fn act(&mut self, act: Act, item: &LibraryItem, window: &mut Window, cx: &mut Context<Self>) {
        self.problem = None;
        match act {
            Act::Open => self.open(item, window, cx),
            Act::Show => {
                if let Some(session) = &item.session {
                    cx.emit(ShowAt {
                        session: session.clone(),
                        message: item.message.clone(),
                        computer: item.computer.as_ref().filter(|computer| !computer.here).map(|computer| computer.key.clone()),
                    });
                }
            }
            Act::Remove => self.remove(item, cx),
        }
        cx.notify();
    }

    /// A picture in the viewer, with the others in view to go through; anything else in the app
    /// that opens it on this Mac.
    fn open(&mut self, item: &LibraryItem, window: &mut Window, cx: &mut Context<Self>) {
        if !item.there {
            self.problem = Some(format!("{} is no longer there to open.", item.name));
            return;
        }
        if item.kind == LibraryKind::Image {
            let images: Vec<LibraryItem> = self.items().into_iter().filter(|each| each.kind == LibraryKind::Image && each.there).collect();
            let active = images.iter().position(|each| each.id == item.id).unwrap_or(0);
            let here = self.client.here();
            let items = images
                .into_iter()
                .map(|image| {
                    let client = self.client.clone();
                    let local_path = (here && image.is_here()).then(|| std::path::PathBuf::from(&image.path)).filter(|path| path.is_file());
                    let source_tag: SharedString = image.computer.as_ref().filter(|_| self.several_computers()).map(|computer| computer.name.clone()).unwrap_or_else(|| "Library".into()).into();
                    let (id, name) = (image.id.clone(), image.name.clone());
                    let loader: Arc<dyn Fn() -> crate::pictures::Read + Send + Sync> = Arc::new(move || match client.library_file(&image) {
                        Ok(bytes) => crate::pictures::shown(bytes),
                        Err(kuvo_box::Error::NotFound) => crate::pictures::Read::Missing,
                        Err(_) => crate::pictures::Read::Failed { why: "Couldn’t load this image.".into(), again: true },
                    });
                    crate::image_viewer::ViewerItem { id, name, caption: None, source_tag, local_path, read: crate::pictures::Read::Reading, loader: Some(loader) }
                })
                .collect();
            crate::image_viewer::open(crate::image_viewer::OpenRequest { items, active, return_focus: None, on_change: None }, window, cx);
            return;
        }
        // A document or page on the window's computer: in the side panel, where its text shows, or
        // Open hands it to its app on this Mac.
        if matches!(item.kind, LibraryKind::Document | LibraryKind::Page) && item.is_here() && (self.client.here() || item.media_type.starts_with("text/")) {
            cx.emit(OpenInPanel { path: item.path.clone(), session: item.session.clone() });
            return;
        }
        let client = self.client.clone();
        let item = item.clone();
        let opening = cx.background_executor().spawn(async move { crate::open_shown::open_library_item(&client, &item).map_err(|error| (item.name.clone(), error.to_string())) });
        cx.spawn(async move |this, cx| {
            if let Err((name, why)) = opening.await {
                this.update(cx, |this, cx| {
                    this.problem = Some(format!("{name} couldn’t be opened: {why}"));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn remove(&mut self, item: &LibraryItem, cx: &mut Context<Self>) {
        self.removed.insert(item.id.clone());
        let client = self.client.clone();
        let item = item.clone();
        let hiding = cx.background_executor().spawn({
            let item = item.clone();
            async move { client.hide_library_item(&item, true) }
        });
        cx.spawn(async move |this, cx| {
            let hidden = hiding.await;
            this.update(cx, |this, cx| {
                match hidden {
                    Err(error) => {
                        this.removed.remove(&item.id);
                        this.problem = Some(format!("{} couldn’t be removed: {}", item.name, kuvo_box::accounts::said(&error)));
                    }
                    // (Followed, the box says so by itself.)
                    Ok(()) if !this.live && this.visible => this.follow_now(cx),
                    Ok(()) => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---- Thumbnails ----

    fn thumb(&mut self, item: &LibraryItem, cx: &mut Context<Self>) -> Thumb {
        if let Some((at, thumb)) = self.thumbs.get(&item.id)
            && *at == item.at
        {
            return thumb.clone();
        }
        if !item.thumbnail {
            self.thumbs.insert(item.id.clone(), (item.at, Thumb::None));
            return Thumb::None;
        }
        self.thumbs.insert(item.id.clone(), (item.at, Thumb::Reading));
        let client = self.client.clone();
        let wanted = item.clone();
        let read = cx.background_executor().spawn(async move { client.library_thumbnail(&wanted).ok().and_then(|bytes| crate::pictures::small(bytes, TILE_SIDE)) });
        let (id, at) = (item.id.clone(), item.at);
        cx.spawn(async move |this, cx| {
            let image = read.await;
            this.update(cx, |this, cx| {
                if this.thumbs.get(&id).is_some_and(|(was, _)| *was == at) {
                    this.thumbs.insert(id, (at, image.map_or(Thumb::None, Thumb::Ready)));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        Thumb::Reading
    }

    fn menu_handle(&mut self, id: &str) -> PopoverMenuHandle {
        self.menus.entry(id.to_owned()).or_default().clone()
    }

    /// The menu for an item: Open, Show in conversation (where it has one), Remove from Library.
    fn menu_builder(&self, item: &LibraryItem, cx: &mut Context<Self>) -> impl Fn(&mut Window, &mut App) -> Entity<Menu> + 'static {
        let view = cx.entity().downgrade();
        let item = item.clone();
        move |_, cx: &mut App| {
            let act = |view: &WeakEntity<LibraryView>, act: Act, item: &LibraryItem| {
                let (view, item) = (view.clone(), item.clone());
                move |window: &mut Window, cx: &mut App| {
                    view.update(cx, |this, cx| this.act(act.clone(), &item, window, cx)).ok();
                }
            };
            let (view, item) = (view.clone(), item.clone());
            cx.new(move |cx| {
                let mut menu = Menu::new(cx).width(px(208.)).entry(MenuEntry::new("Open").icon(IconName::External).on_select(act(&view, Act::Open, &item)));
                if item.session.is_some() {
                    menu = menu.entry(MenuEntry::new("Show in conversation").icon(IconName::Message).on_select(act(&view, Act::Show, &item)));
                }
                menu.separator().entry(MenuEntry::new("Remove from Library").icon(IconName::EyeOff).muted(true).on_select(act(&view, Act::Remove, &item)))
            })
        }
    }

    // ---- The sidebar ----

    /// The sidebar the workspace shows beside the rail while the Library is open: search, All, each
    /// bot, Your sessions.
    pub fn sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let dark = cx.theme().appearance.is_dark();
        let search = div()
            .debug_selector(|| "library-search".into())
            .h(px(30.))
            .px(px(9.))
            .rounded(radius::NAV)
            .bg(if dark { hsla(0., 0., 1., 0.04) } else { hsla(0., 0., 1., 0.82) })
            .border_1()
            .border_color(if dark { hsla(0., 0., 1., 0.08) } else { hsla(0., 0., 0., 0.07) })
            .flex()
            .items_center()
            .gap(space::S1_5)
            .child(Icon::new(IconName::Search).size(IconSize::Nav).color(c.text_faint))
            .child(div().flex_1().min_w_0().child(self.search.clone()));

        let row = |this: &mut Self, id: SharedString, leading: AnyElement, label: String, source: Source, cx: &mut Context<Self>| {
            let selected = this.source == source;
            div()
                .id(id.clone())
                .debug_selector(move || id.to_string())
                .h(px(32.))
                .px(px(8.))
                .rounded(radius::NAV)
                .cursor_pointer()
                .flex()
                .items_center()
                .gap(space::S2)
                .when(selected, |d| {
                    d.bg(if dark { hsla(0., 0., 1., 0.105) } else { c.selected_card })
                        .border_1()
                        .border_color(if dark { hsla(0., 0., 1., 0.14) } else { c.selected_edge })
                        .shadow(cx.theme().selected_shadow())
                })
                .when(!selected, |d| d.hover(|s| s.bg(c.hover)))
                .on_click(cx.listener(move |this, _, _, cx| this.pick_source(source.clone(), cx)))
                .child(leading)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_role(TextRole::RowLabel)
                        .when(selected, |d| d.medium().text_color(c.text))
                        .when(!selected, |d| d.text_color(c.text_muted))
                        .child(label),
                )
                .into_any_element()
        };
        let icon_box = |icon: AnyElement| div().size(px(20.)).flex_none().flex().items_center().justify_center().child(icon).into_any_element();
        let all_on = self.source == Source::All;
        let all = row(self, "library-source-all".into(), icon_box(Icon::new(IconName::Library).size(IconSize::Nav).color(if all_on { c.icon_on } else { c.icon }).into_any_element()), "All".into(), Source::All, cx);
        let bots: Vec<AnyElement> = self
            .sidebar_bots()
            .into_iter()
            .map(|bot| {
                let avatar = crate::bot_face::still_avatar(&bot.id, &bot.look, kuvo_ui::bot::State::Idle, px(20.));
                row(self, format!("library-source-{}", bot.id).into(), icon_box(avatar), bot.name.clone(), Source::Bot(bot.id.clone()), cx)
            })
            .collect();
        let yours_on = self.source == Source::Yours;
        let yours = row(self, "library-source-yours".into(), icon_box(Icon::new(IconName::User).size(IconSize::Nav).color(if yours_on { c.icon_on } else { c.icon }).into_any_element()), "Your sessions".into(), Source::Yours, cx);
        let rule = || div().h(px(1.)).mx(space::S2).my(space::S1_5).bg(c.border);
        div()
            .id("library-sidebar")
            .size_full()
            .px(px(10.))
            .py(space::S3)
            .flex()
            .flex_col()
            .gap(space::S3)
            .child(search)
            .child(
                div()
                    .id("library-sources")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(all)
                    .when(!bots.is_empty(), |d| d.child(rule()).children(bots))
                    .child(rule())
                    .child(yours),
            )
            .into_any_element()
    }

    // ---- The main area ----

    fn top_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let dark = cx.theme().appearance.is_dark();
        let tabs = div().flex().items_center().gap(px(20.)).children(Tab::ALL.into_iter().map(|tab| {
            let active = self.tab == tab;
            div()
                .id(SharedString::from(format!("library-tab-{}", tab.label())))
                .debug_selector(move || format!("library-tab-{}", tab.label().to_lowercase()))
                .cursor_pointer()
                .h(px(36.))
                .flex()
                .items_center()
                .border_b_2()
                .border_color(if active { c.text } else { gpui::transparent_black() })
                .text_ui(TypeSize::Small)
                .when(active, |d| d.medium().text_color(c.text))
                .when(!active, |d| d.text_color(c.text_muted).hover(|s| s.text_color(c.text)))
                .on_click(cx.listener(move |this, _, _, cx| this.pick_tab(tab, cx)))
                .child(tab.label())
        }));
        let toggle = |mode: Mode, icon: IconName, label: &'static str, cx: &mut Context<Self>| {
            let active = self.mode == mode;
            div()
                .id(label)
                .debug_selector(move || format!("library-{}", if mode == Mode::Grid { "grid" } else { "list" }))
                .cursor_pointer()
                .w(px(28.))
                .h(px(24.))
                .rounded(px(6.))
                .flex()
                .items_center()
                .justify_center()
                .when(active, |d| {
                    d.bg(if dark { hsla(0., 0., 1., 0.105) } else { hsla(0., 0., 1., 0.96) })
                        .border_1()
                        .border_color(if dark { hsla(0., 0., 1., 0.14) } else { hsla(0., 0., 0., 0.075) })
                        .shadow(cx.theme().card_shadow())
                })
                .when(!active, |d| d.hover(|s| s.bg(c.hover)))
                .tooltip(move |_, cx| kuvo_ui::menu::Tooltip::new(label).build(cx))
                .on_click(cx.listener(move |this, _, _, cx| this.pick_mode(mode, cx)))
                .child(Icon::new(icon).size(IconSize::Nav).color(if active { c.text } else { c.icon }))
        };
        let track = div()
            .rounded(radius::NAV)
            .bg(if dark { hsla(0., 0., 1., 0.035) } else { hsla(0., 0., 0., 0.038) })
            .border_1()
            .border_color(if dark { hsla(0., 0., 1., 0.07) } else { hsla(0., 0., 0., 0.055) })
            .p(px(2.))
            .flex()
            .items_center()
            .gap(px(2.))
            .child(toggle(Mode::Grid, IconName::Grid, "Grid", cx))
            .child(toggle(Mode::List, IconName::List, "List", cx));
        div()
            .flex_none()
            .px(px(PAD_X))
            .pt(px(10.))
            .border_b_1()
            .border_color(c.border)
            .flex()
            .items_center()
            .justify_between()
            .child(tabs)
            .child(div().pb(px(6.)).child(track))
            .into_any_element()
    }

    /// A quiet line under the tabs: what you did that didn't work, a computer whose part isn't
    /// shown, or that only the newest are listed.
    fn notes(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = cx.theme().colors.clone();
        let mut lines: Vec<(String, bool)> = Vec::new();
        if let Some(problem) = &self.problem {
            lines.push((problem.clone(), true));
        }
        if let Reading::Ready(library) = &self.library {
            for computer in &library.computers {
                if matches!(computer.state, PartState::Away | PartState::Older | PartState::Failed)
                    && let Some(message) = &computer.message
                {
                    lines.push((message.clone(), false));
                }
            }
        }
        if lines.is_empty() {
            return None;
        }
        Some(
            div()
                .debug_selector(|| "library-notes".into())
                .flex_none()
                .px(px(PAD_X))
                .pt(px(12.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .children(lines.into_iter().map(|(line, problem)| {
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S1_5)
                        .text_ui(TypeSize::Caption)
                        .text_color(if problem { c.text } else { c.text_muted })
                        .child(Icon::new(if problem { IconName::Warning } else { IconName::Info }).size(IconSize::XSmall).color(if problem { c.text_muted } else { c.text_faint }))
                        .child(div().min_w_0().truncate().child(line))
                }))
                .into_any_element(),
        )
    }

    fn empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let words = self.words.trim();
        let (icon, line, again): (IconName, String, bool) = match &self.library {
            Reading::NotYet => (IconName::Library, "Loading…".into(), false),
            Reading::Failed(why) => (IconName::Warning, why.clone(), true),
            Reading::Ready(_) if !words.is_empty() => (IconName::Search, format!("Nothing matches “{words}”."), false),
            Reading::Ready(_) if self.source != Source::All || self.tab != Tab::All => (IconName::Library, "Nothing here yet.".into(), false),
            Reading::Ready(_) => (IconName::Library, "Files you attach and results agents hand over appear here.".into(), false),
        };
        div()
            .debug_selector(|| "library-empty".into())
            .flex_1()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .pb(px(72.))
            .gap(space::S3)
            .child(Icon::new(icon).size(IconSize::Card).color(c.text_faint))
            .child(div().max_w(px(420.)).text_center().text_ui(TypeSize::Small).text_color(c.text_muted).child(line))
            .when(again, |d| d.child(Button::new("library-try-again", "Try Again").size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.read_again(cx)))))
            .into_any_element()
    }

    /// How many tiles fit a row at the main area's width.
    fn columns(&self) -> usize {
        columns_for(self.width.get())
    }

    fn meta_line(&self, item: &LibraryItem, list: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let one_bot = matches!(self.source, Source::Bot(_));
        let mut meta = div().flex().items_center().gap(px(5.)).min_w_0().text_ui(TypeSize::Caption).text_color(c.text_faint);
        if !one_bot && !list {
            meta = meta.child(self.badge(item, 13.)).child(div().flex_none().text_color(c.text_muted).child(self.source_word(item))).child("·");
        }
        meta = meta.child(div().flex_none().child(when_words(item.at * 1000., self.now_ms(), false)));
        if self.several_computers()
            && let Some(computer) = &item.computer
        {
            meta = meta.child("·").child(div().min_w_0().truncate().child(computer.name.clone()));
        }
        meta.into_any_element()
    }

    fn source_word(&self, item: &LibraryItem) -> String {
        match &item.bot {
            Some(bot) => self.bot_name(bot),
            None => "You".into(),
        }
    }

    fn badge(&self, item: &LibraryItem, side: f32) -> AnyElement {
        match &item.bot {
            Some(bot) => crate::bot_face::still_avatar(bot, &self.look_of(bot), kuvo_ui::bot::State::Idle, px(side)),
            None => div().size(px(side)).flex_none().flex().items_center().justify_center().child(Icon::new(IconName::User).size(IconSize::XSmall)).into_any_element(),
        }
    }

    fn picture(&mut self, item: &LibraryItem, cx: &mut Context<Self>) -> AnyElement {
        let dark = cx.theme().appearance.is_dark();
        let thumb = self.thumb(item, cx);
        let hue = hue_of(&item.id);
        match thumb {
            Thumb::Ready(image) => {
                let fit = if item.kind == LibraryKind::Image { ObjectFit::Cover } else { ObjectFit::Contain };
                let stage = stage_color(item.kind, hue, dark);
                div()
                    .size_full()
                    .bg(stage)
                    .when(item.kind != LibraryKind::Image, |d| d.p(px(8.)))
                    .child(img(image).size_full().object_fit(fit))
                    .into_any_element()
            }
            Thumb::Reading => div().size_full().bg(stage_color(item.kind, hue, dark)).into_any_element(),
            Thumb::None => {
                let kind = item.kind;
                canvas(|_, _, _| {}, move |bounds, (), window, _| paint_kind(bounds, kind, hue, dark, window)).size_full().into_any_element()
            }
        }
    }

    fn tile(&mut self, item: &LibraryItem, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let dark = cx.theme().appearance.is_dark();
        let handle = self.menu_handle(&item.id);
        let open = handle.is_open();
        let build = self.menu_builder(item, cx);
        let more = PopoverMenu::new(
            SharedString::from(format!("library-more-{}", item.id)),
            div()
                .id(SharedString::from(format!("library-more-button-{}", item.id)))
                .size(px(24.))
                .rounded(radius::SMALL)
                .bg(if dark { hsla(0., 0., 0.12, 0.86) } else { hsla(0., 0., 1., 0.92) })
                .border_1()
                .border_color(if dark { hsla(0., 0., 1., 0.14) } else { hsla(0., 0., 0., 0.08) })
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .child(Icon::new(IconName::More).size(IconSize::Nav).color(c.text)),
            build,
        )
        .with_handle(handle)
        .align_right();
        let group = SharedString::from(format!("library-tile-{}", item.id));
        let frame = div()
            .relative()
            .w_full()
            .h(px(THUMB_H))
            .rounded(px(10.))
            .overflow_hidden()
            .border_1()
            .border_color(if open { c.accent } else if dark { hsla(0., 0., 1., 0.08) } else { hsla(0., 0., 0., 0.07) })
            .when(!item.there, |d| d.opacity(0.55))
            .child(self.picture(item, cx))
            .child(div().absolute().top(px(8.)).right(px(8.)).when(!open, |d| d.invisible().group_hover(group.clone(), |s| s.visible())).child(more));
        let item_for_click = item.clone();
        let body = div()
            .id(group.clone())
            .group(group.clone())
            .debug_selector({
                let id = item.id.clone();
                move || format!("library-item-{id}")
            })
            .relative()
            .w_full()
            .cursor_pointer()
            .flex()
            .flex_col()
            .gap(px(7.))
            .on_click(cx.listener(move |this, _, window, cx| this.act(Act::Open, &item_for_click, window, cx)))
            .child(frame)
            .child(
                div()
                    .px(px(2.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().min_w_0().truncate().text_ui(TypeSize::Small).medium().text_color(c.text).child(item.name.clone()))
                    .child(self.meta_line(item, false, cx)),
            );
        let build = self.menu_builder(item, cx);
        ContextMenuArea::new(SharedString::from(format!("library-menu-{}", item.id)), body, build).into_any_element()
    }

    fn row(&mut self, item: &LibraryItem, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let dark = cx.theme().appearance.is_dark();
        let open = self.menus.get(&item.id).is_some_and(PopoverMenuHandle::is_open);
        let several = self.several_computers();
        let item_for_click = item.clone();
        let body = div()
            .id(SharedString::from(format!("library-row-{}", item.id)))
            .debug_selector({
                let id = item.id.clone();
                move || format!("library-item-{id}")
            })
            .h(px(LIST_ROW))
            .px(space::S2)
            .rounded(radius::NAV)
            .cursor_pointer()
            .border_b_1()
            .border_color(c.border.opacity(0.65))
            .when(open, |d| d.bg(c.hover))
            .hover(|s| s.bg(c.hover))
            .flex()
            .items_center()
            .gap(space::S3)
            .when(!item.there, |d| d.opacity(0.55))
            .on_click(cx.listener(move |this, _, window, cx| this.act(Act::Open, &item_for_click, window, cx)))
            .child(
                div()
                    .w(px(48.))
                    .h(px(34.))
                    .flex_none()
                    .rounded(px(6.))
                    .overflow_hidden()
                    .border_1()
                    .border_color(if dark { hsla(0., 0., 1., 0.08) } else { hsla(0., 0., 0., 0.07) })
                    .child(self.picture(item, cx)),
            )
            .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Small).medium().text_color(c.text).child(item.name.clone()))
            .child(
                div()
                    .w(px(124.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(self.badge(item, 16.))
                    .child(div().min_w_0().truncate().text_ui(TypeSize::Caption).text_color(c.text_muted).child(self.source_word(item))),
            )
            .when(several, |d| {
                d.child(div().w(px(92.)).flex_none().truncate().text_ui(TypeSize::Caption).text_color(c.text_faint).child(item.computer.as_ref().map(|computer| computer.name.clone()).unwrap_or_default()))
            })
            .child(div().w(px(74.)).flex_none().flex().justify_end().text_ui(TypeSize::Caption).text_color(c.text_faint).child(when_words(item.at * 1000., self.now_ms(), true)));
        let build = self.menu_builder(item, cx);
        ContextMenuArea::new(SharedString::from(format!("library-menu-{}", item.id)), body, build).into_any_element()
    }

    fn grid(&mut self, items: Rc<Vec<LibraryItem>>, cx: &mut Context<Self>) -> AnyElement {
        let columns = self.columns();
        let rows = items.len().div_ceil(columns);
        // Each tile as wide as the others, whatever its picture.
        let cell = ((self.width.get() - GRID_GAP * (columns - 1) as f32) / columns as f32).floor().max(TILE_MIN / 2.);
        uniform_list("library-grid", rows, cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
            range
                .map(|row| {
                    let start = row * columns;
                    let tiles: Vec<AnyElement> = (start..start + columns)
                        .map(|at| match items.get(at) {
                            Some(item) => div().w(px(cell)).flex_none().min_w_0().child(this.tile(item, cx)).into_any_element(),
                            None => div().w(px(cell)).flex_none().into_any_element(),
                        })
                        .collect();
                    div().id(("library-grid-row", row)).h(px(GRID_ROW)).flex().items_start().gap(px(GRID_GAP)).children(tiles).into_any_element()
                })
                .collect::<Vec<_>>()
        }))
        .track_scroll(&self.scroll)
        .size_full()
        .px(px(PAD_X))
        .pt(px(22.))
        .into_any_element()
    }

    fn list(&mut self, items: Rc<Vec<LibraryItem>>, cx: &mut Context<Self>) -> AnyElement {
        uniform_list("library-list", items.len(), cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
            range.filter_map(|at| items.get(at).map(|item| div().max_w(px(920.)).child(this.row(item, cx)).into_any_element())).collect::<Vec<_>>()
        }))
        .track_scroll(&self.scroll)
        .size_full()
        .px(px(PAD_X))
        .pt(px(22.))
        .into_any_element()
    }

    #[cfg(feature = "stand-in")]
    /// A shot's state: its clock, and what's picked (`library-<state>`).
    pub fn sample(&mut self, slug: &str, now: f64, window: &mut Window, cx: &mut Context<Self>) {
        self.now = Some(now);
        match slug {
            "library-list" => self.mode = Mode::List,
            "library-bot" => self.source = Source::Bot("bot_foreman".into()),
            "library-images" => self.tab = Tab::Images,
            _ => {}
        }
        if slug == "library-menu" {
            self.menu_open = Some("lib_02".into());
        }
        let _ = window;
        self.changed(cx);
    }

    #[cfg(feature = "stand-in")]
    /// The menu a shot shows open, once its tile is drawn.
    pub fn open_sample_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.menu_open.take() else { return };
        let Some(item) = self.items().into_iter().find(|item| item.id == id) else { return };
        let build = self.menu_builder(&item, cx);
        let menu = build(window, cx);
        self.menu_handle(&id).open(menu, window, cx);
        cx.notify();
    }
}

impl Render for LibraryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        let items = Rc::new(self.items());
        let width = self.width.clone();
        let entity = cx.entity().downgrade();
        let body = if items.is_empty() {
            self.empty(cx)
        } else {
            match self.mode {
                Mode::Grid => self.grid(items, cx),
                Mode::List => self.list(items, cx),
            }
        };
        let more = matches!(&self.library, Reading::Ready(library) if library.more);
        div()
            .debug_selector(|| "library".into())
            .size_full()
            .min_w_0()
            .bg(c.surface)
            .flex()
            .flex_col()
            .child(self.top_bar(cx))
            .children(self.notes(cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(body)
                    // The width the grid has: how many tiles fit a row (drawn again when it changes).
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                let inner = f32::from(bounds.size.width) - 2. * PAD_X;
                                if (width.get() - inner).abs() > 0.5 {
                                    width.set(inner);
                                    entity.update(cx, |_, cx| cx.notify()).ok();
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    ),
            )
            .when(more, |d| {
                d.child(
                    div()
                        .flex_none()
                        .px(px(PAD_X))
                        .py(px(8.))
                        .text_ui(TypeSize::Caption)
                        .text_color(c.text_faint)
                        .child("Showing the newest 1,000. Search to find older ones."),
                )
            })
    }
}

/// How many tiles fit a row `width` wide: two at least, six at most.
pub fn columns_for(width: f32) -> usize {
    (((width + GRID_GAP) / (TILE_MIN + GRID_GAP)).floor() as usize).clamp(2, 6)
}

/// When an item was handed over, as a tile says it ("14m", "2h", "1d", "Nov 8") or a list row
/// ("14m ago", "Yesterday", "Nov 8").
pub fn when_words(then: f64, now: f64, list: bool) -> String {
    let days = (now - then) / 86_400_000.;
    if days >= 7. {
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        let seconds = (then / 1000.) as i64;
        let (_, month, day) = crate::time::civil((seconds + crate::pasted::utc_offset(seconds)).div_euclid(86_400));
        return format!("{} {day}", MONTHS[(month as usize).clamp(1, 12) - 1]);
    }
    let short = crate::time::ago(then, now);
    if !list {
        return short;
    }
    match short.as_str() {
        "now" => "Just now".into(),
        "1d" => "Yesterday".into(),
        _ => format!("{short} ago"),
    }
}

/// A steady hue for an item, from its id: what its drawing is tinted with.
fn hue_of(id: &str) -> f32 {
    let sum = id.bytes().fold(7u32, |sum, byte| sum.wrapping_mul(31).wrapping_add(byte as u32));
    (sum % 360) as f32
}

/// The calm tinted stage a document's page or a page's preview sits on.
fn stage_color(kind: LibraryKind, hue: f32, dark: bool) -> Hsla {
    match kind {
        LibraryKind::Image => if dark { hsla(0., 0., 1., 0.04) } else { hsla(0., 0., 0., 0.03) },
        _ => kuvo_ui::bot::look::oklch(if dark { 0.21 } else { 0.94 }, 0.014, hue, 1.0),
    }
}

/// An item with no thumbnail, drawn as its kind: a page of a document, a page's window, a file.
fn paint_kind(bounds: Bounds<Pixels>, kind: LibraryKind, hue: f32, dark: bool, window: &mut Window) {
    let (x, y) = (bounds.left(), bounds.top());
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    if w <= 1. || h <= 1. {
        return;
    }
    let rect = |fx: f32, fy: f32, fw: f32, fh: f32| Bounds::new(point(x + px(fx * w), y + px(fy * h)), size(px(fw * w), px(fh * h)));
    let fill = |window: &mut Window, fx: f32, fy: f32, fw: f32, fh: f32, r: f32, color: Hsla| window.paint_quad(gpui::fill(rect(fx, fy, fw, fh), color).corner_radii(px(r)));
    let oklch = kuvo_ui::bot::look::oklch;
    window.paint_quad(gpui::fill(bounds, stage_color(if kind == LibraryKind::Image { LibraryKind::Other } else { kind }, hue, dark)));
    let ink_strong = hsla(0., 0., 0.16, 0.7);
    let ink_soft = hsla(0., 0., 0.16, 0.16);
    let accent = oklch(if dark { 0.66 } else { 0.56 }, 0.10, hue, 1.0);
    let line = if dark { hsla(0., 0., 1., 0.12) } else { hsla(0., 0., 0., 0.10) };
    match kind {
        LibraryKind::Document => {
            let paper: Hsla = if dark { hsla(60. / 360., 0.07, 0.95, 1.) } else { hsla(0., 0., 1., 1.) };
            fill(window, 0.22, 0.08, 0.56, 0.84, 3., paper);
            fill(window, 0.28, 0.17, 0.30, 0.055, 1.5, ink_strong);
            for i in 0..7 {
                let row = 0.28 + i as f32 * 0.082;
                fill(window, 0.28, row, if i % 3 == 2 { 0.32 } else { 0.44 }, 0.032, 1., ink_soft);
            }
        }
        LibraryKind::Page => {
            let frame: Hsla = if dark { hsla(220. / 360., 0.12, 0.11, 1.) } else { hsla(0., 0., 1., 1.) };
            let bar: Hsla = if dark { hsla(220. / 360., 0.12, 0.15, 1.) } else { hsla(220. / 360., 0.14, 0.96, 1.) };
            fill(window, 0.08, 0.10, 0.84, 0.80, 5., frame);
            fill(window, 0.08, 0.10, 0.84, 0.12, 4., bar);
            fill(window, 0.32, 0.14, 0.36, 0.04, 2., line);
            fill(window, 0.26, 0.34, 0.48, 0.075, 2., accent.opacity(0.85));
            fill(window, 0.32, 0.46, 0.36, 0.04, 1.5, line);
            fill(window, 0.16, 0.62, 0.20, 0.16, 3., line);
            fill(window, 0.40, 0.62, 0.20, 0.16, 3., line);
            fill(window, 0.64, 0.62, 0.20, 0.16, 3., line);
        }
        LibraryKind::Image | LibraryKind::Other => {
            let file = accent.opacity(if dark { 0.5 } else { 0.4 });
            fill(window, 0.40, 0.24, 0.20, 0.52, 4., file);
            fill(window, 0.44, 0.34, 0.12, 0.035, 1., hsla(0., 0., 1., 0.55));
            fill(window, 0.44, 0.42, 0.10, 0.035, 1., hsla(0., 0., 1., 0.4));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_many_tiles_fit_a_row_as_its_width_takes() {
        assert_eq!(columns_for(300.), 2);
        assert_eq!(columns_for(904.), 4);
        assert_eq!(columns_for(2400.), 6);
    }

    #[test]
    fn when_says_it_shortly_on_a_tile_and_in_words_in_a_list() {
        let now = 1_763_800_000_000.;
        assert_eq!(when_words(now - 14. * 60_000., now, false), "14m");
        assert_eq!(when_words(now - 14. * 60_000., now, true), "14m ago");
        assert_eq!(when_words(now - 26. * 3_600_000., now, true), "Yesterday");
        assert_eq!(when_words(now - 26. * 3_600_000., now, false), "1d");
        assert!(when_words(now - 10. * 86_400_000., now, true).starts_with("Nov "));
    }

    #[test]
    fn a_tab_and_a_source_narrow_what_the_box_is_asked() {
        assert_eq!(Tab::Images.kind(), Some(LibraryKind::Image));
        assert_eq!(Tab::All.kind(), None);
        assert_eq!(Source::Yours.query().as_deref(), Some("you"));
        assert_eq!(Source::Bot("bot_ada".into()).query().as_deref(), Some("bot_ada"));
        assert_eq!(Source::All.query(), None);
    }
}
