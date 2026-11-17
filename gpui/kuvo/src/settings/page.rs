//! Drawing Settings: a side's sidebar (whose it is, its search, its sections, the row to the other
//! side) and a page (its title, quiet group
//! labels, a card of rows for each group, each row's name and line on the left and its control on
//! the right), with what the box refused over it, a confirm, or the sign-in sheet.

use std::{cell::Cell, rc::Rc};

use kuvo_ui::motion::{MotionExt as _, Direction};
use gpui::{
    AnyElement, App, AppContext as _, Context, Focusable as _, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled, Window, canvas, div, list,
    prelude::FluentBuilder as _, px,
};
use kuvo_box::areas::{Level, UsageMeter};
use kuvo_ui::{
    ActiveTheme, Icon, IconName, IconSize, StyledType, TextRole, TypeSize,
    button::{Button, ButtonSize},
    menu::{Menu, MenuEntry, PopoverMenu},
    row::{NavRow, SectionHeader},
    small::{Switch, spinner},
    theme::{radius, space},
};

use super::rows::{Act, Control, FlowDrawn, Layout, Leading, Meter, Part, Push, Row, Tone};
use kuvo_box::settings_pages::{BadgeTone, FlowState};
use super::usage_columns::{PAGE_WIDTH as USAGE_PAGE_WIDTH, UsageColumns};
use super::{AddMachine, Drawn, Item, PAGE_WIDTH, Pick, SettingsView, Side, Standing, box_key, yours_key};
use crate::areas::{Reading, computer_line};
use crate::time;

/// How wide the computer picker and its menu are: the sidebar's, inside its padding.
const PICKER_WIDTH: f32 = crate::sidebar::WIDTH - 24.;

