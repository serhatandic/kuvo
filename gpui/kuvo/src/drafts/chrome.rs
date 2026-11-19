//! A quiet stand-in Kuvo window shell (title bar, rail, Sessions sidebar, centered 720 px
//! conversation column, and composer) so design drafts sit inside the app's actual visual language
//! without touching real views.

use gpui::{
    AnyElement, App, IntoElement, ParentElement, SharedString, Styled, div,
    prelude::FluentBuilder as _, px, relative,
};
use kuvo_ui::{
    ActiveTheme, Icon, IconName, IconSize, StyledType, TextRole, TypeSize,
    theme::{radius, space},
};

/// Wraps a conversation's rows inside a faithful Kuvo window frame.
pub fn window_shell(draft_label: &str, turns: Vec<AnyElement>, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(c.background)
        .text_ui(TypeSize::Body)
        .text_color(c.text)
        .child(title_bar(draft_label, cx))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(rail(cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .rounded_tl(radius::POPOVER)
                        .border_t_1()
                        .border_l_1()
                        .border_color(c.border)
                        .overflow_hidden()
                        .child(sidebar(cx))
                        .child(conversation_pane(turns, cx)),
                ),
        )
        .into_any_element()
}

fn title_bar(draft_label: &str, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let draft_label = SharedString::from(draft_label.to_owned());
    div()
        .h(px(44.))
        .flex_none()
        .flex()
        .items_center()
        .px(space::S3)
        .gap(space::S2)
        .child(
            div()
                .w(px(64.))
                .flex()
                .items_center()
                .gap(px(8.))
                .pl(space::S1)
                .child(div().size(px(12.)).rounded(radius::PILL).bg(c.border_strong))
                .child(div().size(px(12.)).rounded(radius::PILL).bg(c.border_strong))
                .child(div().size(px(12.)).rounded(radius::PILL).bg(c.border_strong)),
        )
        .child(
            div()
                .w(px(218.))
                .flex()
                .items_center()
                .gap(space::S2)
                .child(Icon::new(IconName::SidebarLeft).size(IconSize::Medium).color(c.icon))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S1)
                        .px(space::S2)
                        .py(space::S1)
                        .rounded(radius::NAV)
                        .text_ui(TypeSize::Small)
                        .text_color(c.text_muted)
                        .child("This Mac")
                        .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(c.text_faint)),
                )
                .child(div().flex_1())
                .child(Icon::new(IconName::ArrowLeft).size(IconSize::Medium).color(c.text_faint))
                .child(Icon::new(IconName::ArrowRight).size(IconSize::Medium).color(c.text_faint)),
        )
        .child(div().w(px(1.)).h(px(16.)).bg(c.border))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .pl(space::S2)
                .flex()
                .items_center()
                .gap(space::S2)
                .child(div().text_role(TextRole::WindowTitle).text_color(c.text).child("Wire shown-file Flow view"))
                .child(div().text_role(TextRole::WindowContext).text_color(c.text_faint).child("kuvo-code")),
        )
        .child(
            div()
                .px(space::S2)
                .py(px(3.))
                .rounded(radius::PILL)
                .bg(c.sunken)
                .border_1()
                .border_color(c.border)
                .text_ui(TypeSize::Caption)
                .medium()
                .text_color(c.text_muted)
                .child(draft_label),
        )
        .child(
            div()
                .pl(space::S1)
                .child(Icon::new(IconName::PanelRight).size(IconSize::Large).color(c.icon)),
        )
        .into_any_element()
}

