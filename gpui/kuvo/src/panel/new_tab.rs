//! The New tab page: a search for the folder's files or a page to open, and the tools to open as
//! tabs, each a tile. A tile shows only where its tool works for this conversation.

use gpui::{AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px};
use kuvo_ui::{ActiveTheme, Icon, IconName, IconSize, StyledType, TypeSize, theme::{radius, space}};

use super::{Kind, Place, SidePanel, glass};

/// A tool the page offers.
struct Tile {
    id: &'static str,
    icon: IconName,
    label: &'static str,
    kind: Kind,
}

/// The tools this conversation can open.
fn tiles(panel: &SidePanel, place: Option<&Place>) -> Vec<Tile> {
    let mut tiles = Vec::new();
    if panel.key.is_some() && panel.client.gate().has(kuvo_box::side_chats::SIDE_CHATS) {
        tiles.push(Tile { id: "tile-side-chat", icon: IconName::Message, label: "Side chat", kind: Kind::SideChat(None) });
    }
    // A terminal on the conversation's computer: another computer's over the relay, this Mac's
    // where its box passes terminals to the app.
    if !panel.client.here() || panel.client.gate().has("pty.socket") {
        tiles.push(Tile { id: "tile-terminal", icon: IconName::TerminalWindow, label: "Terminal", kind: Kind::Terminal(0) });
    }
    // That computer's browser, the one its agents use, where its box can show it live (a
    // machine's; a Mac's box doesn't list it until it has a browser of its own, #693).
    if panel.client.advertises(kuvo_box::browser_view::FEATURE) {
        tiles.push(Tile { id: "tile-browser", icon: IconName::Globe, label: "Browser", kind: Kind::Browser(0) });
    }
    if place.is_some() {
        tiles.push(Tile { id: "tile-files", icon: IconName::Folder, label: "Files", kind: Kind::Files });
    }
    tiles
}

pub(super) fn render(panel: &SidePanel, place: Option<&Place>, _window: &mut Window, cx: &mut Context<SidePanel>) -> AnyElement {
    let theme = cx.theme().clone();
    let c = &theme.colors;
    let glass = glass(cx);
    let search = div()
        .id("new-tab-search")
        .debug_selector(|| "new-tab-search".into())
        .w_full()
        .h(px(44.))
        .px(space::S3)
        .rounded(radius::POPOVER)
        .bg(c.card)
        .border_1()
        .border_color(if theme.appearance.is_dark() { c.border_strong } else { c.border })
        .shadow(theme.card_shadow())
        .flex()
        .items_center()
        .gap(space::S2)
        .child(Icon::new(IconName::Search).size(IconSize::Nav).color(c.text_faint))
        .child(div().flex_1().min_w_0().child(panel.search.clone()));

    let tiles = tiles(panel, place);
    // A true grid: two equal columns, the same gap across and down, and a lone tile on the last
    // row keeping its column's width.
    let rows: Vec<AnyElement> = tiles
        .iter()
        .map(|tile| {
            let kind = tile.kind.clone();
            let id = tile.id;
            div()
                .id(SharedString::from(id))
                .debug_selector(move || id.into())
                .min_w_0()
                .h(px(86.))
                .p(space::S3)
                .rounded(radius::POPOVER)
                .bg(glass.tile)
                .border_1()
                .border_color(glass.tile_edge)
                .cursor_pointer()
                .hover(move |s| s.bg(glass.tile_hover))
                .on_click(cx.listener(move |this, _, _, cx| this.replace_active(kind.clone(), cx)))
                .flex()
                .flex_col()
                .justify_between()
                .child(Icon::new(tile.icon).size(IconSize::Medium).color(c.icon_on))
                .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child(tile.label))
                .into_any_element()
        })
        .collect();

    div()
        .id("new-tab")
        .debug_selector(|| "new-tab".into())
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .items_center()
        .pt(px(120.))
        .pb(space::S6)
        .px(space::S5)
        .child(
            div()
                .w_full()
                .max_w(px(436.))
                .flex()
                .flex_col()
                .when(place.is_some(), |d| d.child(search))
                .children(found(panel, cx))
                .when(panel.found().is_none() && !rows.is_empty(), |d| {
                    d.child(div().when(place.is_some(), |d| d.mt(space::S6)).mb(space::S2).text_ui(TypeSize::Caption).medium().text_color(c.text_muted).child("Open"))
                        .child(div().debug_selector(|| "new-tab-tiles".into()).w_full().grid().grid_cols(2).gap(space::S2).children(rows))
                }),
        )
        .into_any_element()
}

/// What the search found: each file with the folder it's in, best first; or that it found none, or
/// couldn't search.
fn found(panel: &SidePanel, cx: &mut Context<SidePanel>) -> Option<AnyElement> {
    let (query, found) = panel.found()?.clone();
    let c = cx.theme().colors.clone();
    let note = |text: String| div().mt(space::S4).px(space::S2).text_ui(TypeSize::Small).text_color(c.text_faint).child(text).into_any_element();
    Some(match found {
        super::Load::Loading => div().mt(space::S4).px(space::S2).child(kuvo_ui::small::LoadingLabel::new("new-tab-finding", "Finding…").compact()).into_any_element(),
        super::Load::Failed(why) => note(format!("Couldn’t search: {why}")),
        super::Load::Ready(found) if found.files.is_empty() => note(format!("No files match “{query}”.")),
        super::Load::Ready(found) => div()
            .debug_selector(|| "new-tab-found".into())
            .mt(space::S3)
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(found.files.iter().take(12).map(|file| {
                let path = file.path.clone();
                let selector = format!("new-tab-found-{}", file.path);
                div()
                    .id(SharedString::from(format!("new-tab-found-{}", file.path)))
                    .debug_selector(move || selector)
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .h(px(32.))
                    .px(space::S2)
                    .rounded(radius::NAV)
                    .cursor_pointer()
                    .hover(move |s| s.bg(c.hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.replace_active(Kind::File(path.clone()), cx)))
                    .child(Icon::new(IconName::File).size(IconSize::Nav).color(c.text_faint))
                    .child(div().flex_none().max_w(px(220.)).truncate().text_ui(TypeSize::Small).child(file.name.clone()))
                    .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Caption).text_color(c.text_faint).child(file.folder.clone()))
            }))
            .into_any_element(),
    })
}
