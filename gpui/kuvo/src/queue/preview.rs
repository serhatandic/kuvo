//! The same Work Queue preview for references in descriptions, links, and conversations.
use std::{rc::Rc, time::Duration};

use gpui::{AnyElement, AnyView, App, AppContext as _, Context, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels, Render, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px};
use kuvo_box::{BoxClient, work_queue::{Item, QueueSaid, Whole}};
use kuvo_ui::{
    ActiveTheme, StyledType, TextRow, TypeSize,
    menu::{HoverPopover, card_frame},
    theme::{MEDIUM, radius, space},
};

use super::marks::{label_dot, status_mark};

#[derive(Clone)]
pub(crate) struct Reference {
    pub item: Item,
    queue: Rc<QueueSaid>,
    client: BoxClient,
    live: bool,
}

impl Reference {
    pub fn new(item: Item, queue: Rc<QueueSaid>, client: BoxClient, live: bool) -> Self {
        Self { item, queue, client, live }
    }

    pub fn status(&self) -> &str {
        self.status_name(&self.item.status)
    }

    fn status_name<'a>(&'a self, id: &'a str) -> &'a str {
        self.queue.statuses.iter().find(|status| status.id == id)
            .map_or(id, |status| status.name.as_str())
    }

    pub fn hover(&self) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let reference = self.clone();
        move |_, cx| cx.new(|cx| Preview::new(reference.clone(), cx)).into()
    }

    pub fn hover_popover(&self, id: impl Into<ElementId>, trigger: impl IntoElement, fill: bool) -> AnyElement {
        let mut popover = HoverPopover::new(id, trigger, self.hover())
            .delay(DELAY)
            .expected_height(HOVER_CARD_EXPECTED_HEIGHT);
        if fill {
            popover = popover.fill();
        }
        popover.into_any_element()
    }

    pub fn chip(&self, id: impl Into<ElementId>, open: impl Fn(&mut Window, &mut App) + 'static, cx: &App) -> AnyElement {
        let c = &cx.theme().colors;
        let id = id.into();
        let number = self.item.number;
        let mark = div().flex_none().child(status_mark(&self.item.status, cx));
        #[cfg(feature = "shots")]
        let mark = match number {
            Some(number) => ShotHoverPoint::capture(ShotHoverTarget::Reference(number), mark),
            None => mark,
        };
        let chip = div()
            .id(id.clone())
            .debug_selector(move || number.map_or_else(|| "item-chip".into(), |number| format!("item-chip-{number}")))
            .flex()
            .items_center()
            .gap(space::S1_5)
            .h(TypeSize::Body.line_height())
            .max_w(px(340.))
            .min_w_0()
            .px(space::S1_5)
            .rounded(radius::NAV)
            .bg(c.sunken)
            .border_1()
            .border_color(c.border)
            .cursor_pointer()
            .hover(|d| d.bg(c.hover))
            .on_click(move |_, window, cx| open(window, cx))
            .child(mark)
            .children(number.map(|number| div().flex_none().text_ui(TypeSize::Body).text_color(c.text_muted).child(format!("#{number}"))))
            .child(div().min_w_0().truncate().text_ui(TypeSize::Body).text_color(c.text).child(self.item.title.clone()));
        self.hover_popover((id, "hover"), chip, false)
    }

    /// Its chip where it's named in a message's text, `size` as its label was laid out there: the
    /// label's glyphs are the text's (its number, its title), drawn over this; this is the chip's
    /// face (its fill, edge and status mark), its hover card and its click.
    pub fn inline_chip(&self, size: gpui::Size<Pixels>, open: Rc<dyn Fn(&mut Window, &mut App)>, cx: &App) -> AnyElement {
        let c = &cx.theme().colors;
        let number = self.item.number;
        let id = ElementId::from(("inline-item", number.unwrap_or_default() as usize));
        let mark = div().flex_none().child(status_mark(&self.item.status, cx));
        #[cfg(feature = "shots")]
        let mark = match number {
            Some(number) => ShotHoverPoint::capture(ShotHoverTarget::Reference(number), mark),
            None => mark,
        };
        let face = div()
            .id(id.clone())
            .debug_selector(move || number.map_or_else(|| "item-chip".into(), |number| format!("item-chip-{number}")))
            .w(size.width)
            .h(size.height)
            .flex()
            .items_center()
            .pl(px(2.))
            .rounded(radius::SMALL)
            .bg(c.sunken)
            .border_1()
            .border_color(c.border)
            .cursor_pointer()
            .hover(|d| d.bg(c.hover))
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                open(window, cx);
            })
            .child(mark);
        self.hover_popover((id, "hover"), face, false)
    }

    /// Its chip in a Work Queue step's row ("Updated [○ #954 Remove slash commands]"): the row's
    /// size, its title truncated.
    pub fn row_chip(&self, id: impl Into<ElementId>, open: impl Fn(&mut Window, &mut App) + 'static, cx: &App) -> AnyElement {
        let c = &cx.theme().colors;
        let id = id.into();
        let number = self.item.number;
        let chip = div()
            .id(id.clone())
            .debug_selector(move || number.map_or_else(|| "row-item-chip".into(), |number| format!("row-item-chip-{number}")))
            .flex()
            .flex_none()
            .items_center()
            .gap(space::S1)
            .h(TypeSize::Small.line_height())
            .max_w(px(320.))
            .min_w_0()
            .px(space::S1)
            .rounded(radius::SMALL)
            .bg(c.sunken)
            .border_1()
            .border_color(c.border)
            .cursor_pointer()
            .hover(|d| d.bg(c.hover))
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                open(window, cx);
            })
            .child(div().flex_none().child(status_mark(&self.item.status, cx)))
            .children(number.map(|number| div().flex_none().text_ui(TypeSize::Small).text_color(c.text_muted).child(format!("#{number}"))))
            .child(div().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text).child(self.item.title.clone()));
        self.hover_popover((id, "hover"), chip, false)
    }
}

