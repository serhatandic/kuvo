//! Menus and popovers: a menu of actions, a dropdown that opens one from a button, a context
//! menu that opens where you right-click, hover cards and tooltips.
//!
//! A menu takes the keyboard while it's open: Up and Down move through the entries (skipping
//! headers, separators and disabled entries, and wrapping around), Return chooses, Escape or a
//! click outside closes it and gives focus back to where it was.

use std::{cell::{Cell, RefCell}, rc::Rc, time::Duration};

use gpui::{
    AnyElement, AnyView, App, AppContext as _, Bounds, Context, DismissEvent, ElementId, Entity,
    EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyBinding, MouseButton,
    ParentElement, Pixels, Point, Render, RenderOnce, ScrollHandle, SharedString, Size,
    StatefulInteractiveElement, Styled, Subscription, Task, Window, actions, div,
    prelude::FluentBuilder as _, px,
};

use crate::{
    anchor::{Placement, Popover, Side, choose_vertical_side},
    button::keycap,
    icon::{Icon, IconName, IconSize},
    small::spinner,
    theme::{ActiveTheme, StyledType, TypeSize, radius, space},
};

actions!(menu, [SelectNext, SelectPrevious, Confirm, Dismiss]);

const CONTEXT: &str = "KuvoMenu";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, Some(CONTEXT)),
        KeyBinding::new("up", SelectPrevious, Some(CONTEXT)),
        KeyBinding::new("enter", Confirm, Some(CONTEXT)),
        KeyBinding::new("space", Confirm, Some(CONTEXT)),
        KeyBinding::new("escape", Dismiss, Some(CONTEXT)),
    ]);
}

/// The menu's padding: with 8 px rows inside, the 12 px `radius::POPOVER` corners nest.
const MENU_PADDING: Pixels = px(4.);

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

/// The tallest a menu grows before it scrolls.
const MOST_HEIGHT: f32 = 420.;

/// One line of a menu.
pub enum MenuItem {
    Entry(MenuEntry),
    Header(SharedString),
    Separator,
}

pub struct MenuEntry {
    label: SharedString,
    detail: Option<SharedString>,
    description: Option<SharedString>,
    icon: Option<IconName>,
    shortcut: Option<SharedString>,
    checked: bool,
    disabled: bool,
    loading: bool,
    muted: bool,
    destructive: bool,
    handler: Option<Handler>,
}

impl MenuEntry {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            detail: None,
            description: None,
            icon: None,
            shortcut: None,
            checked: false,
            disabled: false,
            loading: false,
            muted: false,
            destructive: false,
            handler: None,
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A second, quieter label after the first (a model's speed, a branch's ahead count).
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// A description below the name, with room to wrap.
    pub fn description(mut self, words: impl Into<SharedString>) -> Self {
        self.description = Some(words.into());
        self
    }

    pub fn shortcut(mut self, keys: impl Into<SharedString>) -> Self {
        self.shortcut = Some(keys.into());
        self
    }

    /// Marks the current choice with a check.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// A quiet status row that can't be picked while the work is under way.
    pub fn loading(mut self) -> Self {
        self.disabled = true;
        self.loading = true;
        self
    }

    /// Drawn quiet, and still chosen: a computer that's away.
    pub fn muted(mut self, muted: bool) -> Self {
        self.muted = muted;
        self
    }

    /// Drawn in the danger colour: deleting, stopping, signing out.
    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }

    pub fn on_select(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.handler = Some(Rc::new(handler));
        self
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }
}

/// A menu. Build it with entries, then open it with a [`PopoverMenu`] or a [`ContextMenuArea`].
pub struct Menu {
    items: Vec<MenuItem>,
    selected: Option<usize>,
    /// Whether the pointer made the highlight; only then does the pointer leaving clear it.
    selected_by_pointer: bool,
    focus: FocusHandle,
    width: Pixels,
    scroll: ScrollHandle,
    _source: Option<Subscription>,
}

impl EventEmitter<DismissEvent> for Menu {}

