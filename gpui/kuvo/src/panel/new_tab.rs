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
fn tiles(place: Option<&Place>) -> Vec<Tile> {
    let mut tiles = Vec::new();
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

    let tiles = tiles(place);
    let rows: Vec<AnyElement> = tiles
        .chunks(2)
        .map(|pair| {
            div()
                .flex()
                .gap(space::S2)
                .children(pair.iter().map(|tile| {
                    let kind = tile.kind.clone();
                    let id = tile.id;
                    div()
                        .id(SharedString::from(id))
                        .debug_selector(move || id.into())
                        .flex_1()
                        .min_w_0()
                        .h(px(86.))
                        .p(space::S3)
                        .rounded(radius::POPOVER)
                        .bg(glass.tile)
                        .border_1()
                        .border_color(glass.tile_edge)
                        .cursor_pointer()
                        .hover(move |s| s.bg(glass.tile_hover))
                        .on_click(cx.listener(move |this, _, window, cx| this.replace_active(kind.clone(), window, cx)))
                        .flex()
                        .flex_col()
                        .justify_between()
                        .child(Icon::new(tile.icon).size(IconSize::Medium).color(c.icon_on))
                        .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child(tile.label))
                }))
                // An odd tile keeps its half.
                .when(pair.len() == 1, |d| d.child(div().flex_1()))
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
                .child(search)
                .when(!rows.is_empty(), |d| {
                    d.child(div().mt(space::S6).mb(space::S2).text_ui(TypeSize::Caption).medium().text_color(c.text_muted).child("Open"))
                        .child(div().flex().flex_col().gap(space::S2).children(rows))
                }),
        )
        .into_any_element()
}
