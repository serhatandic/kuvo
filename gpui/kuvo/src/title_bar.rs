//! The title bar: the window's buttons, the sidebar toggle, the computer switcher, back and forward
//! as one group over the rail and the area's sidebar, a thin divider after it; then the main view's title, and the side panel's button.
//! Dragging it moves the window.

use kuvo_ui::motion::{MotionExt as _};
use gpui::{
    AnyElement, AppContext as _, Context, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Pixels, SharedString, StatefulInteractiveElement as _, Styled, WeakEntity, div, prelude::FluentBuilder as _, px,
};
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TextRole, TypeSize,
    menu::{Menu, MenuEntry, MenuItem, PopoverMenu},
    small::{Chip, spinner, status_dot},
    theme::{radius, space},
};

use crate::area::{Area, Back, Forward, ShowComputers};
use crate::areas::{Reading, computer_line};
use crate::session_menu::{ActivePopover, title_anchored_popover};
use crate::workspace::{BoxState, Switch, Workspace};
use kuvo_box::boxes::{Pairing, Reach, Row};

/// How tall it is.
pub const HEIGHT: f32 = 44.;
/// The room the window's own buttons take at its left (macOS's traffic lights).
const LIGHTS: f32 = 76.;

pub fn render(workspace: &Workspace, edge: Option<Pixels>, compact: bool, cx: &mut Context<Workspace>) -> AnyElement {
    let c = cx.theme().colors.clone();
    let (title, beside) = workspace.title(cx);
    let in_sessions = workspace.area() == Area::Sessions && !workspace.client().gate().is_held();
    let archived = in_sessions && workspace.opened_is_archived();
    let more_menu = (in_sessions).then(|| workspace.opened().map(|session| session_more_menu(workspace, session, cx))).flatten();
    let button_size = if compact { ButtonSize::Small } else { ButtonSize::Medium };
    let icon_size = if compact { IconSize::Medium } else { IconSize::Large };
    let gap = space::S0_5;
    let divider_gap = space::S1;
    let divider_width = px(1.);
    let picker_room = edge.filter(|_| !compact).map_or(px(184.), |edge| {
        edge - px(LIGHTS) - button_size.height() * 3. - gap * 3. - divider_gap - divider_width
    });
    let toggle = buttons(
        Button::icon_only("toggle-sidebar", IconName::SidebarLeft, if workspace.sidebar_shown() { "Hide Sidebar" } else { "Show Sidebar" })
            .ghost()
            .size(button_size)
            .icon_size(icon_size)
            .track_focus(workspace.sidebar_toggle_focus().clone())
            .tooltip_keys("⌘B")
            .on_click(cx.listener(|this, _, window, cx| this.toggle_sidebar(window, cx))),
    );
    let navigation = div().flex().flex_none().items_center().gap(gap)
        .child(buttons(computer_switcher(workspace, compact, picker_room, cx)))
        .child(buttons(
            Button::icon_only("back", IconName::ArrowLeft, "Back")
                .ghost()
                .size(button_size)
                .icon_size(icon_size)
                .tooltip_keys("⌘[")
                .disabled(!workspace.can_go_back())
                .on_click(|_, window, cx| window.dispatch_action(Box::new(Back), cx)),
        ))
        .child(buttons(
            Button::icon_only("forward", IconName::ArrowRight, "Forward")
                .ghost()
                .size(button_size)
                .icon_size(icon_size)
                .tooltip_keys("⌘]")
                .disabled(!workspace.can_go_forward())
                .on_click(|_, window, cx| window.dispatch_action(Box::new(Forward), cx)),
        ));
    // With the strip at its minimum, the traffic lights leave room for the toggle alone.
    let (left_navigation, right_navigation) = if compact {
        (None, Some(navigation))
    } else {
        (Some(navigation), None)
    };
    let group = div().flex().flex_none().items_center().gap(gap)
        .child(toggle)
        .children(left_navigation);
    let divider = div().id("title-bar-divider").debug_selector(|| "title-bar-divider".into()).flex_none().w(divider_width).h(px(16.)).bg(c.border);
    let panel_button = (workspace.area() == Area::Sessions).then(|| {
        Button::icon_only("toggle-panel", IconName::PanelRight, if workspace.is_panel_open() { "Hide Panel" } else { "Show Panel" })
            .ghost()
            .size(ButtonSize::Medium)
                .icon_size(IconSize::Large)
            .tooltip("⌥⌘0")
            .on_click(cx.listener(|this, _, _, cx| this.toggle_panel(cx)))
    });
    div()
        .id("title-bar")
        .flex()
        .flex_none()
        .items_center()
        .h(px(HEIGHT))
        .on_mouse_down(MouseButton::Left, |event, window, _| {
            if event.click_count == 1 {
                window.start_window_move();
            } else if event.click_count == 2 {
                window.titlebar_double_click();
            }
        })
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(divider_gap)
                .when_some(edge, |zone, edge| zone.w(edge))
                .h_full()
                .pl(px(LIGHTS))
                .child(group)
                .when(edge.is_some(), |zone| zone.justify_between())
                .child(divider),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(space::S2)
                .pl(space::S3)
                .pr(space::S3)
                .children(right_navigation)
                // The title is the bar's strongest words; where it is, quieter beside it.
                .children(title.map(|title| div().debug_selector(|| "window-title".into()).min_w_0().truncate().text_role(TextRole::WindowTitle).text_color(c.text).child(title)))
                .children(beside.map(|beside| div().flex_none().text_role(TextRole::WindowContext).text_color(c.text_faint).child(beside)))
                .when(archived, |d| d.child(div().flex_none().child(Chip::new("is-archived", "Archived"))))
                .child(div().flex_1())
                .children(update_chip(workspace, cx))
                .children(more_menu)
                .children(panel_button.map(buttons)),
        )
        .into_any_element()
}

