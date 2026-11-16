//! The title bar: the window's buttons, the computer switcher, back, forward and New Session over
//! the rail and the area's sidebar; then the main view's title, and the side panel's button.
//! Dragging it moves the window.

use gpui::{
    AnyElement, AppContext as _, Context, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, StatefulInteractiveElement as _, Styled, div, prelude::FluentBuilder as _, px,
};
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TypeSize,
    menu::{Menu, MenuEntry, PopoverMenu},
    small::{Chip, spinner, status_dot},
    theme::{radius, space},
};

use crate::area::{Area, Back, Forward, ShowComputers};
use crate::areas::{Reading, computer_line};
use crate::session_menu::{ActivePopover, title_anchored_popover};
use crate::workspace::{BoxState, Switch, Workspace};
use kuvo_box::boxes::{Pairing, Reach, Row};
use crate::{NewSession, rail, sidebar};

/// How tall it is.
pub const HEIGHT: f32 = 44.;
/// The room the window's own buttons take at its left (macOS's traffic lights).
const LIGHTS: f32 = 84.;

pub fn render(workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let c = cx.theme().colors.clone();
    let (title, beside) = workspace.title(cx);
    let in_sessions = workspace.area() == Area::Sessions && !workspace.client().gate().is_held();
    let archived = in_sessions && workspace.opened_is_archived();
    let more_menu = (in_sessions).then(|| workspace.opened().map(|session| session_more_menu(workspace, session, cx))).flatten();
    // Over the rail and the area's sidebar: the switcher, then back, forward and + at the
    // sidebar's edge (OpenBot's); with the sidebar folded, they sit together.
    let zone = rail::WIDTH + if workspace.sidebar_shown() { px(sidebar::WIDTH) } else { px(0.) };
    let nav = div()
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S0_5)
        .child(
            Button::icon_only("back", IconName::ArrowLeft, "Back")
                .ghost()
                .size(ButtonSize::Small)
                .tooltip("⌘[")
                .disabled(!workspace.can_go_back())
                .on_click(|_, window, cx| window.dispatch_action(Box::new(Back), cx)),
        )
        .child(
            Button::icon_only("forward", IconName::ArrowRight, "Forward")
                .ghost()
                .size(ButtonSize::Small)
                .tooltip("⌘]")
                .disabled(!workspace.can_go_forward())
                .on_click(|_, window, cx| window.dispatch_action(Box::new(Forward), cx)),
        )
        .child(
            Button::icon_only("new-session", IconName::Plus, "New Session")
                .ghost()
                .size(ButtonSize::Small)
                .tooltip("⌘N").disabled(workspace.client().gate().is_held())
                .on_click(|_, window, cx| window.dispatch_action(Box::new(NewSession), cx)),
        );
    let panel_button = (workspace.area() == Area::Sessions).then(|| {
        Button::icon_only("toggle-panel", IconName::PanelRight, if workspace.is_panel_open() { "Hide Panel" } else { "Show Panel" })
            .ghost()
            .size(ButtonSize::Small)
            .selected(workspace.is_panel_open())
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
            }
        })
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(space::S2)
                .min_w(zone)
                .h_full()
                .pl(px(LIGHTS))
                .pr(space::S2)
                .child(buttons(
                    Button::icon_only("toggle-sidebar", IconName::SidebarLeft, if workspace.sidebar_shown() { "Hide Sidebar" } else { "Show Sidebar" })
                        .ghost()
                        .size(ButtonSize::Small)
                        .selected(workspace.sidebar_shown())
                        .tooltip(if workspace.sidebar_shown() { "Hide Sidebar (⌘B)" } else { "Show Sidebar (⌘B)" })
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_sidebar(window, cx))),
                ))
                .child(buttons(computer_switcher(workspace, cx)))
                .when(workspace.sidebar_shown(), |zone| zone.child(div().flex_1()))
                .child(buttons(nav)),
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
                .children(title.map(|title| div().debug_selector(|| "window-title".into()).min_w_0().truncate().text_ui(TypeSize::Small).medium().text_color(c.text).child(title)))
                .children(beside.map(|beside| div().flex_none().text_ui(TypeSize::Small).text_color(c.text_faint).child(beside)))
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
        .size(ButtonSize::Small)
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
fn computer_switcher(workspace: &Workspace, cx: &Context<Workspace>) -> AnyElement {
    let c = &cx.theme().colors;
    let name = workspace.box_name();
    let (dot, said) = match workspace.box_state() {
        BoxState::Connected => (None, None),
        BoxState::Connecting => (Some(c.text_faint), Some("Connecting…".to_owned())),
        BoxState::Held => (None, None),
        BoxState::Unreachable(why) => (Some(c.danger), Some(why.clone())),
    };
    // A long name is cut, so back, forward and + stay at the sidebar's edge.
    let shown: gpui::SharedString = if name.chars().count() > 18 { format!("{}…", name.chars().take(17).collect::<String>()).into() } else { name.clone() };
    let trigger = Button::new("computer-switcher", shown).ghost().size(ButtonSize::Small).icon(IconName::Computer).icon_after(IconName::ChevronDown);
    let menu_name = name.clone();
    // Your other computers, as the box lists them (`kuvo/computers`), each with how it's reached.
    // A workspace by itself shows its one box: they're listed, and can't be switched to.
    let others: Vec<(String, bool, String)> = match workspace.computers() {
        Reading::Ready(computers) => computers.iter().filter(|computer| !computer.here).map(|computer| (computer.name.clone(), computer.is_mac(), computer_line(computer))).collect(),
        _ => Vec::new(),
    };
    let switching = workspace.switching().cloned();
    let this = cx.entity().downgrade();
    div()
        .flex()
        .items_center()
        .gap(space::S1)
        .child(
            PopoverMenu::new("computer-menu", trigger, move |_, cx| {
                let (name, said, others, switching, this) = (menu_name.clone(), said.clone(), others.clone(), switching.clone(), this.clone());
                cx.new(|cx| {
                    let mut entry = MenuEntry::new(name).icon(IconName::Computer).checked(true);
                    if let Some(said) = said {
                        entry = entry.detail(said);
                    }
                    let mut menu = Menu::new(cx).width(px(280.)).header("Shown in This Window").entry(entry);
                    match switching {
                        // Your computers as this app reaches them: one it reaches shows its
                        // sessions here; one it doesn't says why, and isn't picked.
                        Some(switching) => {
                            let rest: Vec<&Row> = switching.rows.iter().filter(|row| Some(row.key()) != switching.shown.as_deref() && !(switching.shown.is_none() && row.reach == Reach::Nearest)).collect();
                            if !rest.is_empty() {
                                menu = menu.header("Your Other Computers");
                            }
                            for row in rest {
                                let (key, this) = (row.key().to_owned(), this.clone());
                                let can = row.usable() && !key.is_empty();
                                menu = menu.entry(MenuEntry::new(row.computer.name.clone()).icon(if row.computer.is_mac() { IconName::Computer } else { IconName::Terminal }).detail(reach_line(row)).disabled(!can).on_select(move |_, cx| {
                                    this.update(cx, |_, cx| cx.emit(Switch::To(key.clone()))).ok();
                                }));
                            }
                            // Why your other computers aren't reached, in the box's words.
                            match switching.pairing {
                                Pairing::Waiting(why) | Pairing::Refused(why) => menu = menu.separator().entry(MenuEntry::new("Your other computers aren’t reached yet").detail(why).disabled(true)),
                                Pairing::Removed(why) => {
                                    let this = this.clone();
                                    menu = menu.separator().entry(MenuEntry::new("This app was removed from your devices").detail(why).disabled(true)).entry(MenuEntry::new("Connect This App Again").on_select(move |_, cx| {
                                        this.update(cx, |_, cx| cx.emit(Switch::PairAgain)).ok();
                                    }));
                                }
                                Pairing::Paired | Pairing::NotAsked => {}
                            }
                            menu.separator().entry(MenuEntry::new("Show Computers").shortcut("⌃4").on_select(|window, cx| window.dispatch_action(Box::new(ShowComputers), cx)))
                        }
                        None => {
                            if !others.is_empty() {
                                menu = menu.header("Your Other Computers");
                                for (name, mac, line) in others {
                                    menu = menu.entry(MenuEntry::new(name).icon(if mac { IconName::Computer } else { IconName::Terminal }).detail(line).disabled(true));
                                }
                            }
                            menu.separator()
                                .entry(MenuEntry::new("Switching to another computer").detail("not in this app yet").disabled(true))
                                .entry(MenuEntry::new("Show Computers").shortcut("⌃4").on_select(|window, cx| window.dispatch_action(Box::new(ShowComputers), cx)))
                        }
                    }
                })
            })
            .into_any_element(),
        )
        .children(dot.map(status_dot))
        .into_any_element()
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
        Reach::Apart => row.computer.reach_title(if row.computer.shared { "shared" } else { "apart" }),
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
        spinner("update-chip-spinner", IconSize::Nav, c.icon).into_any_element()
    } else {
        Icon::new(if held.attention { IconName::Warning } else { IconName::ArrowDown }).color(if held.attention { c.warning } else { c.icon }).into_any_element()
    };
    let chip = div()
        .id("update-chip")
        .debug_selector(|| "update-chip".into())
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S1_5)
        .h(px(26.))
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
        .child(div().debug_selector(|| "update-chip-label".into()).child(label))
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
                .child(div().debug_selector(|| "update-card-title".into()).min_w_0().text_ui(TypeSize::Small).medium().text_color(c.text).child(title)),
        )
        .children((!words.is_empty()).then(|| div().debug_selector(|| "update-card-words".into()).text_ui(TypeSize::Small).text_color(c.text_muted).child(words)))
        .children(why.filter(|_| confirm.is_none()).map(|why| div().debug_selector(|| "update-card-why".into()).text_ui(TypeSize::Caption).text_color(c.text_faint).child(why)))
        .children(workspace.update_card_error().map(|error| div().debug_selector(|| "update-card-error".into()).text_ui(TypeSize::Caption).text_color(c.danger).child(error.to_owned())))
        .children(buttons_row)
        .into_any_element()
}