impl Focusable for Menu {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Menu {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self { items: Vec::new(), selected: None, selected_by_pointer: false, focus: cx.focus_handle(), width: px(240.), scroll: ScrollHandle::new(), _source: None }
    }

    pub fn entry(mut self, entry: MenuEntry) -> Self {
        self.items.push(MenuItem::Entry(entry));
        self
    }

    pub fn header(mut self, label: impl Into<SharedString>) -> Self {
        self.items.push(MenuItem::Header(label.into()));
        self
    }

    pub fn separator(mut self) -> Self {
        self.items.push(MenuItem::Separator);
        self
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    /// Refreshes the open menu in place when its source changes what it shows.
    pub fn follow<T: 'static, S: PartialEq + 'static>(
        mut self,
        source: &Entity<T>,
        state: impl Fn(&T) -> S + 'static,
        items: impl Fn(&S) -> Vec<MenuItem> + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut shown = state(source.read(cx));
        self.items = items(&shown);
        self._source = Some(cx.observe(source, move |menu, source, cx| {
            let next = state(source.read(cx));
            if shown != next {
                menu.set_items(items(&next), cx);
                shown = next;
            }
        }));
        self
    }

    /// Replaces the menu's items in place (used when items arrive asynchronously after opening).
    pub fn set_items(&mut self, items: Vec<MenuItem>, cx: &mut Context<Self>) {
        self.items = items;
        self.selected = None;
        self.selected_by_pointer = false;
        cx.notify();
    }

    /// Highlights the first entry that can be chosen (as when a menu opens from the keyboard).
    pub fn highlight_first(&mut self, cx: &mut Context<Self>) {
        self.selected = self.step(None, 1);
        self.selected_by_pointer = false;
        cx.notify();
    }

    /// The highlighted entry's label.
    pub fn selected_label(&self) -> Option<SharedString> {
        match self.items.get(self.selected?)? {
            MenuItem::Entry(entry) => Some(entry.label.clone()),
            _ => None,
        }
    }

    fn selectable(&self, ix: usize) -> bool {
        matches!(&self.items[ix], MenuItem::Entry(e) if !e.disabled)
    }

    fn step(&self, from: Option<usize>, by: isize) -> Option<usize> {
        let count = self.items.len() as isize;
        if count == 0 {
            return None;
        }
        let mut ix = from.map_or(if by > 0 { -1 } else { count }, |f| f as isize);
        for _ in 0..count {
            ix = (ix + by).rem_euclid(count);
            if self.selectable(ix as usize) {
                return Some(ix as usize);
            }
        }
        None
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = self.step(self.selected, 1);
        if let Some(ix) = self.selected {
            self.scroll.scroll_to_item(ix);
        }
        self.selected_by_pointer = false;
        cx.notify();
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = self.step(self.selected, -1);
        if let Some(ix) = self.selected {
            self.scroll.scroll_to_item(ix);
        }
        self.selected_by_pointer = false;
        cx.notify();
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.selected {
            self.choose(ix, window, cx);
        }
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    fn choose(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selectable(ix) {
            return;
        }
        let handler = match &self.items[ix] {
            MenuItem::Entry(entry) => entry.handler.clone(),
            _ => None,
        };
        cx.emit(DismissEvent);
        if let Some(handler) = handler {
            handler(window, cx);
        }
    }
}

impl Render for Menu {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let c = &theme.colors;
        // One leading column, shared by checks and icons (as in macOS menus), so every label in
        // a menu starts at the same place.
        let has_leading = self.items.iter().any(|i| matches!(i, MenuItem::Entry(e) if e.checked || e.icon.is_some() || e.loading));
        let rows = self.items.iter().enumerate().map(|(ix, item)| -> AnyElement {
            match item {
                MenuItem::Separator => div().h(px(1.)).mx(space::S2).my(space::S1).bg(c.border_strong).into_any_element(),
                MenuItem::Header(label) => div()
                    .px(space::S2)
                    .pt(space::S2)
                    .pb(space::S1)
                    .text_ui(TypeSize::Caption)
                    .text_color(c.text_faint)
                    .child(label.clone())
                    .into_any_element(),
                MenuItem::Entry(entry) => {
                    let selected = self.selected == Some(ix);
                    let color = if entry.loading {
                        c.text_muted
                    } else if entry.disabled {
                        c.text_faint
                    } else if entry.muted {
                        c.text_muted
                    } else if entry.destructive {
                        c.danger
                    } else {
                        c.text
                    };
                    let icon_color = if entry.disabled { c.text_faint } else if entry.destructive { c.danger } else { c.text_muted };
                    let selector = format!("menu-item-{}", entry.label);
                    let description_selector = format!("{selector}-description");
                    div()
                        .debug_selector(move || selector)
                        .id(ix)
                        .w_full()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .when(entry.description.is_none(), |d| d.h(px(30.)))
                        .when(entry.description.is_some(), |d| d.py(space::S1))
                        .flex_none()
                        .px(space::S2)
                        .rounded(radius::NAV)
                        .text_ui(TypeSize::Small)
                        .text_color(color)
                        .when(selected, |d| d.bg(c.hover))
                        .when(!entry.disabled, |d| {
                            d.cursor_pointer()
                                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                    if *hovered && this.selected != Some(ix) {
                                        this.selected = Some(ix);
                                        this.selected_by_pointer = true;
                                        cx.notify();
                                    }
                                }))
                                .on_click(cx.listener(move |this, _, window, cx| this.choose(ix, window, cx)))
                        })
                        .when(has_leading, |d| {
                            let leading = if entry.checked {
                                Some(Icon::new(IconName::Check).size(IconSize::Nav).color(c.accent))
                            } else {
                                entry.icon.map(|i| Icon::new(i).size(IconSize::Nav).color(icon_color))
                            };
                            d.child(div().flex().w(IconSize::Nav.pixels()).flex_none()
                                .children(entry.loading.then(|| spinner(("menu-loading", ix), IconSize::Nav, c.text_muted)))
                                .children(leading.filter(|_| !entry.loading)))
                        })
                        .child(div().flex_1().min_w_0().flex().flex_col()
                            .child(div().min_w_0().when(entry.description.is_none(), |d| d.truncate()).child(entry.label.clone()))
                            .children(entry.description.clone().map(|words| {
                                div().min_w_0().debug_selector(move || description_selector).text_color(c.text_muted).child(words)
                            })))
                        // (A long detail gives way too, so neither pushes the other out of the menu.)
                        .children(entry.detail.clone().map(|d| div().min_w_0().max_w(gpui::relative(0.6)).truncate().text_color(c.text_faint).child(d)))
                        .children(entry.shortcut.clone().map(|k| {
                            div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_faint).child(k)
                        }))
                        .into_any_element()
                }
            }
        });
        div()
            .id("menu")
            .debug_selector(|| "menu".into())
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered && this.selected_by_pointer && this.selected.is_some() {
                    this.selected = None;
                    cx.notify();
                }
            }))
            .occlude()
            .flex()
            .flex_col()
            .w(self.width)
            .p(MENU_PADDING)
            .rounded(radius::POPOVER)
            .bg(c.elevated)
            .border_1()
            .border_color(c.border)
            .shadow(theme.elevated_shadow())
            // A long one (every time zone) scrolls inside itself, never off the window.
            .max_h(px(MOST_HEIGHT))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(rows)
    }
}

