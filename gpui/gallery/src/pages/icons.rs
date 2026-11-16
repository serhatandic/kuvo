use gpui::{AnyElement, App, Context, InteractiveElement, IntoElement, ParentElement, Styled, Window, div, prelude::FluentBuilder as _, px, svg};
use kuvo_ui::{
    ActiveTheme, Icon, IconName, IconSize, StyledType, TextRole, TypeSize,
    row::{NavRow, SectionHeader},
    small::{count_badge, status_words},
    theme::{radius, space},
};

use crate::{Gallery, page as frame, section};

/// The navigation and toolbar icons the reviews compared: the rail's areas and the title bar's.
const NAVIGATION: [IconName; 5] = [IconName::Bot, IconName::Message, IconName::Queue, IconName::ArrowLeft, IconName::Plus];

/// The sizes they're drawn at, and where.
const SIZES: [(IconSize, &str); 4] = [(IconSize::Rail, "22 · rail"), (IconSize::Large, "18 · title bar"), (IconSize::Medium, "16 · toolbars"), (IconSize::Nav, "14 · rows")];

pub fn page(_window: &mut Window, cx: &mut Context<Gallery>) -> AnyElement {
    frame(
        "Icons and type",
        "Lucide line icons on a 16-unit grid, every size drawn with one line on screen, and the type ramp by role: what a sidebar, a row and the title bar set their words with.",
        vec![
            // (kuvo_ui::icon::STROKE_PX, as the sizes' test holds it.)
            section("One line at every size: 1.4 px on screen", weights(cx), cx),
            section("Where each size goes", in_place(cx), cx),
            section("The type ramp, by role", ramp(cx), cx),
            section("In a sidebar", in_use(cx), cx),
            section("Every icon, at 14 px", grid(cx), cx),
        ],
        cx,
    )
}

/// The same icons at the four sizes: as the files drew them (their stroke growing with the size),
/// and as they're drawn now.
fn weights(cx: &App) -> impl IntoElement {
    let c = &cx.theme().colors;
    let label = |words: &'static str| div().w(px(132.)).flex_none().text_role(TextRole::Meta).text_color(c.text_faint).child(words);
    let line = |name: &'static str, now: bool| {
        div().flex().items_center().gap(space::S6).child(label(name)).children(SIZES.iter().map(move |(size, _)| {
            div().flex().items_center().gap(space::S1).w(px(200.)).children(NAVIGATION.iter().map(|&icon| {
                let drawn: AnyElement = if now {
                    Icon::new(icon).size(*size).color(c.text_muted).into_any_element()
                } else {
                    // (The file as it is, its 1.2-unit stroke scaled with the size: what showed before.)
                    svg().path(icon.path()).size(size.pixels()).flex_none().text_color(c.text_muted).into_any_element()
                };
                div().flex().items_center().justify_center().size(px(26.)).child(drawn)
            }))
        }))
    };
    div()
        .flex()
        .flex_col()
        .gap(space::S3)
        .child(div().flex().items_center().gap(space::S6).child(label("")).children(SIZES.iter().map(|(_, name)| div().w(px(200.)).text_role(TextRole::Meta).text_color(c.text_faint).child(*name))))
        .child(line("Before: 1.65 → 1.05 px", false))
        .child(line("Now: 1.4 px at each", true))
}

/// Each size in its frame: the rail's tile (at rest and open), the title bar's button, a toolbar's
/// and a row's.
fn in_place(cx: &App) -> impl IntoElement {
    let c = &cx.theme().colors;
    let frame = |name: &'static str, frame: f32, size: IconSize, icon: IconName, open: bool| {
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(space::S2)
            .w(px(132.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(frame))
                    .rounded(radius::NAV)
                    .border_1()
                    .border_color(if open { gpui::transparent_black() } else { c.border })
                    .when(open, |d| d.bg(c.selected))
                    .child(Icon::new(icon).size(size).color(if open { c.icon_on } else { c.icon })),
            )
            .child(div().text_role(TextRole::Meta).text_color(c.text_faint).child(name))
    };
    div()
        .flex()
        .items_end()
        .gap(space::S4)
        .child(frame("Rail 22 in 36", 36., IconSize::Rail, IconName::Message, false))
        .child(frame("Rail, open", 36., IconSize::Rail, IconName::Bot, true))
        .child(frame("Title bar 18 in 32", 32., IconSize::Large, IconName::ArrowLeft, false))
        .child(frame("Toolbar 16 in 24", 24., IconSize::Medium, IconName::Plus, false))
        .child(frame("Row 14", 20., IconSize::Nav, IconName::Folder, false))
}