impl SettingsView {
    /// The sidebar of the side shown: whose settings these are (a computer's is its picker), the
    /// search, the sections it finds, what it finds on the other side, and the row to the other.
    pub fn sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let side = self.side();
        let found = self.sections_found(side);
        let open = self.section().to_owned();
        let away = side == Side::Computer && self.standing != Standing::Reached;
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut group = String::new();
        // What isn't in the desktop app yet is one row, which opens its short list (open too
        // while one of them is shown, or a search finds one).
        let later = found.iter().filter(|entry| entry.drawn == Drawn::Later).count();
        let listed = self.later_open() || self.is_searching() || found.iter().any(|entry| entry.drawn == Drawn::Later && entry.id == open);
        for entry in &found {
            if entry.drawn == Drawn::Later && entry.group != group {
                group = entry.group.clone();
                let fold = NavRow::new("settings-later-row", super::LATER)
                    .muted(true)
                    .trailing(div().flex().items_center().gap(space::S2).child(div().text_role(TextRole::Meta).text_color(c.text_faint).child(later.to_string())).child(Icon::new(if listed { IconName::ChevronDown } else { IconName::ChevronRight }).color(c.icon)))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_later(cx)));
                rows.push(div().debug_selector(|| "settings-later".into()).pt(space::S3).pb(space::S0_5).child(fold).into_any_element());
            }
            if entry.drawn == Drawn::Later && !listed {
                continue;
            }
            // A computer's sections under their headings (General, first, has none of its own).
            if entry.group != group {
                group = entry.group.clone();
                if !group.is_empty() && group != "General" {
                    rows.push(SectionHeader::new(SharedString::from(format!("settings-group-{group}")), group.clone()).into_any_element());
                }
            }
            let selector = format!("setting-{}", entry.id);
            let place = if side == Side::Yours { yours_key(&entry.id) } else { box_key(self.computer_key().unwrap_or_default(), &entry.id) };
            let row = NavRow::new(SharedString::from(selector.clone()), entry.title.clone()).icon(entry.icon).muted(entry.drawn == Drawn::Later && entry.id != open).on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(Pick { place: place.clone(), record: true });
            }));
            // The open section under the theme's own selected wash (not a raised card, #723).
            let washed = div().rounded(radius::NAV).when(entry.id == open, |d| d.bg(c.selected)).child(row);
            rows.push(div().debug_selector(move || selector).pb(space::S0_5).child(washed).into_any_element());
        }
        // What the search finds on the other side, so typing in the wrong one still gets there.
        let hits = self.hits();
        let name = self.computer_name();
        if !hits.is_empty() {
            let heading = if side == Side::Yours { format!("Computer · {name}") } else { "Your settings".to_owned() };
            rows.push(SectionHeader::new("settings-other-side", heading).into_any_element());
            for (ix, hit) in hits.into_iter().enumerate() {
                let selector = format!("settings-hit-{ix}");
                let place = hit.place.clone();
                let row = NavRow::new(SharedString::from(selector.clone()), hit.title).icon(hit.icon).trailing(Icon::new(IconName::ArrowRight).color(c.icon)).on_click(cx.listener(move |this, _, _, cx| this.act(Act::Go(place.clone()), cx)));
                rows.push(div().debug_selector(move || selector).pb(space::S0_5).child(row).into_any_element());
            }
        }
        let quiet = |words: String| div().px(space::S2).pt(space::S2).text_ui(TypeSize::Small).text_color(c.text_muted).child(words).into_any_element();
        if rows.is_empty() && self.is_searching() {
            rows.push(quiet("No settings match.".into()));
        }
        if let Some(why) = self.sections_problem() {
            rows.push(div().debug_selector(|| "settings-sections-problem".into()).child(quiet(format!("{name}’s other settings couldn’t be read: {why}"))).into_any_element());
        }
        let title: AnyElement = match side {
            Side::Yours => div().debug_selector(|| "settings-yours-title".into()).pl(space::S1).h(px(36.)).flex().items_center().text_role(TextRole::PaneTitle).text_color(c.text).child("Your settings").into_any_element(),
            Side::Computer => self.picker(cx),
        };
        let foot = match side {
            Side::Yours => NavRow::new("settings-foot-row", "Computer settings").icon(IconName::Computer).trailing(
                div().flex().items_center().gap(space::S2).child(div().max_w(px(88.)).truncate().text_role(TextRole::Meta).text_color(c.text_faint).child(name.clone())).child(Icon::new(IconName::ChevronRight).color(c.icon)),
            ),
            Side::Computer => NavRow::new("settings-foot-row", "Your settings").icon(IconName::User).trailing(Icon::new(IconName::ChevronRight).color(c.icon)),
        }
        .on_click(cx.listener(|this, _, _, cx| this.switch_side(cx)));
        div()
            .id("settings-sidebar")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div().flex_none().px(space::S3).pt(space::S3).pb(space::S3).flex().flex_col().gap(space::S2).child(title).when(!away, |d| {
                    d.child(
                        div()
                            .debug_selector(|| "settings-search".into())
                            .flex()
                            .items_center()
                            .gap(space::S2)
                            .h(px(36.))
                            .mx(space::S1)
                            .px(space::S3)
                            .rounded(radius::POPOVER)
                            .bg(c.background)
                            .border_1()
                            .border_color(c.border)
                            .child(Icon::new(IconName::Search).color(c.icon))
                            .child(div().flex_1().min_w_0().child(self.search.clone())),
                    )
                }),
            )
            .child(div().id("settings-sections").flex_1().min_h_0().overflow_y_scroll().px(space::S2).pb(space::S4).children(rows))
            .child(div().flex_none().border_t_1().border_color(c.border).px(space::S2).py(space::S2).child(div().debug_selector(|| "settings-foot".into()).child(foot)))
            // A sign-in or a confirm open: the sidebar waits under the same dimming as the page.
            .when(self.sign_in.is_some() || self.confirm.is_some() || self.folder_picker.is_some(), |d| d.child(div().id("settings-sidebar-dim").debug_selector(|| "settings-sidebar-dim".into()).absolute().inset_0().bg(dim(cx)).occlude()))
            .into_any_element()
    }

    /// A computer's sidebar is titled with its picker: the computer whose settings these are,
    /// which opens your computers (one that isn't reached quieter, still picked) and Add a Machine….
    fn picker(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let (name, kind) = (self.computer_name(), self.computer_kind());
        let trigger = div()
            .id("settings-computer")
            .debug_selector(|| "settings-computer".into())
            .w(px(PICKER_WIDTH))
            .h(px(36.))
            .px(space::S2)
            .flex()
            .items_center()
            .gap(space::S2)
            .rounded(radius::NAV)
            .cursor_pointer()
            .hover(|d| d.bg(c.hover))
            .child(Icon::new(IconName::Computer).color(c.icon))
            .child(div().flex_1().min_w_0().truncate().text_role(TextRole::PaneTitle).text_color(c.text).child(name))
            .child(div().flex_none().text_ui(TypeSize::Small).text_color(c.text_faint).child(kind))
            .child(Icon::new(IconName::ChevronDown).color(c.icon))
            .into_any_element();
        let build = self.picker_menu(cx);
        PopoverMenu::new("settings-computer-menu", trigger, move |_, cx| build(cx)).with_handle(self.picker.clone()).into_any_element()
    }

    /// What makes the picker's menu, from your computers as they're listed now.
    fn picker_menu(&self, cx: &mut Context<Self>) -> impl Fn(&mut App) -> gpui::Entity<Menu> + 'static {
        let computers = self.listed_computers();
        let shown = self.computer_key().map(str::to_owned);
        let here = self.this_box().to_owned();
        let this = cx.entity().downgrade();
        move |cx| {
            let (computers, shown, this, here) = (computers.clone(), shown.clone(), this.clone(), here.clone());
            cx.new(|cx| {
                let mut menu = Menu::new(cx).width(px(PICKER_WIDTH)).header("Settings Of");
                for computer in computers {
                    let checked = if computer.here { shown.is_none() } else { shown.as_deref() == Some(computer.key.as_str()) };
                    let key = (!computer.here).then(|| computer.key.clone());
                    let this = this.clone();
                    let reached = computer.here || computer.reach == "connected" && !computer.is_older();
                    // (The window's own by the name the title bar gives it.)
                    let (name, line) = if computer.here { (here.clone(), "This window’s".to_owned()) } else { (computer.name.clone(), computer_line(&computer)) };
                    menu = menu.entry(MenuEntry::new(name).icon(if computer.is_mac() { IconName::Computer } else { IconName::Terminal }).detail(line).checked(checked).muted(!reached).on_select(move |_, cx| {
                        this.update(cx, |this, cx| this.pick_computer(key.clone(), cx)).ok();
                    }));
                }
                let this = this.clone();
                menu.separator().entry(MenuEntry::new("Add a Machine…").icon(IconName::Plus).on_select(move |_, cx| {
                    this.update(cx, |_, cx| cx.emit(AddMachine)).ok();
                }))
            })
        }
    }

    /// Opens the picker (for the shots, which press nothing).
    pub fn open_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let menu = self.picker_menu(cx)(cx);
        self.picker.open(menu, window, cx);
    }

    fn item(&mut self, ix: usize, page_width: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let Some(item) = self.items.get(ix).cloned() else { return div().into_any_element() };
        let body: AnyElement = match item {
            Item::Title(title) => div().pt(space::S8).text_ui(TypeSize::Title).medium().text_color(c.text).child(title).into_any_element(),
            Item::Whose(words) => div().debug_selector(|| "settings-whose".into()).pt(space::S1).text_ui(TypeSize::Small).text_color(c.text_muted).child(words).into_any_element(),
            Item::UsageSummary => {
                let busy = self.usage_refreshing();
                let words = if busy {
                    "Refreshing…".to_owned()
                } else {
                    self.usage_updated_at().map(|at| usage_updated(at, self.now.unwrap_or_else(time::now_ms))).unwrap_or_default()
                };
                div().flex().items_center().justify_between().gap(space::S2).pt(space::S0_5).pb(space::S2)
                    .child(div().debug_selector(|| "usage-updated".into()).text_ui(TypeSize::Small).text_color(c.text_muted).child(words))
                    .child(Button::icon_only("usage-refresh", IconName::Refresh, "Refresh usage").ghost().size(ButtonSize::Small)
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.refresh_usage(cx))))
                    .into_any_element()
            }
            Item::UsageCards(rows) => {
                let width = (page_width - 2. * space::S8).clamp(px(0.), px(USAGE_PAGE_WIDTH));
                let cards = rows.into_iter().map(|row| {
                    let Control::Usage { account, .. } = &row.control else { unreachable!("usage account") };
                    (account.clone(), self.row(row, true, true, cx))
                }).collect();
                UsageColumns { width, placement: self.usage_placement.clone(), cards }.into_any_element()
            }
            Item::Group { label, words } => div()
                .pt(space::S6)
                .pb(space::S2)
                .flex()
                .flex_col()
                .gap(space::S1)
                .when(!label.is_empty(), |d| d.child(div().text_ui(TypeSize::Body).medium().text_color(c.text).child(label)))
                .when(!words.is_empty(), |d| d.child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(words)))
                .into_any_element(),
            Item::Row { row, first, last } => self.row(*row, first, last, cx),
            Item::Note { key, words, busy: true, .. } => {
                let first = self.items.iter().find_map(|item| match item { Item::Note { key, busy: true, .. } => Some(key), _ => None });
                if first == Some(&key) {
                    kuvo_ui::small::LoadingLabel::new(gpui::SharedString::from(key), words).into_any_element()
                } else {
                    div().into_any_element()
                }
            },
            Item::Note { key, words, busy, warning } => div()
                .debug_selector(move || key)
                .pt(space::S4)
                .flex()
                .items_start()
                .gap(space::S2)
                .text_ui(TypeSize::Small)
                .text_color(if warning { c.warning } else { c.text_muted })
                .children(busy.then(|| div().pt(px(3.)).child(spinner("settings-reading", IconSize::Nav, c.text_muted))))
                .child(div().flex_1().min_w_0().child(words))
                .into_any_element(),
            Item::Nothing(query) => div().pt(space::S4).text_ui(TypeSize::Body).text_color(c.text_muted).child(format!("Nothing on this page matches “{query}”.")).into_any_element(),
            Item::Aside { key, icon, title, words } => div()
                .debug_selector(move || key.into())
                .h(px(560.))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(space::S3)
                .child(div().flex().items_center().justify_center().size(px(48.)).rounded(radius::PILL).bg(c.sunken).child(Icon::new(icon).size(IconSize::Card).color(c.icon)))
                .child(div().text_center().text_ui(TypeSize::Title).medium().text_color(c.text).child(title))
                .child(div().max_w(px(420.)).text_center().text_ui(TypeSize::Body).text_color(c.text_muted).child(words))
                .into_any_element(),
            Item::End => div().h(space::S10).into_any_element(),
        };
        // Usage has room for three cards. The other settings pages use the usual column.
        let width = if self.side() == Side::Computer && self.section() == "usage" { USAGE_PAGE_WIDTH } else { PAGE_WIDTH };
        div().w_full().flex().justify_center().px(space::S8).child(div().w_full().max_w(px(width)).child(body)).into_any_element()
    }

    fn row(&mut self, row: Row, first: bool, last: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        if row.layout == Layout::Bare {
            return self.bare_row(row, cx);
        }
        let Row { key, leading, title, detail, tone, why, control, badge, notice, flow, aside, .. } = row;
        let tile = if matches!(&control, Control::Usage { .. }) { px(22.) } else { px(28.) };
        let (mark, letter) = (format!("{key}-mark"), format!("{key}-initial"));
        let leading: Option<AnyElement> = match leading {
            Leading::None => None,
            Leading::Mark(service) => Some(match kuvo_ui::icon::Mark::of_service(&service) {
                Some(drawn) => div().debug_selector(move || mark).flex().flex_none().items_center().justify_center().size(tile).rounded(radius::PILL).bg(c.sunken).child(drawn.in_tile(tile)).into_any_element(),
                // A service a newer box names, with no mark here: its first letter.
                None => div().debug_selector(move || letter).child(initial(&service, cx)).into_any_element(),
            }),
            Leading::Initial(name) => Some(div().debug_selector(move || letter).child(initial(&name, cx)).into_any_element()),
        };
        // A card of rows, each its own item: the card's edge drawn a side at a time, a hairline
        // between rows.
        let card = |d: gpui::Stateful<gpui::Div>| {
            d.w_full()
                .bg(c.card)
                .border_color(c.border)
                .border_l_1()
                .border_r_1()
                .when(first, |d| d.rounded_t(radius::POPOVER).border_t_1())
                .when(last, |d| d.rounded_b(radius::POPOVER).border_b_1())
                .px(space::S5)
        };
        if let Control::Usage { plan, meters, .. } = &control {
            let selector = key.clone();
            let now = self.now.unwrap_or_else(time::now_ms);
            let plan = plan.as_ref().filter(|plan| !plan.is_empty()).map(|plan| {
                div().flex_none().max_w(px(110.)).truncate().rounded(radius::PILL).bg(c.sunken)
                    .px(space::S1_5).text_ui(TypeSize::Caption).text_color(c.text_muted).child(plan.clone())
            });
            return div().id(SharedString::from(key.clone())).debug_selector(move || selector)
                    .w_full().min_w_0()
                    .rounded(radius::POPOVER).border_1().border_color(c.border).bg(c.card)
                    .px(space::S3).py(space::S2).flex().flex_col().gap(space::S1_5)
                    .child(
                        div().flex().items_center().gap(space::S2).children(leading)
                            .child(div().min_w_0().truncate().text_ui(TypeSize::Body).medium().text_color(c.text).child(title))
                            .children(plan)
                            .when(!detail.is_empty(), |d| d.child(div().ml_auto().min_w_0().truncate().text_ui(TypeSize::Caption).text_color(c.text_muted).child(detail))),
                    )
                    .children(why.map(|words| {
                        div().text_ui(TypeSize::Small).text_color(if tone == Tone::Warning { c.warning } else { c.text_muted }).child(words)
                    }))
                    .when(!meters.is_empty(), |d| d.child(usage_meters(&key, meters, now, self.usage_elapsed(), cx)))
                    .into_any_element();
        }
        if matches!(&control, Control::Field { save: Act::Filter, .. }) {
            let selector = key.clone();
            let control = self.control(&key, control, cx);
            return card(div().id(SharedString::from(key)).debug_selector(move || selector)).child(div().py(space::S3).when(!last, |d| d.border_b_1().border_color(c.border)).child(control)).into_any_element();
        }
        let badge = badge.map(|(words, tone)| badge_of(&key, words, tone, cx));
        let notice = notice.map(|(words, tone)| {
            let selector = format!("{key}-notice");
            div()
                .debug_selector(move || selector)
                .pt(space::S0_5)
                .flex()
                .items_start()
                .gap(space::S1_5)
                .text_ui(TypeSize::Small)
                .text_color(c.text_muted)
                .child(div().pt(px(2.)).child(Icon::new(if tone == Tone::Warning { IconName::Warning } else { IconName::Info }).size(IconSize::Nav).color(if tone == Tone::Warning { c.warning } else { c.text_faint })))
                .child(div().flex_1().min_w_0().line_clamp(3).child(words))
        });
        let flow = flow.map(|flow| self.flow(&key, flow, cx));
        // A row that opens a page is pressed whole, its › at the end.
        let opens = match &control {
            Control::Opens(act) => Some(act.clone()),
            _ => None,
        };
        let control = match control {
            Control::Opens(_) => Icon::new(IconName::ChevronRight).color(c.icon).into_any_element(),
            control => self.control(&key, control, cx),
        };
        let aside = aside.map(|words| {
            let selector = format!("{key}-aside");
            div().debug_selector(move || selector).flex_none().text_ui(TypeSize::Body).text_color(c.text_muted).child(words)
        });
        let selector = key.clone();
        let why_selector = format!("{key}-why");
        card(div().id(SharedString::from(key)).debug_selector(move || selector))
            .when_some(opens, |d, act| d.cursor_pointer().hover(|d| d.bg(c.hover)).on_click(cx.listener(move |this, _, _, cx| this.act(act.clone(), cx))))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S3)
                    .py(space::S4)
                    .when(!last, |d| d.border_b_1().border_color(c.border))
                    .children(leading)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(space::S0_5)
                            .when(!title.is_empty() || badge.is_some(), |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(space::S2)
                                        .min_w_0()
                                        .child(div().min_w_0().truncate().text_ui(TypeSize::Body).medium().text_color(c.text).child(title))
                                        .children(badge),
                                )
                            })
                            .when(!detail.is_empty(), |d| d.child(div().text_ui(TypeSize::Small).text_color(if tone == Tone::Warning { c.warning } else { c.text_muted }).child(detail)))
                            // Why it can't be changed from here, in the box's words: drawn, never hidden.
                            .children(why.map(|why| {
                                div()
                                    .debug_selector(move || why_selector)
                                    .pt(space::S0_5)
                                    .flex()
                                    .items_start()
                                    .gap(space::S1)
                                    .text_ui(TypeSize::Small)
                                    .text_color(c.text_faint)
                                    .child(div().pt(px(2.)).child(Icon::new(IconName::Info).size(IconSize::Nav).color(c.text_faint)))
                                    .child(div().flex_1().min_w_0().child(why))
                            }))
                            .children(notice)
                            .children(flow),
                    )
                    .children(aside)
                    .child(div().flex_none().max_w(px(360.)).child(control)),
            )
            .into_any_element()
    }

    /// A row on the page itself: a group's one multi-line field, full width; or a whole by its
    /// parts, its title and actions over its bar.
    fn bare_row(&mut self, row: Row, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let Row { key, title, detail, tone, why, control, .. } = row;
        let selector = key.clone();
        let why = why.map(|why| div().debug_selector({ let key = key.clone(); move || format!("{key}-why") }).pt(space::S2).text_ui(TypeSize::Small).text_color(c.text_faint).child(why));
        let body: AnyElement = match control {
            Control::Parts { parts, actions, busy, open, expand } => {
                let buttons = self.buttons(&key, actions, cx);
                let header = div()
                    .flex()
                    .items_center()
                    .gap(space::S3)
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(space::S0_5).child(div().debug_selector({ let key = key.clone(); move || format!("{key}-title") }).text_ui(TypeSize::Body).medium().text_color(c.text).child(title)).when(!detail.is_empty(), |d| d.child(div().text_ui(TypeSize::Small).text_color(if tone == Tone::Warning { c.warning } else { c.text_muted }).child(detail))))
                    .children(busy.map(|words| div().debug_selector({ let key = key.clone(); move || format!("{key}-busy") }).flex().items_center().gap(space::S1_5).text_ui(TypeSize::Small).text_color(c.text_muted).child(spinner(SharedString::from(format!("{key}-busy-spinner")), IconSize::Nav, c.text_muted)).child(div().id(SharedString::from(format!("{key}-busy-label"))).child(words).pulsing())))
                    .children(buttons);
                div().flex().flex_col().gap(space::S3).child(header).child(parts_drawn(&key, &parts, open, expand, cx)).into_any_element()
            }
            control => {
                let field = self.field(&key).cloned();
                let busy = match &control {
                    Control::Field { busy, .. } => busy.clone(),
                    _ => None,
                };
                let lines = match &control {
                    Control::Field { lines, .. } => *lines,
                    _ => None,
                };
                match field {
                    Some(field) => div()
                        .debug_selector({ let key = key.clone(); move || format!("{key}-field") })
                        .on_mouse_down_out({
                            let leave = field.focus_handle(cx);
                            move |_, window, cx| if leave.is_focused(window) { window.blur(cx) }
                        })
                        .relative()
                        .w_full()
                        .min_h(px(lines.map_or(32., |(min, _)| min as f32 * 22. + 24.)))
                        .px(space::S4)
                        .py(space::S3)
                        .rounded(radius::POPOVER)
                        .border_1()
                        .border_color(c.border_strong)
                        .bg(c.card)
                        .child(field)
                        .children(busy.map(|_| div().absolute().top(space::S3).right(space::S3).child(spinner(SharedString::from(format!("{key}-busy")), IconSize::Nav, c.text_muted))))
                        .into_any_element(),
                    None => self.control(&key, control, cx),
                }
            }
        };
        div().id(SharedString::from(key)).debug_selector(move || selector).w_full().child(body).children(why).into_any_element()
    }

    /// A flow the box leads for a row, under it: a spinner and the box's words while it waits, a
    /// check once done, a warning once failed; its buttons; and where it has one, the field to
    /// paste the address its page ended on.
    fn flow(&mut self, key: &str, flow: FlowDrawn, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let selector = format!("{key}-flow");
        let state: AnyElement = match flow.state {
            FlowState::Waiting => spinner(SharedString::from(format!("{key}-flow-spinner")), IconSize::Nav, c.text_muted).into_any_element(),
            FlowState::Done => Icon::new(IconName::Check).size(IconSize::Nav).color(c.success).into_any_element(),
            FlowState::Failed => Icon::new(IconName::Warning).size(IconSize::Nav).color(c.warning).into_any_element(),
        };
        let code = flow.code.clone().and_then(|(_, title, act)| self.code_field(key).cloned().map(|field| (field, title, act)));
        let buttons = self.buttons(&format!("{key}-flow"), flow.buttons, cx);
        div()
            .debug_selector(move || selector)
            .mt(space::S2)
            .p(space::S3)
            .rounded(radius::NAV)
            .bg(c.sunken)
            .flex()
            .flex_col()
            .gap(space::S2)
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(space::S2)
                    .child(div().pt(px(2.)).flex_none().child(state))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(space::S0_5)
                            .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child(flow.title))
                            .when(!flow.line.is_empty(), |d| d.child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(flow.line))),
                    ),
            )
            .children(code.map(|(field, title, act)| {
                let go = SharedString::from(format!("{key}-flow-code-go"));
                div()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h(px(30.))
                            .px(space::S3)
                            .flex()
                            .items_center()
                            .rounded(radius::NAV)
                            .border_1()
                            .border_color(c.border_strong)
                            .bg(c.card)
                            .child(div().flex_1().min_w_0().child(field)),
                    )
                    .child(Button::new(go, title).size(ButtonSize::Small).on_click(cx.listener(move |this, _, _, cx| this.act(act.clone(), cx))))
            }))
            .when(!buttons.is_empty(), |d| d.child(div().flex().flex_wrap().gap(space::S2).children(buttons)))
            .into_any_element()
    }

    fn buttons(&mut self, key: &str, buttons: Vec<Push>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        buttons
            .into_iter()
            .map(|push| {
                let id = SharedString::from(format!("{key}-{}", push.label.to_lowercase().replace([' ', '…'], "-").trim_end_matches('-')));
                // (One that takes something away is drawn as the rest: what asks first is the
                // confirm, whose button is the destructive one.)
                let button = Button::new(id, push.label).size(ButtonSize::Small);
                match push.act {
                    Some(act) => button.on_click(cx.listener(move |this, _, _, cx| this.act(act.clone(), cx))).into_any_element(),
                    None => button.disabled(true).into_any_element(),
                }
            })
            .collect()
    }

    fn control(&mut self, key: &str, control: Control, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        match control {
            Control::None => div().into_any_element(),
            Control::Text(words) => div().text_ui(TypeSize::Body).text_color(c.text_muted).child(words).into_any_element(),
            Control::Busy(words) => div().flex().items_center().gap(space::S2).text_ui(TypeSize::Small).text_color(c.text_muted).child(spinner(SharedString::from(format!("{key}-busy")), IconSize::Nav, c.text_muted)).child(div().id(SharedString::from(format!("{key}-busy-label"))).child(words).pulsing()).into_any_element(),
            Control::Switch { on, act } => {
                let this = cx.entity().downgrade();
                let switch = Switch::new(SharedString::from(format!("{key}-switch")), on).disabled(act.is_none());
                match act {
                    Some(act) => switch
                        .on_change(move |_, _, cx| {
                            this.update(cx, |this, cx| this.act(act.clone(), cx)).ok();
                        })
                        .into_any_element(),
                    None => switch.into_any_element(),
                }
            }
            Control::Model { shown, current, unset, path, was } => {
                let (this, kept) = (cx.entity().downgrade(), cx.entity().downgrade());
                let client = self.client.clone();
                crate::model_picker::setting_button(
                    key,
                    shown,
                    move |window, cx| {
                        // (What the box last said, read as it opens rather than copied as it's drawn.)
                        let offered = this
                            .upgrade()
                            .and_then(|view| match &view.read(cx).offered {
                                Reading::Ready(offered) => Some(offered.clone()),
                                _ => None,
                            })
                            .unwrap_or_default();
                        // (The engine's models: these are the engine's own settings.)
                        let catalog = crate::model_picker::Catalog::new(&offered, |model| model.side == kuvo_box::models::Side::Engine).with_unset(unset.clone(), "", current.is_none());
                        let (this, kept, client, current, starred, was) = (this.clone(), kept.clone(), client.clone(), current.clone(), offered.starred.clone(), was.clone());
                        cx.new(|cx| {
                            crate::model_picker::ModelPicker::new(
                                std::sync::Arc::new(catalog),
                                starred,
                                current,
                                client,
                                std::rc::Rc::new(move |picked: Option<kuvo_box::typed::ModelRef>, _, cx| {
                                    let model = picked.map(|picked| format!("{}/{}", picked.provider_id, picked.id));
                                    if model != was {
                                        this.update(cx, |this, cx| this.act(Act::SetModel { path, model, was: was.clone() }, cx)).ok();
                                    }
                                }),
                                window,
                                cx,
                            )
                            .on_starred(std::rc::Rc::new(move |starred, cx| {
                                kept.update(cx, |this, _| {
                                    if let Reading::Ready(offered) = &mut this.offered {
                                        offered.starred = Some(starred);
                                    }
                                })
                                .ok();
                            }))
                        })
                    },
                    cx,
                )
            }
            Control::Buttons(buttons) => div().flex().gap(space::S2).children(self.buttons(key, buttons, cx)).into_any_element(),
            Control::Field { more, lines, unit, save, browse, picks, busy, .. } => {
                let field = self.field(key).cloned();
                let leave = field.as_ref().map(|field| field.focus_handle(cx));
                let selector = format!("{key}-field");
                let search = matches!(save, Act::Filter);
                let stacked = lines.is_some();
                // In the field, at its end: a change on its way, common values to pick, a folder to
                // choose.
                let busy = busy.map(|_| spinner(SharedString::from(format!("{key}-busy")), IconSize::Nav, c.text_muted).into_any_element());
                let picks = (!picks.is_empty()).then(|| {
                    let choices = picks.into_iter().map(|push| (push.label, None, push.act, false)).collect();
                    self.picks_menu(&format!("{key}-picks"), choices, cx)
                });
                let browse = browse.map(|act| {
                    Button::icon_only(SharedString::from(format!("{key}-browse")), IconName::Folder, "Choose a Folder…").ghost().size(ButtonSize::Small).tooltip("Choose a Folder…").on_click(cx.listener(move |this, _, _, cx| this.act(act.clone(), cx))).into_any_element()
                });
                let input = div()
                    .debug_selector(move || selector)
                    // A press anywhere else leaves it (it's saved then), as on any page.
                    .when_some(leave, |d, leave| d.on_mouse_down_out(move |_, window, cx| if leave.is_focused(window) { window.blur(cx) }))
                    .when(search, |d| d.w_full())
                    .when(!search, |d| d.w(px(if stacked || browse.is_some() { 320. } else if unit.is_some() { 150. } else { 260. })))
                    .min_h(px(lines.map_or(32., |(min, _)| min as f32 * 22. + 12.)))
                    .pl(space::S3)
                    .pr(if browse.is_some() || picks.is_some() { space::S1 } else { space::S3 })
                    .flex()
                    .when(stacked, |d| d.py(space::S2).items_start())
                    .when(!stacked, |d| d.items_center())
                    .gap(space::S2)
                    .rounded(radius::NAV)
                    .border_1()
                    .border_color(c.border_strong)
                    .bg(c.card)
                    .children(search.then(|| Icon::new(IconName::Search).size(IconSize::Nav).color(c.text_faint)))
                    .children(field.map(|field| div().flex_1().min_w_0().child(field)))
                    .children(unit.map(|unit| div().flex_none().text_ui(TypeSize::Small).text_color(c.text_muted).child(unit)))
                    .children(busy)
                    .children(picks)
                    .children(browse);
                div().flex().items_center().when(search, |d| d.w_full()).gap(space::S2).child(input).children(self.buttons(key, more, cx)).into_any_element()
            }
            Control::Segments(choices) => {
                let this = cx.entity().downgrade();
                div()
                    .debug_selector({ let key = key.to_owned(); move || format!("{key}-segments") })
                    .flex()
                    .items_center()
                    .gap(space::S0_5)
                    .p(px(2.))
                    .rounded(radius::NAV)
                    .bg(c.sunken)
                    .children(choices.into_iter().enumerate().map(|(ix, (label, act, chosen))| {
                        let selector = format!("{key}-segment-{ix}");
                        let this = this.clone();
                        div()
                            .id(SharedString::from(selector.clone()))
                            .debug_selector(move || selector)
                            .px(space::S3)
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .rounded(radius::SMALL)
                            .text_ui(TypeSize::Small)
                            .when(chosen, |d| d.bg(c.card).text_color(c.text).medium().shadow(cx.theme().card_shadow()))
                            .when(!chosen, |d| d.text_color(c.text_muted))
                            .when(!chosen && act.is_some(), |d| d.cursor_pointer().hover(|d| d.text_color(c.text)))
                            .child(label)
                            .when_some(act.filter(|_| !chosen), |d, act| {
                                d.on_click(move |_, _, cx| {
                                    this.update(cx, |this, cx| this.act(act.clone(), cx)).ok();
                                })
                            })
                    }))
                    .into_any_element()
            }
            Control::Parts { parts, open, expand, .. } => parts_drawn(key, &parts, open, expand, cx),
            Control::Opens(act) => Button::icon_only(SharedString::from(format!("{key}-open")), IconName::ChevronRight, "Open").ghost().size(ButtonSize::Small).on_click(cx.listener(move |this, _, _, cx| this.act(act.clone(), cx))).into_any_element(),
            Control::Menu { shown, choices } => {
                let this = cx.entity().downgrade();
                let id = SharedString::from(format!("{key}-menu"));
                // A menu of actions shows "…"; a setting's shows what's chosen (a dash for none).
                let trigger: AnyElement = match shown {
                    None => Button::icon_only(id.clone(), IconName::More, "More").ghost().size(ButtonSize::Small).into_any_element(),
                    Some(shown) => div()
                        .id(id.clone())
                        .debug_selector({
                            let id = id.to_string();
                            move || id
                        })
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .min_w(px(160.))
                        .h(px(32.))
                        .px(space::S3)
                        .rounded(radius::NAV)
                        .border_1()
                        .border_color(c.border_strong)
                        .bg(c.card)
                        .cursor_pointer()
                        .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Body).text_color(c.text).child(if shown.is_empty() { "—".to_owned() } else { shown }))
                        .child(div().flex().flex_none().flex_col().child(Icon::new(IconName::ChevronUp).size(IconSize::Indicator).color(c.icon)).child(Icon::new(IconName::ChevronDown).size(IconSize::Indicator).color(c.icon)))
                        .into_any_element(),
                };
                let wide = choices.iter().any(|(_, line, _, _)| line.is_some());
                PopoverMenu::new(SharedString::from(format!("{key}-popover")), trigger, move |_, cx| {
                    let (choices, this) = (choices.clone(), this.clone());
                    cx.new(|cx| {
                        let mut menu = Menu::new(cx).width(px(if wide { 420. } else { 240. }));
                        for (label, line, act, checked) in choices {
                            let this = this.clone();
                            let mut entry = MenuEntry::new(label).checked(checked);
                            entry = match act {
                                Some(act) => entry.on_select(move |_, cx| {
                                    this.update(cx, |this, cx| this.act(act.clone(), cx)).ok();
                                }),
                                None => entry.disabled(true),
                            };
                            if let Some(line) = line {
                                entry = entry.detail(line);
                            }
                            menu = menu.entry(entry);
                        }
                        menu
                    })
                })
                .align_right()
                .into_any_element()
            }
            Control::Windows(meters) => windows(key, &meters, self.now.unwrap_or_else(time::now_ms), cx),
            Control::Usage { .. } => unreachable!("usage meters are drawn in their account row"),
            Control::Thing { buttons, menu, switch } => {
                let menu = (!menu.is_empty()).then(|| self.control(key, Control::Menu { shown: None, choices: menu.into_iter().map(|push| (push.label, None, push.act, false)).collect() }, cx));
                let switch = switch.map(|(on, act)| self.control(key, Control::Switch { on, act }, cx));
                div().flex().items_center().gap(space::S2).children(self.buttons(key, buttons, cx)).children(menu).children(switch).into_any_element()
            }
        }
    }
}