fn rail(cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let rail_btn = |icon: IconName, selected: bool| {
        div()
            .size(px(36.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(radius::NAV)
            .when(selected, |d| d.bg(c.selected_card).shadow(cx.theme().selected_shadow()))
            .child(Icon::new(icon).size(IconSize::Rail).color(if selected { c.icon_on } else { c.icon }))
    };
    div()
        .w(px(46.))
        .h_full()
        .flex_none()
        .flex()
        .flex_col()
        .items_center()
        .pt(space::S2)
        .pb(space::S3)
        .gap(space::S1_5)
        .child(rail_btn(IconName::Bot, false))
        .child(rail_btn(IconName::Message, true))
        .child(rail_btn(IconName::Queue, false))
        .child(rail_btn(IconName::Computer, false))
        .child(rail_btn(IconName::Activity, false))
        .child(div().flex_1())
        .child(rail_btn(IconName::Question, false))
        .child(rail_btn(IconName::Settings, false))
        .into_any_element()
}

fn sidebar(cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let session_row = |title: &'static str, when: &'static str, selected: bool| {
        div()
            .h(px(30.))
            .px(space::S2)
            .flex()
            .items_center()
            .gap(space::S2)
            .rounded(radius::NAV)
            .when(selected, |d| d.bg(c.selected_card).shadow(cx.theme().selected_shadow()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_role(TextRole::RowLabel)
                    .when(selected, |d| d.medium())
                    .text_color(if selected { c.text } else { c.text_muted })
                    .child(title),
            )
            .child(div().flex_none().text_role(TextRole::Meta).text_color(c.text_faint).child(when))
    };

    div()
        .w(px(252.))
        .h_full()
        .flex_none()
        .flex()
        .flex_col()
        .bg(c.sidebar)
        .border_r_1()
        .border_color(c.border)
        .p(space::S3)
        .gap(space::S3)
        .child(
            div()
                .h(px(30.))
                .px(space::S2)
                .rounded(radius::NAV)
                .bg(c.surface)
                .border_1()
                .border_color(c.border)
                .flex()
                .items_center()
                .gap(space::S1_5)
                .child(Icon::new(IconName::Search).size(IconSize::Nav).color(c.text_faint))
                .child(div().text_ui(TypeSize::Small).text_color(c.text_faint).child("Search sessions")),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::S0_5)
                .child(
                    div()
                        .px(space::S2)
                        .py(space::S1)
                        .flex()
                        .items_center()
                        .gap(space::S1_5)
                        .child(Icon::new(IconName::FolderOpen).size(IconSize::Nav).color(c.text_faint))
                        .child(div().flex_1().text_role(TextRole::Meta).medium().text_color(c.text_muted).child("kuvo-code"))
                        .child(Icon::new(IconName::Plus).size(IconSize::Nav).color(c.text_faint)),
                )
                .child(session_row("Wire shown-file Flow view", "now", true))
                .child(session_row("Tool call details drafts", "18m", false))
                .child(session_row("Work Queue inline notes", "2h", false))
                .child(session_row("Wake-up schedule popover", "1d", false)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::S0_5)
                .child(
                    div()
                        .px(space::S2)
                        .py(space::S1)
                        .text_role(TextRole::Meta)
                        .text_color(c.text_faint)
                        .child("Chats"),
                )
                .child(session_row("Release checklist", "3d", false))
                .child(session_row("Friday sync notes", "5d", false)),
        )
        .into_any_element()
}

fn conversation_pane(turns: Vec<AnyElement>, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .flex_col()
        .bg(c.surface)
        .child(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .px(space::S6)
                .pt(space::S5)
                .pb(space::S4)
                .child(
                    div()
                        .w_full()
                        .max_w(px(720.))
                        .flex()
                        .flex_col()
                        .gap(space::S4)
                        .children(turns),
                ),
        )
        .child(composer_bar(cx))
        .into_any_element()
}

fn composer_bar(cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .flex_none()
        .w_full()
        .flex()
        .justify_center()
        .px(space::S6)
        .pb(space::S4)
        .pt(space::S1)
        .child(
            div()
                .w_full()
                .max_w(px(720.))
                .rounded(radius::CARD)
                .bg(c.surface)
                .border_1()
                .border_color(c.border_strong)
                .shadow(cx.theme().card_shadow())
                .px(space::S4)
                .py(space::S3)
                .flex()
                .flex_col()
                .gap(space::S2)
                .child(div().text_ui(TypeSize::Body).text_color(c.text_faint).child("Reply…"))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .child(Icon::new(IconName::Paperclip).size(IconSize::Nav).color(c.icon))
                        .child(
                            div()
                                .px(space::S2)
                                .py(px(3.))
                                .rounded(radius::NAV)
                                .bg(c.sunken)
                                .text_ui(TypeSize::Caption)
                                .text_color(c.text_muted)
                                .child("Claude Sonnet 4.6"),
                        )
                        .child(
                            div()
                                .px(space::S2)
                                .py(px(3.))
                                .rounded(radius::NAV)
                                .text_ui(TypeSize::Caption)
                                .text_color(c.text_faint)
                                .child("Auto"),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .size(px(28.))
                                .rounded(radius::PILL)
                                .bg(c.sunken)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(Icon::new(IconName::ArrowUp).size(IconSize::Nav).color(c.text_faint)),
                        ),
                ),
        )
        .into_any_element()
}

/// Right-aligned user message bubble matching `entries.rs::user_message`.
pub fn user_bubble(text: &str, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .w_full()
        .flex()
        .justify_end()
        .child(
            div()
                .max_w(relative(0.80))
                .px(space::S4)
                .py(px(10.))
                .rounded(radius::CARD)
                .bg(c.sunken)
                .text_ui(TypeSize::Body)
                .text_color(c.text)
                .child(SharedString::from(text.to_owned())),
        )
        .into_any_element()
}

/// Left-aligned plain assistant prose matching `entries.rs`.
pub fn agent_text(text: &str, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .text_ui(TypeSize::Body)
        .text_color(c.text)
        .child(SharedString::from(text.to_owned()))
        .into_any_element()
}