/// What a popover can show: a [`Menu`], or a view of its own (a picker) that takes the keyboard
/// while it's open and says when it's done with [`DismissEvent`].
pub trait ManagedView: Focusable + EventEmitter<DismissEvent> + Render {}
impl<T: Focusable + EventEmitter<DismissEvent> + Render> ManagedView for T {}

/// Opens a menu from code (a shortcut, or the gallery's shot) and tells whether one is open.
#[derive(Clone, Default)]
pub struct PopoverMenuHandle(Rc<RefCell<Option<OpenMenu>>>);

struct OpenMenu {
    view: AnyView,
    focus: FocusHandle,
    /// What had focus before any menu opened here; it gets focus back when the menu closes.
    return_to: Option<FocusHandle>,
    _dismissed: Subscription,
}

impl PopoverMenuHandle {
    pub fn is_open(&self) -> bool {
        self.0.borrow().is_some()
    }

    pub fn menu(&self) -> Option<Entity<Menu>> {
        self.0.borrow().as_ref().and_then(|open| open.view.clone().downcast::<Menu>().ok())
    }

    /// What's open, a menu or a view of its own.
    pub fn view(&self) -> Option<AnyView> {
        self.0.borrow().as_ref().map(|open| open.view.clone())
    }

    /// Shows `menu`, giving it the keyboard. When it closes, focus goes back to what had it
    /// before: if that was another menu (a second right-click, or one dropdown opened while
    /// another was open), to what that menu was going to give focus back to.
    pub fn open<V: ManagedView>(&self, menu: Entity<V>, window: &mut Window, cx: &mut App) {
        let replaced = self.0.borrow_mut().take();
        let focused = window.focused(cx);
        let return_to = match replaced {
            Some(open) => {
                OpenMenus::remove(&open.focus, cx);
                open.return_to
            }
            None => OpenMenus::return_target(focused, cx),
        };
        let menu_focus = menu.focus_handle(cx);
        OpenMenus::add(menu_focus.clone(), return_to.clone(), cx);
        let dismissed = {
            let slot = Rc::downgrade(&self.0);
            let menu_focus = menu_focus.clone();
            let return_to = return_to.clone();
            window.subscribe(&menu, cx, move |_, _: &DismissEvent, window, cx| {
                if let Some(slot) = slot.upgrade() {
                    slot.borrow_mut().take();
                }
                OpenMenus::remove(&menu_focus, cx);
                // Leave focus alone if the chosen entry moved it on purpose (a rename field) or
                // a press outside put it somewhere else.
                let menu_kept_focus = menu_focus.contains_focused(window, cx) || window.focused(cx).is_none();
                if let (true, Some(target)) = (menu_kept_focus, &return_to) {
                    window.focus(target, cx);
                }
                window.refresh();
            })
        };
        *self.0.borrow_mut() = Some(OpenMenu { view: menu.into(), focus: menu_focus.clone(), return_to, _dismissed: dismissed });
        // The menu is painted through `deferred`, so its element only exists after the next
        // frame; focusing it before then would land nowhere.
        window.defer(cx, move |window, cx| window.focus(&menu_focus, cx));
        window.refresh();
    }