impl SettingsView {
    /// A field's common values, under a small chevron at its end.
    fn picks_menu(&mut self, id: &str, choices: Vec<(String, Option<String>, Option<Act>, bool)>, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        let trigger = Button::icon_only(SharedString::from(format!("{id}-menu")), IconName::ChevronDown, "Common values").ghost().size(ButtonSize::Small).into_any_element();
        PopoverMenu::new(SharedString::from(format!("{id}-popover")), trigger, move |_, cx| {
            let (choices, this) = (choices.clone(), this.clone());
            cx.new(|cx| {
                let mut menu = Menu::new(cx).width(px(200.));
                for (label, _, act, _) in choices {
                    let this = this.clone();
                    menu = menu.entry(match act {
                        Some(act) => MenuEntry::new(label).on_select(move |_, cx| {
                            this.update(cx, |this, cx| this.act(act.clone(), cx)).ok();
                        }),
                        None => MenuEntry::new(label).disabled(true),
                    });
                }
                menu
            })
        })
        .align_right()
        .into_any_element()
    }
}

/// The colours a whole's parts are drawn in, in order: the last a neutral grey, for what's left.
fn part_color(ix: usize, count: usize, cx: &App) -> Hsla {
    let c = &cx.theme().colors;
    let dark = cx.theme().appearance.is_dark();
    if ix + 1 == count && count > 1 {
        return c.text_faint;
    }
    let (lightness, saturation) = if dark { (0.62, 0.70) } else { (0.52, 0.72) };
    match ix % 4 {
        0 => gpui::hsla(214. / 360., saturation, lightness, 1.),
        1 => gpui::hsla(146. / 360., saturation - 0.15, lightness - 0.06, 1.),
        2 => gpui::hsla(38. / 360., 0.88, lightness + 0.04, 1.),
        _ => gpui::hsla(268. / 360., saturation - 0.12, lightness + 0.04, 1.),
    }
}