#[cfg(feature = "shots")]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ShotHoverTarget {
    Reference(u64),
    ItemStatus,
}

#[cfg(feature = "shots")]
pub struct ShotHoverPoint {
    pub target: ShotHoverTarget,
    pub point: std::cell::Cell<Option<gpui::Point<gpui::Pixels>>>,
}

#[cfg(feature = "shots")]
impl ShotHoverPoint {
    pub(super) fn capture(target: ShotHoverTarget, element: gpui::Div) -> gpui::Div {
        element.on_children_prepainted(move |children, _, cx| {
            if let Some(shot) = cx.try_global::<Self>()
                && shot.target == target
                && let Some(bounds) = children.first()
            {
                shot.point.set(Some(bounds.center()));
            }
        })
    }
}

#[cfg(feature = "shots")]
impl gpui::Global for ShotHoverPoint {}

pub(crate) const DELAY: Duration = Duration::from_millis(300);

/// Conservative full height (title + metadata + labels + 3-line excerpt) used before the first
/// frame so the card picks above or below up front and never flips once the body loads.
pub(crate) const HOVER_CARD_EXPECTED_HEIGHT: Pixels = px(260.);

struct Preview {
    reference: Reference,
    whole: Option<Whole>,
}

impl Preview {
    fn new(reference: Reference, cx: &mut Context<Self>) -> Self {
        let client = reference.client.clone();
        let id = reference.item.id.clone();
        let read = move || client.work_item(&id).ok().map(|mut whole| {
            whole.body = if whole.told { String::new() } else { excerpt(&whole.body) };
            whole
        });
        let whole = if reference.live {
            cx.spawn(async move |this, cx| {
                let whole = cx.background_executor().spawn(async move { read() }).await;
                this.update(cx, |this, cx| {
                    this.whole = whole;
                    cx.notify();
                }).ok();
            }).detach();
            None
        } else {
            read()
        };
        Self { reference, whole }
    }
}

fn excerpt(body: &str) -> String {
    kuvo_ui::markdown::plain_text(body).chars().take(320).collect()
}

impl Render for Preview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = &cx.theme().colors;
        let reference = &self.reference;
        let item = self.whole.as_ref().map_or(&reference.item, |whole| &whole.item);
        let priority = reference.queue.priorities.iter().find(|priority| priority.id == item.priority)
            .map_or("Normal priority".to_owned(), |priority| format!("{} priority", priority.name));
        let labels = item.labels.iter().filter_map(|id| reference.queue.labels.iter().find(|label| &label.id == id));
        let assignee = item.assignee.as_ref().map(|assignee| if assignee.name.is_empty() { "Assigned" } else { &assignee.name }).unwrap_or("Unassigned");
        let content = div()
            .debug_selector(|| "queue-item-hover-card".into())
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(space::S2)
            .child(
                TextRow::wrapping("queue-hover-title", item.title.clone())
                    .debug_selector("queue-hover-title")
                    .max_lines(2)
                    .text_size(TypeSize::Body)
                    .font_weight(MEDIUM)
                    .text_color(c.text),
            )
            .child(
                TextRow::truncated("queue-hover-status", format!("{} · {priority}", reference.status_name(&item.status)))
                    .hug()
                    .gap(space::S1_5)
                    .text_size(TypeSize::Caption)
                    .text_color(c.text_muted)
                    .leading(status_mark(&item.status, cx)),
            )
            .child(
                TextRow::truncated("queue-hover-assignee", assignee.to_owned())
                    .text_size(TypeSize::Caption)
                    .text_color(c.text_muted),
            )
            .when(!item.labels.is_empty(), |d| {
                d.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .flex_wrap()
                        .gap(space::S2)
                        .children(labels.map(|label| {
                            div()
                                .max_w_full()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap(space::S1)
                                .text_ui(TypeSize::Caption)
                                .text_color(c.text_muted)
                                .child(div().flex_none().child(label_dot(&label.color)))
                                .child(div().min_w_0().truncate().child(label.name.clone()))
                        })),
                )
            })
            .when_some(item.computer.as_ref(), |d, computer| {
                d.child(
                    TextRow::truncated("queue-hover-computer", crate::bots::computer_name(&computer.name).to_owned())
                        .text_size(TypeSize::Caption)
                        .text_color(c.text_muted),
                )
            })
            .when_some(self.whole.as_ref().map(|whole| &whole.body).filter(|body| !body.is_empty()), |d, body| {
                d.child(
                    TextRow::wrapping("queue-hover-body", body.clone())
                        .debug_selector("queue-hover-body")
                        .max_lines(3)
                        .text_size(TypeSize::Small)
                        .text_color(c.text_muted),
                )
            });
        card_frame(content, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::excerpt;

    #[test]
    fn excerpt_strips_markdown_and_images_to_plain_text() {
        let raw = "The user, 11-18: \"file this\", ![shot](kuvo-attachment:file/abc). **What's wrong:** in `AskCard`, [links](https://example.com) and *italics* overflow.";
        assert_eq!(
            excerpt(raw),
            "The user, 11-18: \"file this\", . What's wrong: in AskCard, links and italics overflow."
        );
    }
}
