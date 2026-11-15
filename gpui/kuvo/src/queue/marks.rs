//! The Work Queue's small marks, the same wherever one shows (a row, a chip, a menu, the sidebar):
//! a status, a priority, a label's dot and chip, who an item is for. Each is drawn from what the
//! box names (a status's or a priority's id, a label's colour, a bot's look); one this doesn't know
//! gets a plain mark, never none.

use gpui::{AnyElement, App, InteractiveElement, IntoElement, ParentElement, Pixels, Styled, div, px, svg};
use kuvo_box::areas::Look;
use kuvo_ui::{ActiveTheme, Icon, IconName, StyledType, TypeSize, theme::radius, theme::space};

use crate::areas::{bot_face, look_tint};

/// The box a mark sits in, so marks of every kind line up.
const BOX: f32 = 16.;

fn boxed(mark: impl IntoElement) -> AnyElement {
    div().flex().flex_none().items_center().justify_center().size(px(BOX)).child(mark).into_any_element()
}

/// A status's mark, by its id: a ring that fills as work moves on, a check done, a cross canceled.
pub fn status_mark(id: &str, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let ring = |color: gpui::Hsla| div().flex().items_center().justify_center().size(px(13.)).rounded(radius::PILL).border(px(1.5)).border_color(color);
    match id {
        // Only you can move it on: a ring with a bar and a dot.
        "needs-you" => boxed(ring(c.danger).flex_col().gap(px(1.)).child(div().w(px(1.5)).h(px(4.)).rounded(radius::PILL).bg(c.danger)).child(div().size(px(1.5)).rounded(radius::PILL).bg(c.danger))),
        "in-review" => boxed(ring(c.success).child(div().size(px(7.)).rounded(radius::PILL).bg(c.success))),
        "in-progress" => boxed(ring(c.warning).child(div().size(px(4.)).rounded(radius::PILL).bg(c.warning))),
        "waiting" => boxed(Icon::new(IconName::Clock).color(look_tint("sky"))),
        "todo" => boxed(ring(c.text_muted)),
        "backlog" => boxed(ring(c.text_faint).border_dashed()),
        "done" => boxed(Icon::new(IconName::Check).color(c.success)),
        "canceled" => boxed(Icon::new(IconName::Close).color(c.text_faint)),
        _ => boxed(ring(c.text_faint)),
    }
}

/// A priority's mark, by its id: three bars, as many filled as it's pressing (all of them, red,
/// for Urgent).
pub fn priority_mark(id: &str, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let (filled, color) = match id {
        "urgent" => (3, c.danger),
        "high" => (3, c.text_muted),
        "normal" => (2, c.text_muted),
        "low" => (1, c.text_muted),
        _ => (0, c.text_muted),
    };
    boxed(div().flex().items_end().gap(px(1.5)).h(px(11.)).children((0..3).map(|bar| div().w(px(3.)).h(px(5. + 3. * bar as f32)).rounded(px(1.)).bg(if bar < filled { color } else { c.border_strong }))))
}

/// A label's dot, in its colour.
pub fn label_dot(color: &str) -> AnyElement {
    div().size(px(8.)).flex_none().rounded(radius::PILL).bg(look_tint(color)).into_any_element()
}

/// A label as an item wears it: its dot and its name.
pub fn label_chip(name: &str, color: &str, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S1_5)
        .h(px(22.))
        .max_w(px(140.))
        .px(space::S2)
        .rounded(radius::PILL)
        .border_1()
        .border_color(c.border)
        .text_ui(TypeSize::Caption)
        .text_color(c.text_muted)
        .child(label_dot(color))
        .child(div().truncate().child(name.to_owned()))
        .into_any_element()
}

/// The mark Kuvo has for a coding agent, by the id the box names it by (`claude-code`, `codex-cli`,
/// `cursor`): the one the rail draws for its account.
fn agent_mark(id: &str) -> Option<(&'static str, String)> {
    let service = id.split('-').next().unwrap_or(id);
    [id, service].into_iter().find_map(|service| kuvo_ui::icon::mark_path(service).map(|path| (path, service.to_owned())))
}

/// Who an item is for: a bot's still face where the box gave its look; a coding agent's own mark;
/// else an initial.
pub fn who_mark(look: Option<&Look>, id: &str, name: &str, size: Pixels, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    match (look, agent_mark(id)) {
        (Some(look), _) => bot_face(look, size),
        (None, Some((path, service))) => div()
            .debug_selector({
                let selector = format!("agent-mark-{service}");
                move || selector
            })
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(size)
            .child(svg().path(path).size(size * 0.86).flex_none().text_color(kuvo_ui::icon::mark_color(&service).unwrap_or(c.text)))
            .into_any_element(),
        (None, None) => div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(size)
            .rounded(radius::PILL)
            .bg(c.sunken)
            .border_1()
            .border_color(c.border)
            .text_size(size * 0.56)
            .line_height(size)
            .medium()
            .text_color(c.text_muted)
            .child(name.chars().next().map(|first| first.to_uppercase().to_string()).unwrap_or_default())
            .into_any_element(),
    }
}