/// A whole by its parts: one bar of them, each its colour and share; under it each one's dot,
/// name and words; and, where they're made of more, a breakdown that opens.
fn parts_drawn(key: &str, parts: &[Part], open: bool, expand: Act, cx: &mut Context<SettingsView>) -> AnyElement {
    let c = cx.theme().colors.clone();
    let count = parts.len();
    // (A part too small to see still shows a sliver.)
    let bar = div()
        .debug_selector({ let key = key.to_owned(); move || format!("{key}-bar") })
        .h(px(10.))
        .w_full()
        .flex()
        .gap(px(2.))
        .rounded(radius::PILL)
        .overflow_hidden()
        .bg(c.border)
        .children(parts.iter().enumerate().filter(|(_, part)| part.share > 0.).map(|(ix, part)| div().h_full().flex_basis(gpui::relative(part.share.max(0.004))).min_w(px(3.)).bg(part_color(ix, count, cx))));
    let legend = div().flex().flex_wrap().gap_x(space::S6).gap_y(space::S2).children(parts.iter().enumerate().map(|(ix, part)| {
        let selector = format!("{key}-part-{ix}");
        div()
            .debug_selector(move || selector)
            .flex()
            .flex_col()
            .gap(space::S0_5)
            .child(div().flex().items_center().gap(space::S1_5).text_ui(TypeSize::Small).text_color(c.text_muted).child(div().size(px(8.)).rounded(radius::PILL).bg(part_color(ix, count, cx))).child(part.title.clone()))
            .child(div().pl(px(14.)).text_ui(TypeSize::Small).text_color(c.text).child(part.words.clone()))
    }));
    let made_of: Vec<&Part> = parts.iter().filter(|part| !part.parts.is_empty()).collect();
    let breakdown = (!made_of.is_empty()).then(|| {
        let toggle = div()
            .id(SharedString::from(format!("{key}-breakdown")))
            .debug_selector({ let key = key.to_owned(); move || format!("{key}-breakdown") })
            .flex()
            .items_center()
            .gap(space::S1)
            .cursor_pointer()
            .text_ui(TypeSize::Small)
            .text_color(c.text_muted)
            .hover(|d| d.text_color(c.text))
            .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::Nav).color(c.icon))
            .child("Breakdown")
            .on_click(cx.listener(move |this, _, _, cx| this.act(expand.clone(), cx)));
        let list = open.then(|| {
            div()
                .debug_selector({ let key = key.to_owned(); move || format!("{key}-breakdown-list") })
                .mt(space::S1)
                .flex()
                .flex_col()
                .rounded(radius::POPOVER)
                .border_1()
                .border_color(c.border)
                .bg(c.card)
                .px(space::S5)
                .children(made_of.iter().flat_map(|part| {
                    let heading = div().pt(space::S3).pb(space::S1).text_ui(TypeSize::Small).medium().text_color(c.text).child(part.title.clone()).into_any_element();
                    std::iter::once(heading).chain(part.parts.iter().map(|(title, words)| {
                        div().flex().items_center().justify_between().gap(space::S3).py(space::S1).text_ui(TypeSize::Small).child(div().min_w_0().truncate().text_color(c.text_muted).child(title.clone())).child(div().flex_none().text_color(c.text).child(words.clone())).into_any_element()
                    }))
                }))
                .child(div().h(space::S3))
        });
        div().flex().flex_col().child(toggle).children(list)
    });
    div().flex().flex_col().gap(space::S3).child(bar).child(legend).children(breakdown).into_any_element()
}