    /// Closes the menu without choosing, giving focus back if the menu had it.
    pub fn close(&self, window: &mut Window, cx: &mut App) {
        let open = self.0.borrow_mut().take();
        if let Some(open) = open {
            let menu_focus = open.focus.clone();
            OpenMenus::remove(&menu_focus, cx);
            if menu_focus.contains_focused(window, cx)
                && let Some(target) = &open.return_to {
                    window.focus(target, cx);
                }
        }
        window.refresh();
    }
}

/// The menus open in the app, each with where it gives focus back. A menu opened while another
/// has focus takes over that one's place to return to, rather than the other menu, which is
/// about to close.
#[derive(Default)]
struct OpenMenus(Vec<(gpui::WeakFocusHandle, Option<FocusHandle>)>);

impl gpui::Global for OpenMenus {}

/// Whether a menu has the keyboard (a selection it was opened over still shows as in use).
pub fn is_menu_focused(window: &Window, cx: &App) -> bool {
    cx.try_global::<OpenMenus>().is_some_and(|menus| menus.0.iter().filter_map(|(menu, _)| menu.upgrade()).any(|menu| menu.contains_focused(window, cx)))
}

impl OpenMenus {
    fn return_target(focused: Option<FocusHandle>, cx: &mut App) -> Option<FocusHandle> {
        let focused = focused?;
        let menus = cx.default_global::<OpenMenus>();
        match menus.0.iter().find(|(menu, _)| menu == &focused) {
            Some((_, return_to)) => return_to.clone(),
            None => Some(focused),
        }
    }