fn session_more_menu(workspace: &Workspace, session: &str, cx: &Context<Workspace>) -> AnyElement {
    let menu_handle = workspace.session_menu().title_menu.clone();
    let popover = workspace
        .session_menu()
        .active_popover
        .as_ref()
        .filter(|popover| popover.is_at_title_bar())
        .map(|popover| {
            let content = match popover {
                ActivePopover::Rename { view, .. } => view.clone().into_any_element(),
                ActivePopover::Delete { view, .. } => view.clone().into_any_element(),
            };
            title_anchored_popover(content)
        });
    let selected = menu_handle.is_open() || popover.is_some();
    let trigger = Button::icon_only("session-more", IconName::More, "Session actions")
        .ghost()
        .size(ButtonSize::Medium)
                .icon_size(IconSize::Large)
        .selected(selected)
        .tooltip("Session actions");
    let session_id = session.to_owned();
    let workspace_handle = cx.entity();
    let menu = PopoverMenu::new("session-menu", trigger, move |window, cx| {
        workspace_handle.update(cx, |workspace, cx| workspace.open_title_menu(&session_id, window, cx))
    })
    .with_handle(menu_handle)
    .align_right();
    div()
        .relative()
        .flex_none()
        .child(buttons(menu))
        .children(popover)
        .into_any_element()
}

/// A press in here is its buttons', never the title bar's window move.
fn buttons(child: impl IntoElement) -> impl IntoElement {
    div().flex_none().on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(child)
}

/// The computer whose box the window shows, and how it stands; it opens the list of your computers.
fn computer_switcher(workspace: &Workspace, compact: bool, room: Pixels, cx: &Context<Workspace>) -> AnyElement {
    let c = &cx.theme().colors;
    let name = workspace.box_name();
    let dot = match workspace.box_state() {
        BoxState::Connecting => Some(c.text_faint),
        BoxState::Unreachable(_) => Some(c.danger),
        BoxState::Connected | BoxState::Held => None,
    };
    let trigger = if compact {
        Button::icon_only("computer-switcher", IconName::Computer, name.clone())
            .size(ButtonSize::Small)
    } else {
        Button::new("computer-switcher", name.clone())
            .ghost()
            .quiet()
            .icon(IconName::Computer)
            .icon_after(IconName::ChevronDown)
            .truncate_at(room)
            .tooltip(name.clone())
    }.icon_size(IconSize::Medium);
    let this = cx.entity().downgrade();
    div()
        .relative()
        .flex()
        .items_center()
        .child(PopoverMenu::new("computer-menu", trigger, move |_, cx| {
            let workspace = this.upgrade();
            let this = this.clone();
            cx.new(|cx| {
                let menu = Menu::new(cx).width(px(280.));
                match workspace {
                    Some(workspace) => menu.follow(&workspace, ComputerMenu::read, move |state| state.items(&this), cx),
                    None => menu,
                }
            })
        }))
        .children(dot.map(|color| div().absolute().top(px(1.)).right(px(1.)).child(status_dot(color))))
        .into_any_element()
}