/// How a thing stands, beside its name: a dot of its tone and the box's words.
fn badge_of(key: &str, words: String, tone: BadgeTone, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let selector = format!("{key}-badge");
    let dot = match tone {
        BadgeTone::Good => c.success,
        BadgeTone::Warning => c.warning,
        BadgeTone::Failed => c.danger,
        BadgeTone::Busy | BadgeTone::Quiet => c.text_faint,
    };
    div()
        .debug_selector(move || selector)
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S1)
        .text_ui(TypeSize::Small)
        .text_color(c.text_muted)
        .child(if tone == BadgeTone::Busy { spinner(SharedString::from(format!("{key}-badge-busy")), IconSize::Nav, c.text_muted).into_any_element() } else { kuvo_ui::small::status_dot(dot).into_any_element() })
        .child(words)
        .into_any_element()
}

fn initial(name: &str, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let letter = name.chars().next().map(|first| first.to_uppercase().to_string()).unwrap_or_default();
    div().flex().flex_none().items_center().justify_center().size(px(28.)).rounded(radius::PILL).bg(c.sunken).text_ui(TypeSize::Small).medium().text_color(c.text_muted).child(letter).into_any_element()
}

/// Windows of use, as the rail's hover card shows them: each one's name, share and meter, and
/// when it starts over.
fn windows(key: &str, meters: &[Meter], now: f64, cx: &App) -> AnyElement {
    let c = cx.theme().colors.clone();
    div()
        .w(px(260.))
        .flex()
        .flex_col()
        .gap(space::S3)
        .children(meters.iter().enumerate().map(|(ix, meter)| {
            let selector = format!("{key}-window-{ix}");
            // Its colour is the box's level, as the rail's rings.
            let color = crate::rail::level_color(meter.level, cx);
            let calm = matches!(meter.level, Level::Calm | Level::Unknown);
            let resets = meter.resets_at.map(|at| format!("Resets in {}", time::until(at, now)));
            div()
                .debug_selector(move || selector)
                .flex()
                .flex_col()
                .gap(space::S1)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(space::S2)
                        .text_ui(TypeSize::Small)
                        .child(div().min_w_0().truncate().text_color(c.text).child(meter.title.clone()))
                        .child(div().flex_none().text_color(if calm { c.text_muted } else { color }).child(meter.said.clone())),
                )
                .child(div().h(px(4.)).w_full().rounded(radius::PILL).bg(c.border).child(div().h_full().w(gpui::relative(meter.share)).rounded(radius::PILL).bg(color)))
                .children(resets.map(|resets| div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(resets)))
        }))
        .into_any_element()
}