    fn add(menu: FocusHandle, return_to: Option<FocusHandle>, cx: &mut App) {
        let menus = cx.default_global::<OpenMenus>();
        menus.0.retain(|(open, _)| open.upgrade().is_some());
        menus.0.push((menu.downgrade(), return_to));
    }

    fn remove(menu: &FocusHandle, cx: &mut App) {
        cx.default_global::<OpenMenus>().0.retain(|(open, _)| open != menu && open.upgrade().is_some());
    }
}

type MenuBuilder = Rc<dyn Fn(&mut Window, &mut App) -> Entity<Menu>>;
/// Makes what a popover shows and opens it with the handle.
type Opener = Rc<dyn Fn(&PopoverMenuHandle, &mut Window, &mut App)>;

enum Trigger {
    Element(AnyElement),
    Button(Box<crate::Button>),
}

/// A dropdown: a trigger (usually a button) and the menu it opens from itself, by the rule in
/// [`crate::anchor`]: below it at its leading edge, unless told otherwise.
#[derive(IntoElement)]
pub struct PopoverMenu {
    id: ElementId,
    trigger: Trigger,
    menu: Opener,
    handle: Option<PopoverMenuHandle>,
    align_right: bool,
    above: bool,
    centered: bool,
    fill: bool,
}

impl PopoverMenu {
    pub fn new(
        id: impl Into<ElementId>,
        trigger: impl IntoElement,
        menu: impl Fn(&mut Window, &mut App) -> Entity<Menu> + 'static,
    ) -> Self {
        Self::view(id, trigger, menu)
    }

    /// A popover of a view of its own (a picker), made only as it opens.
    pub fn view<V: ManagedView>(id: impl Into<ElementId>, trigger: impl IntoElement, view: impl Fn(&mut Window, &mut App) -> Entity<V> + 'static) -> Self {
        Self::view_for(id.into(), Trigger::Element(trigger.into_any_element()), view)
    }

    /// A button trigger that also opens with Enter and Space.
    pub fn button<V: ManagedView>(id: impl Into<ElementId>, button: crate::Button, view: impl Fn(&mut Window, &mut App) -> Entity<V> + 'static) -> Self {
        Self::view_for(id.into(), Trigger::Button(Box::new(button)), view)
    }

    fn view_for<V: ManagedView>(id: ElementId, trigger: Trigger, view: impl Fn(&mut Window, &mut App) -> Entity<V> + 'static) -> Self {
        let menu: Opener = Rc::new(move |handle, window, cx| {
            let view = view(window, cx);
            handle.open(view, window, cx);
        });
        Self { id, trigger, menu, handle: None, align_right: false, above: false, centered: false, fill: false }
    }

    /// Opens above the trigger (a composer's control, at the window's foot).
    pub fn above(mut self) -> Self {
        self.above = true;
        self
    }

    /// Centred on the row it's in, rather than at its top (a toolbar's controls of other heights).
    pub fn centered(mut self) -> Self {
        self.centered = true;
        self
    }

    /// Takes the available width, so the trigger can size itself to the parent.
    pub fn fill(mut self) -> Self {
        self.fill = true;
        self
    }

    pub fn with_handle(mut self, handle: PopoverMenuHandle) -> Self {
        self.handle = Some(handle);
        self
    }

    /// Lines the menu up with the trigger's trailing edge (a trigger at the end of a bar or row).
    pub fn align_right(mut self) -> Self {
        self.align_right = true;
        self
    }
}

