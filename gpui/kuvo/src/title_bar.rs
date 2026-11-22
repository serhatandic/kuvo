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
pub const LIGHTS: f32 = 76.;

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
    let panel_button = workspace.panel_here().then(|| {
        Button::icon_only("toggle-panel", IconName::PanelRight, if workspace.is_panel_open() { "Hide Panel" } else { "Show Panel" })
            .ghost()
            .size(ButtonSize::Medium)
            .icon_size(IconSize::Large)
            .selected(workspace.is_panel_open())
            .tooltip("⌥⌘0")
            .on_click(cx.listener(|this, _, _, cx| this.toggle_panel(cx)))
    });
    // A bot's conversation, titled as Sessions titles one: its face, name and job here, and its
    // settings and Outputs at the right, where a session has its menu and Outputs.
    let bot_chat = workspace.bot_chat().map(|(_, view)| view.clone());
    let in_bot_chat = bot_chat.is_some();
    let bot_face = workspace.bot_chat_face(cx);
    let bot_settings = bot_chat.is_some().then(|| {
        Button::icon_only("bot-chat-settings", IconName::Settings, "Bot Settings")
            .ghost()
            .size(ButtonSize::Medium)
            .icon_size(IconSize::Large)
            .tooltip("Bot Settings")
            .on_click(cx.listener(|this, _, window, cx| this.close_bot_chat(window, cx)))
    });
    let bot_page = if workspace.area() == Area::Bots && workspace.bot_chat().is_none() {
        workspace.area_view(Area::Bots)
            .and_then(|area| area.read(cx).page())
            .filter(|page| page.read(cx).bot().is_some())
            .cloned()
    } else {
        None
    };
    let open_bot_chat = bot_page.map(|page| {
        Button::icon_only("bot-chat", IconName::Message, "Open bot chat").ghost().tooltip("Open bot chat")
            .on_click(move |_, _, cx| page.update(cx, |page, cx| page.act(crate::bots::Act::Chat, cx)))
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
                .children(bot_face.map(|face| div().debug_selector(|| "bot-chat-title".into()).flex_none().child(face)))
                .children(title.map(|title| div().debug_selector(|| "window-title".into()).min_w_0().truncate().when(in_bot_chat, |d| d.flex_none()).text_role(TextRole::WindowTitle).text_color(c.text).child(title)))
                // (Short beside a session's title; a bot's job truncates.)
                .children(beside.map(|beside| div().min_w_0().truncate().text_role(TextRole::WindowContext).text_color(c.text_faint).child(beside)))
                .when(archived, |d| d.child(div().flex_none().child(Chip::new("is-archived", "Archived"))))
                .child(div().flex_1())
                // The setup's update where the box says one (it takes the old chip's place), else
                // the old chip for a box from before it.
                .children(fleet_chip(workspace, cx))
                .children(workspace.fleet().is_none().then(|| update_chip(workspace, cx)).flatten())
                .children(more_menu)
                .children(bot_settings.map(buttons))
                .child(notifications_bell(workspace, cx))
                .children(in_sessions.then(|| workspace.conversation()).flatten().cloned().or(bot_chat).map(|view| buttons(view.read(cx).bars().clone())))
                .children(open_bot_chat.map(buttons))
                .children(panel_button.map(buttons)),
        )
        .into_any_element()
}