fn usage_updated(at: f64, now: f64) -> String {
    let minutes = ((now - at) / 60_000.).max(0.) as u64;
    match minutes {
        0 => "Updated just now".into(),
        1..60 => format!("Updated {minutes} min ago"),
        60..1440 => format!("Updated {} h ago", minutes / 60),
        _ => format!("Updated {} d ago", minutes / 1440),
    }
}

fn usage_reset(at: Option<f64>, now: f64) -> String {
    let Some(at) = at else { return "Reset time unavailable".into() };
    let minutes = (at - now) / 60_000.;
    match minutes.floor() as i64 {
        ..=-1 => "Reset due".into(),
        0 if minutes == 0. => "Reset due".into(),
        0..60 => format!("resets in {} min", minutes.ceil() as u64),
        60..1440 => format!("resets in {} h", (minutes / 60.) as u64),
        _ => {
            let seconds = (at / 1000.) as i64;
            let local_day = (seconds + crate::pasted::utc_offset(seconds)).div_euclid(86_400);
            let weekday = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][local_day.rem_euclid(7) as usize];
            format!("resets {weekday} {}", time::clock(at))
        }
    }
}

/// Stack full-width meters so all tracks have the same length.
fn usage_meters(key: &str, meters: &[UsageMeter], now: f64, elapsed: std::time::Duration, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .flex()
        .flex_col()
        .gap(space::S2)
        .children(meters.iter().enumerate().map(|(ix, meter)| {
            let selector = format!("{key}-window-{ix}");
            let old = meter.old_at(elapsed);
            let color = if old || matches!(meter.level, Level::Calm | Level::Unknown) { c.text_muted } else { crate::rail::level_color(meter.level, cx) };
            let detail = if old { format!("old {}", time::when(meter.read_at, now)) } else { usage_reset(meter.resets_at, now) };
            div()
                .debug_selector(move || selector)
                .min_w_0()
                .flex()
                .flex_col()
                .gap(space::S0_5)
                .child(div().truncate().text_ui(TypeSize::Small).text_color(c.text).child(meter.title.clone()))
                .child(div().h(px(3.)).w_full().rounded(radius::PILL).bg(c.border)
                    .child(div().h_full().w(gpui::relative(meter.share)).rounded(radius::PILL).bg(color)))
                .child(div().flex().gap(space::S1).min_w_0().text_ui(TypeSize::Caption)
                    .child(div().flex_none().text_color(color).child(meter.value.clone()))
                    .child(div().min_w_0().truncate().text_color(if old { c.warning } else { c.text_muted }).child(format!("· {detail}"))))
        }))
        .into_any_element()
}

