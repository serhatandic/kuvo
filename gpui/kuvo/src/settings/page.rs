//! Drawing Settings: a side's sidebar (whose it is, its search, its sections, the row to the other
//! side) and a page (its title, quiet group
//! labels, a card of rows for each group, each row's name and line on the left and its control on
//! the right), with what the box refused over it, a confirm, or the sign-in sheet.

use gpui::{
    AnyElement, App, AppContext as _, Context, Hsla, InteractiveElement, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, list,
    prelude::FluentBuilder as _, px, svg,
};
use kuvo_box::areas::Level;
use kuvo_ui::{
    ActiveTheme, Icon, IconName, IconSize, StyledType, TypeSize,
    button::{Button, ButtonSize},
    menu::{Menu, MenuEntry, PopoverMenu},
    row::{NavRow, SectionHeader},
    small::{Switch, spinner},
    theme::{radius, space},
};

use super::rows::{Act, Control, Leading, Meter, Push, Row, Tone};
use super::{AddMachine, Drawn, Item, PAGE_WIDTH, Pick, SettingsView, Side, Standing, box_key, yours_key};
use crate::areas::computer_line;
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
                    .trailing(div().flex().items_center().gap(space::S2).child(div().text_ui(TypeSize::Small).text_color(c.text_faint).child(later.to_string())).child(Icon::new(if listed { IconName::ChevronDown } else { IconName::ChevronRight }).color(c.icon)))
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
            Side::Yours => div().debug_selector(|| "settings-yours-title".into()).pl(space::S1).h(px(36.)).flex().items_center().text_size(px(17.)).medium().text_color(c.text).child("Your settings").into_any_element(),
            Side::Computer => self.picker(cx),
        };
        let foot = match side {
            Side::Yours => NavRow::new("settings-foot-row", "Computer settings").icon(IconName::Computer).trailing(
                div().flex().items_center().gap(space::S2).child(div().max_w(px(88.)).truncate().text_ui(TypeSize::Small).text_color(c.text_faint).child(name.clone())).child(Icon::new(IconName::ChevronRight).color(c.icon)),
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
            .when(self.sign_in.is_some() || self.confirm.is_some(), |d| d.child(div().id("settings-sidebar-dim").debug_selector(|| "settings-sidebar-dim".into()).absolute().inset_0().bg(dim(cx)).occlude()))
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
            .child(div().flex_1().min_w_0().truncate().text_size(px(17.)).medium().text_color(c.text).child(name))
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

    fn item(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let Some(item) = self.items.get(ix).cloned() else { return div().into_any_element() };
        let body: AnyElement = match item {
            Item::Title(title) => div().pt(space::S8).text_ui(TypeSize::Title).medium().text_color(c.text).child(title).into_any_element(),
            Item::Whose(words) => div().debug_selector(|| "settings-whose".into()).pt(space::S1).text_ui(TypeSize::Small).text_color(c.text_muted).child(words).into_any_element(),
            Item::Group { label, words } => div()
                .pt(space::S6)
                .pb(space::S2)
                .flex()
                .flex_col()
                .gap(space::S1)
                .when(!label.is_empty(), |d| d.child(div().text_ui(TypeSize::Body).text_color(c.text_muted).child(label)))
                .when(!words.is_empty(), |d| d.child(div().text_ui(TypeSize::Small).text_color(c.text_faint).child(words)))
                .into_any_element(),
            Item::Row { row, first, last } => self.row(*row, first, last, cx),
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
        // Each item in a column as wide as a page's cards, in the middle.
        div().w_full().flex().justify_center().px(space::S8).child(div().w_full().max_w(px(PAGE_WIDTH)).child(body)).into_any_element()
    }

    fn row(&mut self, row: Row, first: bool, last: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let Row { key, leading, title, detail, tone, why, control } = row;
        let (mark, letter) = (format!("{key}-mark"), format!("{key}-initial"));
        let leading: Option<AnyElement> = match leading {
            Leading::None => None,
            Leading::Mark(service) => Some(match kuvo_ui::icon::mark_path(&service) {
                Some(path) => div().debug_selector(move || mark).flex().flex_none().items_center().justify_center().size(px(28.)).rounded(radius::PILL).bg(c.sunken).child(svg().path(path).size(px(16.)).text_color(kuvo_ui::icon::mark_color(&service).unwrap_or(c.text))).into_any_element(),
                // A service a newer box names, with no mark here: its first letter.
                None => div().debug_selector(move || letter).child(initial(&service, cx)).into_any_element(),
            }),
            Leading::Initial(name) => Some(div().debug_selector(move || letter).child(initial(&name, cx)).into_any_element()),
        };
        let control = self.control(&key, control, cx);
        let selector = key.clone();
        let why_selector = format!("{key}-why");
        div()
            .id(SharedString::from(key))
            .debug_selector(move || selector)
            .w_full()
            .bg(c.card)
            .when(first, |d| d.rounded_t(radius::POPOVER).mt(px(0.)))
            .when(last, |d| d.rounded_b(radius::POPOVER))
            .shadow(if first && last { cx.theme().card_shadow() } else { Vec::new() })
            .px(space::S5)
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
                            .when(!title.is_empty(), |d| d.child(div().text_ui(TypeSize::Body).medium().text_color(c.text).child(title)))
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
                            })),
                    )
                    .child(div().flex_none().max_w(px(340.)).child(control)),
            )
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
            Control::Busy(words) => div().flex().items_center().gap(space::S2).text_ui(TypeSize::Small).text_color(c.text_muted).child(spinner(SharedString::from(format!("{key}-busy")), IconSize::Nav, c.text_muted)).child(words).into_any_element(),
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
            Control::Buttons(buttons) => div().flex().gap(space::S2).children(self.buttons(key, buttons, cx)).into_any_element(),
            Control::Field { more, lines, .. } => {
                let field = self.field(key).cloned();
                let selector = format!("{key}-field");
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(space::S2)
                    .child(div().debug_selector(move || selector).w(px(if lines.is_some() { 320. } else { 200. })).min_h(px(if let Some((min, _)) = lines { min as f32 * 22. + 12. } else { 32. })).px(space::S3).flex().items_center().rounded(radius::NAV).border_1().border_color(c.border_strong).bg(c.card).children(field.map(|field| div().flex_1().min_w_0().child(field))))
                    .child(div().flex().flex_wrap().gap(space::S2).children(self.buttons(key, more, cx)))
                    .into_any_element()
            }
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
                        .child(div().flex().flex_none().flex_col().child(svg().path(IconName::ChevronUp.path()).size(px(10.)).text_color(c.icon)).child(svg().path(IconName::ChevronDown.path()).size(px(10.)).text_color(c.icon)))
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
        }
    }
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
            let resets = meter.resets_at.map(|at| format!("Starts over in {}", time::until(at, now)));
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

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        // The window's own box held for its update: its settings wait under the window's notice.
        if self.side() == Side::Computer && self.computer_key().is_none() && self.home.gate().is_held() {
            return div().id("settings-page").debug_selector(|| "settings-held".into()).size_full().bg(c.sidebar);
        }
        if std::mem::take(&mut self.focus_confirm) && self.confirm.is_some() {
            window.focus(&self.confirm_focus, cx);
        }
        if self.dark != cx.theme().appearance.is_dark() {
            self.rebuild(cx);
        }
        let page = list(self.list.clone(), cx.processor(|this, ix, _, cx| this.item(ix, cx))).size_full();
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
            sheet(card, cx)
        });
        let sign_in = self.sign_in.clone().map(|sheet_view| sheet(sheet_view.into_any_element(), cx));
        div()
            .id("settings-page")
            .debug_selector(|| "settings-page".into())
            .relative()
            .size_full()
            .bg(c.sidebar)
            .child(div().size_full().flex().flex_col().children(problem).child(div().flex_1().min_h_0().child(page)))
            .children(confirm)
            .children(sign_in)
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
        .child(content)
        .into_any_element()
}