#[derive(PartialEq)]
struct ComputerMenuRow {
    name: String,
    mac: bool,
    detail: String,
    pick: Option<String>,
    pair_again: Option<String>,
}

#[derive(PartialEq)]
struct ComputerMenu {
    name: SharedString,
    detail: Option<&'static str>,
    rows: Vec<ComputerMenuRow>,
    pairing: Option<Pairing>,
}

impl ComputerMenu {
    fn read(workspace: &Workspace) -> Self {
        let detail = match workspace.box_state() {
            BoxState::Connecting => Some("Connecting…"),
            BoxState::Unreachable(_) => Some("Can’t connect"),
            BoxState::Connected | BoxState::Held => None,
        };
        let (rows, pairing) = if let Some(switching) = workspace.switching() {
            let rows = switching.rows.iter()
                .filter(|row| Some(row.key()) != switching.shown.as_deref() && !(switching.shown.is_none() && row.reach == Reach::Nearest))
                .map(|row| ComputerMenuRow {
                    name: row.computer.name.clone(),
                    mac: row.computer.is_mac(),
                    detail: reach_line(row),
                    pick: (row.usable() && !row.key().is_empty()).then(|| row.key().to_owned()),
                    pair_again: match &row.reach {
                        Reach::Readd(said) => said.pair_again_label().map(str::to_owned),
                        _ => None,
                    },
                })
                .collect();
            (rows, Some(switching.pairing.clone()))
        } else {
            let rows = match workspace.computers() {
                Reading::Ready(computers) => computers.iter().filter(|computer| !computer.here)
                    .map(|computer| ComputerMenuRow {
                        name: computer.name.clone(), mac: computer.is_mac(), detail: computer_line(computer), pick: None, pair_again: None,
                    }).collect(),
                _ => Vec::new(),
            };
            (rows, None)
        };
        Self { name: workspace.box_name(), detail, rows, pairing }
    }

    fn items(&self, workspace: &WeakEntity<Workspace>) -> Vec<MenuItem> {
        let shown = MenuEntry::new(self.name.clone()).icon(IconName::Computer).checked(true);
        let shown = match self.detail { Some(detail) => shown.detail(detail), None => shown };
        let mut items = vec![MenuItem::Header("Shown in This Window".into()), MenuItem::Entry(shown)];
        if !self.rows.is_empty() {
            items.push(MenuItem::Header("Your Other Computers".into()));
        }
        for row in &self.rows {
            let entry = MenuEntry::new(row.name.clone())
                .icon(if row.mac { IconName::Computer } else { IconName::Terminal })
                .detail(row.detail.clone());
            let entry = if let Some(key) = row.pick.clone() {
                let workspace = workspace.clone();
                entry.on_select(move |_, cx| {
                    workspace.update(cx, |_, cx| cx.emit(Switch::To(key.clone()))).ok();
                })
            } else {
                entry.disabled(true)
            };
            items.push(MenuItem::Entry(entry));
            if let Some(label) = &row.pair_again {
                items.push(MenuItem::Entry(pair_again(label, workspace)));
            }
        }
        let status = match &self.pairing {
            Some(Pairing::Waiting(why) | Pairing::Refused(why)) => Some(MenuEntry::new("Your other computers aren’t reached yet").detail(why.clone()).disabled(true)),
            Some(Pairing::Checking(why)) => Some(MenuEntry::new(why.clone()).loading()),
            Some(Pairing::Readd(said)) => Some(MenuEntry::new(said.title.clone().unwrap_or_default()).detail(said.message.clone()).disabled(true)),
            Some(Pairing::Removed(why)) => Some(MenuEntry::new("This app was removed from your devices").detail(why.clone()).disabled(true)),
            None if !self.rows.is_empty() => Some(MenuEntry::new("Switching to another computer").detail("not in this app yet").disabled(true)),
            _ => None,
        };
        if let Some(status) = status {
            items.extend([MenuItem::Separator, MenuItem::Entry(status)]);
        }
        let action = match &self.pairing {
            Some(Pairing::Readd(said)) => said.pair_again_label(),
            Some(Pairing::Removed(_)) => Some("Connect This App Again"),
            _ => None,
        };
        if let Some(label) = action {
            items.push(MenuItem::Entry(pair_again(label, workspace)));
        }
        if self.pairing.is_some() || !self.rows.is_empty() {
            if self.pairing.is_some() {
                items.push(MenuItem::Separator);
            }
            items.push(MenuItem::Entry(MenuEntry::new("Show Computers").shortcut("⌃4")
                .on_select(|window, cx| window.dispatch_action(Box::new(ShowComputers), cx))));
        }
        items
    }
}