impl RenderOnce for PopoverMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let handle = self.handle.unwrap_or_else(|| {
            window.use_keyed_state(self.id.clone(), cx, |_, _| PopoverMenuHandle::default()).read(cx).clone()
        });
        let open = handle.view();
        let builder = self.menu.clone();
        let toggle = handle.clone();
        let trigger = match self.trigger {
            Trigger::Button(button) => {
                let (handle, build) = (handle.clone(), self.menu.clone());
                button.on_click(move |event, window, cx| {
                    if event.is_keyboard() {
                        if handle.is_open() {
                            handle.close(window, cx);
                        } else {
                            build(&handle, window, cx);
                        }
                        cx.stop_propagation();
                    }
                }).into_any_element()
            }
            Trigger::Element(element) => element,
        };
        let placement = match (self.above, self.align_right) {
            (false, false) => Placement::below(),
            (false, true) => Placement::below().align_end(),
            (true, false) => Placement::above(),
            (true, true) => Placement::above().align_end(),
        };
        div()
            .id(self.id)
            .relative()
            .flex()
            .flex_none()
            .when(self.fill, |d| d.w_full().min_w_0())
            .when(!self.centered, |d| d.self_start())
            .when(self.centered, |d| d.self_center())
            // A press on the trigger while the menu is open closes it, rather than closing on
            // the press and opening again on the release.
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                // The press mustn't move focus: the menu is about to take it.
                window.prevent_default();
                if toggle.is_open() {
                    toggle.close(window, cx);
                    cx.stop_propagation();
                } else {
                    builder(&toggle, window, cx);
                }
            })
            .child(trigger)
            .children(open.map(|menu| Popover::new(("menu-motion", menu.entity_id().as_u64()), placement, menu)))
    }
}

/// An area that opens a menu where it's right-clicked.
#[derive(IntoElement)]
pub struct ContextMenuArea {
    id: ElementId,
    child: AnyElement,
    menu: MenuBuilder,
    handle: Option<PopoverMenuHandle>,
    fill: bool,
}

#[derive(Clone, Default)]
struct ContextState(Rc<RefCell<Option<Point<Pixels>>>>, PopoverMenuHandle);

impl ContextMenuArea {
    pub fn new(
        id: impl Into<ElementId>,
        child: impl IntoElement,
        menu: impl Fn(&mut Window, &mut App) -> Entity<Menu> + 'static,
    ) -> Self {
        Self { id: id.into(), child: child.into_any_element(), menu: Rc::new(menu), handle: None, fill: false }
    }

    pub fn with_handle(mut self, handle: PopoverMenuHandle) -> Self {
        self.handle = Some(handle);
        self
    }

    /// Takes all the room it's given (around a list that fills its place).
    pub fn fill(mut self) -> Self {
        self.fill = true;
        self
    }
}

impl RenderOnce for ContextMenuArea {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| ContextState::default()).read(cx).clone();
        let handle = self.handle.unwrap_or_else(|| state.1.clone());
        let builder = self.menu.clone();
        let opener = (state.0.clone(), handle.clone());
        let at = *state.0.borrow();
        let open = handle.view();
        div()
            .id(self.id)
            .when(self.fill, |d| d.size_full())
            .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                window.prevent_default();
                *opener.0.borrow_mut() = Some(event.position);
                let menu = builder(window, cx);
                opener.1.open(menu, window, cx);
                cx.stop_propagation();
            })
            .child(self.child)
            .children(open.zip(at).map(|(menu, at)| Popover::at(("context-menu-motion", menu.entity_id().as_u64()), at, menu)))
    }
}


/// Draws what a popover shows.
type Builder = Box<dyn Fn(&mut Window, &mut App) -> AnyElement>;
/// A hover card: richer than a tooltip (a person, a session, a file), shown after the pointer
/// rests on its trigger, and kept open while the pointer moves onto it.
pub struct HoverCard {
    build: Builder,
}

impl HoverCard {
    pub fn new(build: impl Fn(&mut Window, &mut App) -> AnyElement + 'static) -> Self {
        Self { build: Box::new(build) }
    }

    /// The view GPUI's hoverable tooltip shows.
    pub fn build(self, cx: &mut App) -> AnyView {
        cx.new(|_| self).into()
    }
}