#[cfg(test)]
mod usage_tests {
    use super::*;

    #[test]
    fn usage_time_labels_are_short_and_local() {
        assert_eq!(usage_updated(0., 120_000.), "Updated 2 min ago");
        assert_eq!(usage_reset(Some(3. * 3_600_000.), 0.), "resets in 3 h");
        let noon = 86_400 + 43_200;
        let afternoon = (noon - crate::pasted::utc_offset(noon)) as f64 * 1000.;
        let reset = usage_reset(Some(afternoon), afternoon - 2. * 86_400_000.);
        assert!(reset.starts_with("resets Fri "));
        assert!(reset.ends_with(&time::clock(afternoon)));
        assert!(!reset.contains("UTC"));
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        // The window's own box held for its update: its settings wait under the window's notice.
        if self.side() == Side::Computer && self.computer_key().is_none() && self.home.gate().is_held() {
            return div().id("settings-page").debug_selector(|| "settings-held".into()).size_full().bg(c.sidebar);
        }
        if std::mem::take(&mut self.focus_confirm) && (self.confirm.is_some() || self.folder_picker.is_some()) {
            window.focus(&self.confirm_focus, cx);
        }
        if self.dark != cx.theme().appearance.is_dark() {
            self.rebuild(cx);
        }
        self.hear_blurs(window, cx);
        let loading = self.items.iter().all(|item| matches!(item, Item::Title(_) | Item::End | Item::Note { busy: true, .. } | Item::Whose(_)))
            .then(|| self.items.iter().find_map(|item| match item { Item::Note { words, busy: true, .. } => Some(words.clone()), _ => None })).flatten();
        let page_width = Rc::new(Cell::new(px(0.)));
        let measured_width = page_width.clone();
        let page = match loading {
            Some(words) => div().size_full().flex().items_center().justify_center().child(kuvo_ui::small::LoadingLabel::new("page-loading", words)).into_any_element(),
            None => list(self.list.clone(), cx.processor(move |this, ix, _, cx| this.item(ix, measured_width.get(), cx))).size_full().into_any_element(),
        };
        let problem = self.problem.clone().map(|problem| {
            // Over the page, which starts under it: never over its title.
            div()
                .flex_none()
                .pt(space::S4)
                .flex()
                .justify_center()
                .px(space::S8)
                .child(
                    div()
                        .debug_selector(|| "settings-problem".into())
                        .max_w(px(PAGE_WIDTH))
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(space::S3)
                        .px(space::S4)
                        .py(space::S3)
                        .rounded(radius::NAV)
                        .bg(c.card)
                        .border_1()
                        .border_color(c.danger)
                        .shadow(cx.theme().card_shadow())
                        .child(Icon::new(IconName::Warning).color(c.danger))
                        .child(div().flex_1().min_w_0().text_ui(TypeSize::Small).text_color(c.text).child(problem))
                        .child(Button::icon_only("settings-problem-close", IconName::Close, "Close").ghost().size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| {
                            this.problem = None;
                            cx.notify();
                        }))),
                )
        });
        let confirm = self.confirm.clone().map(|confirm| {
            let card = confirm_card(
                ConfirmIds { card: "settings-confirm", cancel: "confirm-cancel", go: "confirm-go" },
                &self.confirm_focus,
                (confirm.title, confirm.body, confirm.button),
                cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                    this.confirm = None;
                    cx.notify();
                }),
                cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        this.confirm = None;
                        cx.notify();
                    }
                }),
                cx.listener(|this, _: &gpui::ClickEvent, _, cx| this.confirmed(cx)),
                cx,
            );
            sheet_keyed(card, "settings-confirm", cx)
        });
        let folder_picker = self.folder_picker.clone().map(|picker| {
            let up = picker.folder.as_ref().and_then(|folder| folder.parent.clone().filter(|parent| *parent != folder.path)).or_else(|| picker.back.clone());
            let mut up_button = Button::icon_only("settings-folder-parent", IconName::ArrowUp, "Parent folder")
                .ghost()
                .size(ButtonSize::Small)
                .disabled(up.is_none());
            if let Some(parent) = up {
                up_button = up_button.on_click(cx.listener(move |this, _, _, cx| this.browse_folder(parent.clone(), cx)));
            }
            let body: AnyElement = if let Some(problem) = picker.problem {
                div().text_ui(TypeSize::Small).text_color(c.danger).child(problem).into_any_element()
            } else if let Some(folder) = &picker.folder {
                if folder.folders.is_empty() {
                    div().px(space::S1).py(space::S2).text_ui(TypeSize::Small).text_color(c.text_muted).child("No folders in here.").into_any_element()
                } else {
                    let count = folder.folders.len();
                    let rows: Vec<_> = folder
                        .folders
                        .iter()
                        .enumerate()
                        .map(|(ix, named)| {
                            let path = format!("{}/{}", folder.path.trim_end_matches('/'), named.name);
                            let last = ix + 1 == count;
                            let selector = format!("settings-folder-{ix}");
                            div()
                                .id(("settings-folder", ix))
                                .debug_selector(move || selector)
                                .flex()
                                .items_center()
                                .gap(space::S3)
                                .px(space::S4)
                                .py(space::S2)
                                .when(!last, |d| d.border_b_1().border_color(c.border))
                                .hover(|s| s.bg(c.hover))
                                .cursor_pointer()
                                .child(Icon::new(if named.repository { IconName::Branch } else { IconName::Folder }).size(IconSize::Nav).color(c.icon))
                                .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text).child(named.name.clone()))
                                .child(Icon::new(IconName::ChevronRight).size(IconSize::Nav).color(c.text_faint))
                                .on_click(cx.listener(move |this, _, _, cx| this.browse_folder(path.clone(), cx)))
                                .into_any_element()
                        })
                        .collect();
                    div()
                        .rounded(radius::NAV)
                        .border_1()
                        .border_color(c.border)
                        .bg(c.card)
                        .overflow_hidden()
                        .child(div().id("settings-folder-list").max_h(px(280.)).overflow_y_scroll().flex().flex_col().children(rows))
                        .into_any_element()
                }
            } else {
                kuvo_ui::small::LoadingLabel::new("settings-folder-reading", "Reading this folder…").compact().into_any_element()
            };
            let ready = picker.folder.is_some();
            let at = picker.at;
            let for_form = picker.for_form.clone();
            let path = picker.path.clone();
            let card = div()
                .id("settings-folder-picker")
                .debug_selector(|| "settings-folder-picker".into())
                .track_focus(&self.confirm_focus)
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        this.folder_picker = None;
                        cx.notify();
                    }
                }))
                .w(px(480.))
                .p(space::S6)
                .flex()
                .flex_col()
                .gap(space::S3)
                .rounded(radius::CARD)
                .bg(c.elevated)
                .border_1()
                .border_color(c.border)
                .shadow(cx.theme().elevated_shadow())
                .child(div().text_ui(TypeSize::Body).medium().text_color(c.text).child(picker.words.choose))
                .when(!picker.words.message.is_empty(), |d| d.child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(picker.words.message)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .child(up_button)
                        .child(Icon::new(IconName::Folder).size(IconSize::Nav).color(c.icon))
                        .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Small).medium().text_color(c.text).child(picker.path)),
                )
                .child(body)
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(space::S2)
                        .pt(space::S2)
                        .child(Button::new("settings-folder-cancel", "Cancel").on_click(cx.listener(|this, _, _, cx| {
                            this.folder_picker = None;
                            cx.notify();
                        })))
                        .child(Button::new("settings-folder-choose", picker.words.prompt).primary().disabled(!ready).on_click(cx.listener(move |this, _, _, cx| {
                            this.folder_picker = None;
                            match (&for_form, this.form().cloned()) {
                                (Some(field), Some(form)) => form.update(cx, |form, cx| form.chose_folder(field, &path, cx)),
                                // (Its form closed meanwhile: nothing to put it in.)
                                (Some(_), None) => {}
                                (None, _) => this.act(Act::Change { at: at.clone(), value: serde_json::json!(path), asked: false }, cx),
                            }
                            cx.notify();
                        }))),
                )
                .into_any_element();
            sheet_keyed(card, "settings-folder-picker", cx)
        });
        let sign_in = self.sign_in.clone().map(|sheet_view| sheet_keyed(sheet_view.clone().into_any_element(), ("sign-in", sheet_view.entity_id().as_u64()), cx));
        let form = self.form().cloned().map(|form| sheet_keyed(form.clone().into_any_element(), ("form", form.entity_id().as_u64()), cx));
        div()
            .id("settings-page")
            .debug_selector(|| "settings-page".into())
            .relative()
            .size_full()
            .bg(c.sidebar)
            // Prepaint this before the list asks for Usage cards, including after a sidebar resize.
            .when(self.side() == Side::Computer && self.section() == "usage", |d| {
                d.child(canvas(move |bounds, _, _| page_width.set(bounds.size.width), |_, _, _, _| {})
                    .absolute().size_full())
            })
            .child(div().size_full().flex().flex_col().children(problem).child(div().flex_1().min_h_0().child(page)))
            .children(confirm)
            .children(sign_in)
            .children(form)
            .children(folder_picker)
    }
}