fn pair_again(label: &str, workspace: &WeakEntity<Workspace>) -> MenuEntry {
    let workspace = workspace.clone();
    MenuEntry::new(label.to_owned()).on_select(move |_, cx| {
        workspace.update(cx, |_, cx| cx.emit(Switch::PairAgain)).ok();
    })
}

/// How this app stands with one of your computers, in the switcher's words.
pub fn reach_line(row: &Row) -> String {
    match &row.reach {
        // (This app's own fact, not a box's: which computer the app itself runs on. No box can
        // say it for the app, so the words are drawn here.)
        Reach::Nearest => "This computer".into(),
        Reach::Connected => {
            let mut computer = row.computer.clone();
            // (As this app found it: reached.)
            computer.reach = "connected".into();
            computer.here = false;
            computer_line(&computer)
        }
        Reach::Connecting => row.computer.reach_title("connecting"),
        Reach::Held { .. } => row.computer.reach_title("held"),
        Reach::Away(_) => row.computer.reach_title("away"),
        Reach::NotPaired => row.computer.reach_title("notPaired"),
        Reach::Waiting(message) => message.clone(),
        Reach::Readd(said) => said.message.clone(),
        // The list itself says why where it can (it's away, it's older); else that this app has no
        // way to it.
        Reach::Unreached if row.computer.is_older() || row.computer.reach == "away" => computer_line(&row.computer),
        Reach::Unreached => row.computer.reach_title("unreached"),
    }
}

/// The update chip's words: short, as the card's title names the computer.
pub fn chip_label(held: &crate::workspace::HeldUpdate) -> &'static str {
    if held.updating {
        "Updating…"
    } else if held.own && !held.attention && held.action.as_ref().is_some_and(|action| action.why.is_none()) {
        // This Mac's own Kuvo, waiting for your sessions here to finish.
        "Update Ready"
    } else if held.action.as_ref().is_some_and(|action| action.why.is_none()) {
        "Update Kuvo"
    } else {
        "Needs an update"
    }
}

/// While the box this window shows waits for its update: a small chip at the title bar's right, as
/// Zed shows its own update (`UpdateButton` beside the title bar's other controls), never a strip
/// that pushes the window down. It says how the update stands in a few words (a spinner while it
/// runs), and opens a card with the box's words and, where the box that lists your computers
/// offers it, Update Now (asked first where that box says to).
fn update_chip(workspace: &Workspace, cx: &Context<Workspace>) -> Option<AnyElement> {
    let held = workspace.update_chip()?;
    let c = cx.theme().colors.clone();
    let open = workspace.update_card_open();
    let label = chip_label(&held);
    let leading = if held.updating {
        spinner("update-chip-spinner", IconSize::Large, c.icon).into_any_element()
    } else {
        Icon::new(if held.attention { IconName::Warning } else { IconName::ArrowDown }).size(IconSize::Large).color(if held.attention { c.warning } else { c.icon }).into_any_element()
    };
    let chip = div()
        .id("update-chip")
        .debug_selector(|| "update-chip".into())
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S1_5)
        .h(px(28.))
        .px(space::S2)
        .rounded(radius::PILL)
        .border_1()
        .border_color(c.border)
        .bg(if open { c.selected } else { c.surface })
        .hover(|s| s.bg(c.hover))
        .cursor_pointer()
        .text_ui(TypeSize::Small)
        .text_color(c.text)
        .child(leading)
        .child(div().debug_selector(|| "update-chip-label".into()).child(div().id("update-chip-label").child(label).map(|d| if held.updating { d.pulsing().into_any_element() } else { d.into_any_element() })))
        .on_click(cx.listener(|this, _, _, cx| this.toggle_update_card(cx)));
    let card = open.then(|| crate::session_menu::title_anchored_popover(update_card(workspace, &held, cx)));
    Some(div().relative().flex_none().child(buttons(chip)).children(card).into_any_element())
}