impl Render for HoverCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = (self.build)(window, cx);
        hover_card_frame(content, cx)
    }
}

/// The card a hover card's content sits in, a little below the pointer; it takes the pointer's
/// clicks itself, so what's under it isn't reached.
pub fn hover_card_frame(content: impl IntoElement, cx: &App) -> impl IntoElement {
    div().occlude().pt(space::S1_5).child(card_frame(content, cx))
}

/// A card over the window (a hover card, a ring's card), raised. What holds it takes the pointer's
/// clicks (`occlude`), so what's under it isn't reached.
pub fn card_frame(content: impl IntoElement, cx: &App) -> gpui::Div {
    let theme = cx.theme();
    div()
        .w(px(280.))
        .p(space::S4)
        .rounded(radius::CARD)
        .bg(theme.colors.elevated)
        .border_1()
        .border_color(theme.colors.border)
        .shadow(theme.elevated_shadow())
        .child(content)
}

type HoverViewBuilder = Rc<dyn Fn(&mut Window, &mut App) -> AnyView>;

#[derive(Default)]
struct HoverPopoverState {
    trigger_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    trigger_hovered: bool,
    card_hovered: bool,
    open: Option<(AnyView, Side)>,
    pending: Option<Task<()>>,
}

/// A hover-triggered popover card anchored to its trigger via [`Popover`]. Before opening the
/// first frame, it decides above vs below from the trigger's measured bounds and viewport room
/// ([`choose_vertical_side`]) and locks that side so async content growth never flips the card.
#[derive(IntoElement)]
pub struct HoverPopover {
    id: ElementId,
    trigger: AnyElement,
    build: HoverViewBuilder,
    delay: Duration,
    expected_height: Pixels,
    fill: bool,
    priority: usize,
}

impl HoverPopover {
    pub fn new(
        id: impl Into<ElementId>,
        trigger: impl IntoElement,
        build: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            trigger: trigger.into_any_element(),
            build: Rc::new(build),
            delay: Duration::from_millis(300),
            expected_height: px(240.),
            fill: false,
            priority: 2,
        }
    }

    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// Estimated full height of the card (including async content) used before Frame 1 to decide
    /// whether to open below or above the trigger.
    pub fn expected_height(mut self, height: Pixels) -> Self {
        self.expected_height = height;
        self
    }

    /// Takes the parent's full width (for full-width rows such as linked-item rows).
    pub fn fill(mut self) -> Self {
        self.fill = true;
        self
    }

    pub fn priority(mut self, priority: usize) -> Self {
        self.priority = priority;
        self
    }
}

const HOVER_CLOSE_GRACE: Duration = Duration::from_millis(80);

fn schedule_hover_close(
    state: &mut HoverPopoverState,
    window: &mut Window,
    cx: &mut Context<HoverPopoverState>,
) {
    if state.trigger_hovered || state.card_hovered {
        return;
    }
    if state.open.is_none() {
        state.pending = None;
        return;
    }
    state.pending = Some(cx.spawn_in(window, async move |this, cx| {
        cx.background_executor().timer(HOVER_CLOSE_GRACE).await;
        this.update_in(cx, |state, window, _| {
            if !state.trigger_hovered && !state.card_hovered {
                state.open = None;
                state.pending = None;
                window.refresh();
            }
        })
        .ok();
    }));
}