fn ramp(cx: &App) -> impl IntoElement {
    let c = &cx.theme().colors;
    let row = |role: TextRole, name: &'static str, color: gpui::Hsla, sample: AnyElement| {
        div()
            .flex()
            .items_baseline()
            .gap(space::S6)
            .child(div().w(px(240.)).flex_none().text_ui(TypeSize::Caption).text_color(c.text_faint).child(name))
            .child(div().text_role(role).text_color(color).child(sample))
    };
    let text = |words: &'static str| div().child(words).into_any_element();
    div()
        .flex()
        .flex_col()
        .gap(space::S4)
        .child(row(TextRole::WindowTitle, "Window title · 14 medium", c.text, text("Release Captain")))
        .child(row(TextRole::WindowContext, "Where it is · 13, faint", c.text_faint, text("Bots")))
        .child(row(TextRole::PaneTitle, "Sidebar heading · 14 medium", c.text, text("Bots")))
        .child(row(TextRole::RowTitle, "Row name · 14 medium / 18", c.text, text("Inbox Helper")))
        .child(row(TextRole::RowDetail, "Under it · 12 / 15, second level", c.text_muted, status_words("1 at work, 1 waiting on you", Some(11..27), cx).into_any_element()))
        .child(row(TextRole::RowLabel, "One-line row · 13", c.text, text("Fix the sign-up form wiping on a taken email")))
        .child(row(TextRole::Meta, "Sections, counts, times · 12, faint", c.text_faint, text("On Other Computers · 12m · 19 open")))
}

/// A sidebar drawn with the ramp and the icons together: two-line rows, a section, one-line rows.
fn in_use(cx: &App) -> impl IntoElement {
    let c = &cx.theme().colors;
    let face = |tint: u32| div().size(px(28.)).flex_none().rounded(radius::PILL).bg(gpui::rgb(tint));
    let bot = |id: &'static str, tint: u32, name: &'static str, words: &'static str, waiting: Option<std::ops::Range<usize>>, selected: bool, badge: usize| {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(space::S2)
            .px(space::S2)
            .py(space::S1_5)
            .rounded(radius::NAV)
            .when(selected, |d| d.bg(c.selected))
            .child(face(tint))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().text_role(TextRole::RowTitle).text_color(c.text).child(name))
                    .child(div().truncate().text_role(TextRole::RowDetail).text_color(c.text_muted).child(status_words(words, waiting, cx))),
            )
            .children((badge > 0).then(|| count_badge(badge, cx)))
    };
    div()
        .flex()
        .items_start()
        .gap(space::S8)
        .child(
            div()
                .flex()
                .flex_col()
                .w(px(300.))
                .flex_none()
                .p(space::S2)
                .rounded(radius::CARD)
                .bg(c.sidebar)
                .gap(space::S0_5)
                .child(div().px(space::S2).h(px(32.)).flex().items_center().text_role(TextRole::PaneTitle).text_color(c.text).child("Bots"))
                .child(bot("g-mira", 0x4F9CF5, "Mira", "1 at work, 1 waiting on you", Some(11..27), false, 1))
                .child(bot("g-inbox", 0xF2735C, "Inbox Helper", "1 at work", None, false, 0))
                .child(bot("g-release", 0x8CCC4F, "Release Captain", "Ships a beta every Friday", None, true, 0))
                .child(bot("g-docs", 0x8A5CF5, "Docs Keeper", "Finished 3h ago", None, false, 0))
                .child(SectionHeader::new("g-groups", "Group Chats"))
                .child(NavRow::new("g-design", "Design review").icon(IconName::Message).trailing(div().text_role(TextRole::Meta).text_color(c.text_faint).child("1d")))
                .child(NavRow::new("g-weekly", "Weekly planning").icon(IconName::Message).selected(true)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .w(px(240.))
                .flex_none()
                .p(space::S2)
                .rounded(radius::CARD)
                .bg(c.sidebar)
                .gap(space::S0_5)
                .child(NavRow::new("g-all", "All Items").icon(IconName::Queue).trailing(div().text_role(TextRole::Meta).text_color(c.text_faint).child("19 open")))
                .child(SectionHeader::new("g-status", "Status"))
                .child(NavRow::new("g-todo", "Todo").icon(IconName::Clock).selected(true))
                .child(NavRow::new("g-done", "Done").icon(IconName::Check))
                .child(NavRow::new("g-files", "Files").icon(IconName::Folder))
                .child(NavRow::new("g-terminal", "Terminal").icon(IconName::Terminal)),
        )
}

fn grid(cx: &App) -> impl IntoElement {
    let c = &cx.theme().colors;
    div().flex().flex_wrap().gap(space::S1).children(IconName::ALL.iter().map(|&icon| {
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(space::S2)
            .w(px(84.))
            .h(px(46.))
            .rounded(radius::NAV)
            .child(Icon::new(icon).size(IconSize::Nav).color(c.text))
            .child(div().text_role(TextRole::Meta).text_color(c.text_faint).whitespace_nowrap().child(icon.name()))
    }))
}