/// The update chip's card: the box's words, then Update Now where it's offered (its confirm asked
/// in its place first), or why it can't be done now.
fn update_card(workspace: &Workspace, held: &crate::workspace::HeldUpdate, cx: &Context<Workspace>) -> AnyElement {
    let theme = cx.theme().clone();
    let c = &theme.colors;
    let confirm = workspace.update_card_confirming().then(|| held.action.as_ref().and_then(|action| action.confirm.clone())).flatten();
    let sending = workspace.update_card_sending();
    let (title, words) = match &confirm {
        Some(confirm) => (confirm.title.clone(), confirm.message.clone()),
        None => (held.title.clone(), held.detail.clone()),
    };
    let why = held.action.as_ref().and_then(|action| action.why.clone());
    let buttons_row = match (&confirm, &held.action) {
        (Some(confirm), _) => Some(
            div()
                .flex()
                .justify_end()
                .gap(space::S2)
                .pt(space::S1)
                .child(Button::new("update-cancel", "Cancel").size(ButtonSize::Small).disabled(sending).on_click(cx.listener(|this, _, _, cx| this.close_update_card(cx))))
                .child(Button::new("update-confirm", confirm.label.clone()).primary().size(ButtonSize::Small).loading(sending).on_click(cx.listener(|this, _, _, cx| this.update_now(true, cx))))
                .into_any_element(),
        ),
        (None, Some(action)) if action.why.is_none() => Some(
            div()
                .flex()
                .justify_end()
                .pt(space::S1)
                .child(Button::new("update-now", action.label.clone()).primary().size(ButtonSize::Small).loading(sending).on_click(cx.listener(|this, _, _, cx| this.update_now(false, cx))))
                .into_any_element(),
        ),
        _ => None,
    };
    div()
        .id("update-card")
        .debug_selector(|| "update-card".into())
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_update_card(cx)))
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
            if event.keystroke.key == "escape" {
                cx.stop_propagation();
                this.close_update_card(cx);
            }
        }))
        .w(px(300.))
        .flex()
        .flex_col()
        .gap(space::S2)
        .p(space::S3)
        .rounded(radius::POPOVER)
        .bg(c.elevated)
        .border_1()
        .border_color(c.border)
        .shadow(theme.elevated_shadow())
        .child(
            div()
                .flex()
                .items_center()
                .gap(space::S2)
                .child(if held.updating && confirm.is_none() {
                    spinner("update-card-spinner", IconSize::Nav, c.icon).into_any_element()
                } else {
                    Icon::new(IconName::Computer).color(c.icon).into_any_element()
                })
                .child(div().debug_selector(|| "update-card-title".into()).min_w_0().text_ui(TypeSize::Small).medium().text_color(c.text).child(title).map(|d| if held.updating && confirm.is_none() { d.id("update-card-label").pulsing().into_any_element() } else { d.into_any_element() })),
        )
        .children((!words.is_empty()).then(|| div().debug_selector(|| "update-card-words".into()).text_ui(TypeSize::Small).text_color(c.text_muted).child(words)))
        .children(why.filter(|_| confirm.is_none()).map(|why| div().debug_selector(|| "update-card-why".into()).text_ui(TypeSize::Caption).text_color(c.text_faint).child(why)))
        .children(workspace.update_card_error().map(|error| div().debug_selector(|| "update-card-error".into()).text_ui(TypeSize::Caption).text_color(c.danger).child(error.to_owned())))
        .children(buttons_row)
        .into_any_element()
}