impl RenderOnce for HoverPopover {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| HoverPopoverState::default());
        let (measured, open) = {
            let s = state.read(cx);
            (s.trigger_bounds.clone(), s.open.clone())
        };
        let on_trigger_hover = {
            let state = state.clone();
            let build = self.build.clone();
            let delay = self.delay;
            let expected_height = self.expected_height;
            move |hovered: &bool, window: &mut Window, cx: &mut App| {
                let hovered = *hovered;
                state.update(cx, |s, cx| {
                    s.trigger_hovered = hovered;
                    if hovered {
                        if s.open.is_some() {
                            s.pending = None;
                            return;
                        }
                        let build = build.clone();
                        s.pending = Some(cx.spawn_in(window, async move |this, cx| {
                            cx.background_executor().timer(delay).await;
                            this.update_in(cx, |state, window, cx| {
                                if !state.trigger_hovered || state.open.is_some() {
                                    return;
                                }
                                let viewport = Bounds { origin: Point::default(), size: window.viewport_size() }
                                    .inset(window.client_inset().unwrap_or(px(0.)));
                                let trigger = state.trigger_bounds.get().unwrap_or_else(|| Bounds {
                                    origin: window.mouse_position(),
                                    size: Size::default(),
                                });
                                let side = choose_vertical_side(trigger, viewport, expected_height, space::S1_5);
                                let view = build(window, cx);
                                state.open = Some((view, side));
                                state.pending = None;
                                window.refresh();
                            })
                            .ok();
                        }));
                    } else {
                        schedule_hover_close(s, window, cx);
                    }
                });
            }
        };
        let on_trigger_down = {
            let state = state.clone();
            move |_: &gpui::MouseDownEvent, window: &mut Window, cx: &mut App| {
                state.update(cx, |s, _| {
                    s.pending = None;
                    if s.open.take().is_some() {
                        window.refresh();
                    }
                });
            }
        };
        let popover = open.map(|(view, side)| {
            let placement = match side {
                Side::Above => Placement::above(),
                _ => Placement::below(),
            }
            .flush()
            .lock_side();
            let card_state = state.clone();
            let id_num = view.entity_id().as_u64();
            let card = div()
                .id(("hover-popover-card", id_num))
                .occlude()
                .when(side == Side::Below, |d| d.pt(space::S1_5))
                .when(side == Side::Above, |d| d.pb(space::S1_5))
                .on_hover(move |hovered, window, cx| {
                    let hovered = *hovered;
                    card_state.update(cx, |s, cx| {
                        s.card_hovered = hovered;
                        if hovered {
                            s.pending = None;
                        } else {
                            schedule_hover_close(s, window, cx);
                        }
                    });
                })
                .child(view);
            Popover::new(("hover-popover-motion", id_num), placement, card).priority(self.priority)
        });
        div()
            .on_children_prepainted(move |children, _, _| {
                if let Some(bounds) = children.first() {
                    measured.set(Some(*bounds));
                }
            })
            .id(self.id)
            .relative()
            .flex()
            .min_w_0()
            .max_w_full()
            .when(self.fill, |d| d.w_full())
            .on_hover(on_trigger_hover)
            .on_mouse_down(MouseButton::Left, on_trigger_down)
            .child(self.trigger)
            .children(popover)
    }
}

/// A tooltip: a short label (and an optional shortcut) in a small capsule, shown after the
/// pointer rests.
pub struct Tooltip {
    text: SharedString,
    shortcut: Option<SharedString>,
}

impl Tooltip {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into(), shortcut: None }
    }

    pub fn shortcut(mut self, keys: impl Into<SharedString>) -> Self {
        self.shortcut = Some(keys.into());
        self
    }

    pub fn build(self, cx: &mut App) -> AnyView {
        cx.new(|_| self).into()
    }
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        tooltip_frame(self.text.clone(), self.shortcut.clone(), cx)
    }
}

/// How a tooltip looks; also used to draw one in place (the gallery).
pub fn tooltip_frame(text: SharedString, shortcut: Option<SharedString>, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let c = &theme.colors;
    let selector = text.clone();
    div().debug_selector(move || format!("tooltip-{selector}")).pl(px(4.)).pt(px(6.)).child(
        div()
            .flex()
            .items_center()
            .gap(space::S2)
            .px(space::S2)
            .py(space::S1)
            .rounded(radius::NAV)
            .bg(c.elevated)
            .border_1()
            .border_color(c.border)
            .shadow(theme.elevated_shadow())
            .text_ui(TypeSize::Caption)
            .text_color(c.text)
            .child(text)
            .children(shortcut.map(|k| keycap(k, c.border_strong, c.text_muted))),
    )
}