/// The debug selectors of a confirm's card and its two buttons.
pub(crate) struct ConfirmIds {
    pub card: &'static str,
    pub cancel: &'static str,
    pub go: &'static str,
}

/// A confirm as a box words it (its question, what it comes to, the button that goes ahead), drawn
/// alike wherever one is asked: Cancel, Escape, and the destructive button.
pub(crate) fn confirm_card(
    ids: ConfirmIds,
    focus: &gpui::FocusHandle,
    (title, message, label): (String, String, String),
    cancel: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    escape: impl Fn(&gpui::KeyDownEvent, &mut Window, &mut App) + 'static,
    go: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let c = &cx.theme().colors;
    let card = ids.card;
    div()
        .id(card)
        .debug_selector(move || card.into())
        .track_focus(focus)
        .on_key_down(escape)
        .w(px(400.))
        .flex()
        .flex_col()
        .gap(space::S3)
        .p(space::S6)
        .rounded(radius::CARD)
        .bg(c.elevated)
        .border_1()
        .border_color(c.border)
        .shadow(cx.theme().elevated_shadow())
        .child(div().text_ui(TypeSize::Body).medium().text_color(c.text).child(title))
        .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(message))
        .child(div().flex().justify_end().gap(space::S2).pt(space::S2).child(Button::new(ids.cancel, "Cancel").on_click(cancel)).child(Button::new(ids.go, label).destructive().on_click(go)))
        .into_any_element()
}

/// What's under a sheet is dimmed by.
pub(crate) fn dim(cx: &App) -> Hsla {
    if cx.theme().appearance.is_dark() { gpui::hsla(0., 0., 0., 0.45) } else { gpui::hsla(0., 0., 0., 0.12) }
}

/// A sheet over the page, on a dimmed backdrop that takes the clicks.
pub(crate) fn sheet(content: AnyElement, cx: &App) -> AnyElement {
    sheet_keyed(content, "sheet-content", cx)
}

fn sheet_keyed(content: AnyElement, id: impl Into<gpui::ElementId>, cx: &App) -> AnyElement {
    let dim = dim(cx);
    div()
        .id("settings-sheet")
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .p(space::S6)
        .bg(dim)
        .occlude()
        .child(div().id((id.into(), "settings-sheet-content")).child(content).animate_in(Direction::FromTop, true))
        .into_any_element()
}