/// The title bar's notifications: what lasts or wants something of you (a move that failed, one
/// that went through), each a plain row with its one action. A dot on the bell while any wait;
/// each goes by itself once it's done or, if it was only news, once it's been seen.
fn notifications_bell(workspace: &Workspace, cx: &Context<Workspace>) -> AnyElement {
    let theme = cx.theme().clone();
    let c = &theme.colors;
    let open = workspace.notifications_open();
    let moves = workspace.moves().clone();
    let count = moves.read(cx).notice_count();
    let bell = Button::icon_only("notifications", IconName::Bell, "Notifications")
        .ghost()
        .size(ButtonSize::Medium)
        .icon_size(IconSize::Large)
        .selected(open)
        .tooltip("Notifications")
        .on_click(cx.listener(|this, _, _, cx| this.toggle_notifications(cx)));
    let dot = (count > 0).then(|| div().debug_selector(|| "notifications-dot".into()).absolute().top(px(4.)).right(px(4.)).child(status_dot(c.accent)));
    let card = open.then(|| {
        let rows = crate::moves::MovesView::listed(&moves, cx);
        let empty = rows.is_empty().then(|| div().px(space::S2).py(px(7.)).text_ui(TypeSize::Small).text_color(c.text_muted).child("Nothing needs you."));
        let card = div()
            .id("notifications-card")
            .debug_selector(|| "notifications-card".into())
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_notifications(cx)))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.close_notifications(cx);
                }
            }))
            .w(px(396.))
            .max_h(px(420.))
            .overflow_y_scroll()
            .p(space::S3)
            .rounded(radius::POPOVER)
            .bg(c.elevated)
            .border_1()
            .border_color(c.border)
            .shadow(theme.elevated_shadow())
            .text_ui(TypeSize::Small)
            .text_color(c.text)
            .flex()
            .flex_col()
            .gap(px(2.))
            // Its header as the Outputs popover's sections have theirs.
            .child(
                div()
                    .h(px(24.))
                    .px(space::S2)
                    .flex()
                    .items_center()
                    .gap(space::S1_5)
                    .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child("Notifications"))
                    .children((count > 0).then(|| div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(count.to_string()))),
            )
            .children(empty)
            .children(rows);
        title_anchored_popover(card.into_any_element())
    });
    div().relative().flex_none().child(buttons(bell)).children(dot).children(card).into_any_element()
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
    // Away: the box's word for it on its name, muted ("Can’t be reached"), not a red dot; a quiet
    // dot where no list says it.
    let away = match (workspace.box_state(), workspace.computers()) {
        (BoxState::Unreachable(_), Reading::Ready(computers)) => computers.iter().find(|computer| computer.here).map(|computer| computer.reach_title("away")).filter(|words| !words.is_empty()),
        _ => None,
    };
    let dot = match workspace.box_state() {
        BoxState::Connecting => Some(c.text_faint),
        BoxState::Unreachable(_) if away.is_none() || compact => Some(c.text_faint),
        BoxState::Connected | BoxState::Held | BoxState::Unreachable(_) => None,
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
        .children(away.filter(|_| !compact).map(|words| kuvo_ui::notice::on_name(words, cx)))
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
        .children(workspace.update_card_error().map(|error| kuvo_ui::Notice::said("update-card-error", kuvo_ui::Tone::Error, error.to_owned())))
        .children(buttons_row)
        .into_any_element()
}

/// The setup’s update, with the title supplied by the box.
fn fleet_chip(workspace: &Workspace, cx: &Context<Workspace>) -> Option<AnyElement> {
    let summary = workspace.fleet_header()?.clone();
    let c = cx.theme().colors.clone();
    let open = workspace.fleet_card_open();
    let failed = summary.warning == Some(true);
    let leading = if failed {
        Icon::new(IconName::Warning).size(IconSize::Nav).color(c.warning).into_any_element()
    } else {
        spinner("fleet-chip-spinner", IconSize::Nav, c.accent).into_any_element()
    };
    let chip = div()
        .id("fleet-chip")
        .debug_selector(|| "fleet-chip".into())
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S1_5)
        .h(px(28.))
        .px(space::S2)
        .rounded(radius::PILL)
        .border_1()
        .border_color(if failed { c.warning.opacity(0.45) } else { c.border })
        .bg(if open { c.selected } else { c.surface })
        .hover(|s| s.bg(c.hover))
        .cursor_pointer()
        .text_ui(TypeSize::Small)
        .text_color(c.text)
        .child(leading)
        .child(div().debug_selector(|| "fleet-chip-label".into()).medium().child(crate::fleet::summary_words(&summary)))
        .on_click(cx.listener(|this, _, _, cx| this.toggle_fleet_card(cx)));
    let card = open.then(|| crate::session_menu::title_anchored_popover(fleet_card(workspace, &summary, cx)));
    Some(div().relative().flex_none().child(buttons(chip)).children(card).into_any_element())
}

/// Each computer’s update progress, with Try Again only where the box says it can be fixed.
fn fleet_card(workspace: &Workspace, summary: &kuvo_box::areas::FleetUpdateSummary, cx: &Context<Workspace>) -> AnyElement {
    use crate::fleet::Tone;
    let theme = cx.theme().clone();
    let c = &theme.colors;
    let rows = workspace.fleet_rows();
    let retrying = workspace.fleet_retrying().map(str::to_owned);
    let segments = rows.iter().map(|row| {
        let color = match row.standing.line.tone {
            Tone::Done => c.success,
            Tone::Working => c.accent,
            Tone::Warning | Tone::Blocked => c.warning,
            Tone::Off => c.border_strong,
        };
        div().flex_1().rounded(radius::PILL).bg(color)
    });
    let count = rows.len();
    let busy = retrying.is_some();
    let computer_rows = rows.iter().enumerate().map(|(ix, row)| {
        let key = row.key.clone();
        let button = row.standing.can_update.then(|| {
            let busy = retrying.as_deref() == Some(key.as_str());
            let selector = format!("fleet-card-update-{key}");
            div().debug_selector(move || selector).flex_none().child(
                Button::new(SharedString::from(format!("fleet-card-update-{key}")), "Try Again")
                    .size(ButtonSize::Small)
                    .primary()
                    .loading(busy)
                    .disabled(retrying.is_some())
                    .on_click(cx.listener(move |this, _, _, cx| this.fleet_retry(&key, cx))),
            )
        });
        div()
            .w_full()
            .flex()
            .items_center()
            .gap(space::S2)
            .py(space::S2)
            .when(ix + 1 < count, |d| d.border_b_1().border_color(c.border))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(space::S0_5)
                    .child(div().truncate().text_ui(TypeSize::Small).medium().text_color(c.text).child(row.name.clone()))
                    .child(crate::fleet::status_line(&format!("card-{}", row.key), &row.standing.line, cx)),
            )
            .children(button)
    });
    div()
        .id("fleet-card")
        .debug_selector(|| "fleet-card".into())
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_fleet_card(cx)))
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
            if event.keystroke.key == "escape" {
                cx.stop_propagation();
                this.close_fleet_card(cx);
            }
        }))
        .w(px(420.))
        .flex()
        .flex_col()
        .gap(space::S3)
        .p(space::S4)
        .rounded(radius::POPOVER)
        .bg(c.elevated)
        .border_1()
        .border_color(c.border_strong)
        .shadow(theme.elevated_shadow())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::S0_5)
                .child(div().debug_selector(|| "fleet-card-title".into()).text_ui(TypeSize::Body).medium().text_color(c.text).child(crate::fleet::summary_words(summary)))
                // (The box's line under it: why, in amber, only where you can fix it.)
                .children(summary.detail.as_ref().map(|words| {
                    if summary.warning == Some(true) {
                        kuvo_ui::Notice::inline("fleet-card-detail-warning", kuvo_ui::Tone::Warning, words.clone()).into_any_element()
                    } else {
                        div().debug_selector(|| "fleet-card-detail".into()).text_ui(TypeSize::Caption).text_color(c.text_muted).child(words.clone()).into_any_element()
                    }
                })),
        )
        .when(count > 0, |d| d.child(div().w_full().h(px(6.)).flex().gap(px(4.)).children(segments)))
        // (Many computers: the list scrolls, the summary and the buttons stay.)
        .child(div().id("fleet-card-rows").max_h(px(360.)).overflow_y_scroll().flex().flex_col().children(computer_rows))
        .children(workspace.fleet_error().map(|error| kuvo_ui::Notice::said("fleet-card-error", kuvo_ui::Tone::Error, error.to_owned())))
        .child(
            div()
                .pt(space::S2)
                .border_t_1()
                .border_color(c.border)
                .flex()
                .items_center()
                .gap(space::S2)
                .child(div().flex_1())
                .child(
                    div()
                        .flex_none()
                        .child(div().debug_selector(|| "fleet-card-open".into()).child(Button::new("fleet-card-open", "Open Computers").size(ButtonSize::Small).disabled(busy).on_click(cx.listener(|this, _, window, cx| this.fleet_open_computers(None, window, cx))))),
                ),
        )
        .into_any_element()
}
