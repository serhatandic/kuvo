//! The same Work Queue preview for references in descriptions, links, and conversations.
use std::{rc::Rc, time::Duration};

use gpui::{AnyElement, AnyView, App, AppContext as _, Context, ElementId, InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px};
use kuvo_box::{BoxClient, work_queue::{Item, QueueSaid, Whole}};
use kuvo_ui::{ActiveTheme, StyledType, TypeSize, menu::hover_card_frame, theme::{radius, space}};

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

    pub fn chip(&self, id: impl Into<ElementId>, open: impl Fn(&mut Window, &mut App) + 'static, cx: &App) -> AnyElement {
        let c = &cx.theme().colors;
        let number = self.item.number;
        let mark = div().flex_none().child(status_mark(&self.item.status, cx));
        #[cfg(feature = "shots")]
        let mark = mark.on_children_prepainted(move |children, _, cx| {
            if let Some(shot) = cx.try_global::<ShotHoverPoint>()
                && Some(shot.number) == number
                && let Some(bounds) = children.first()
            {
                shot.point.set(Some(bounds.center()));
            }
        });
        div().id(id).debug_selector(move || number.map_or_else(|| "item-chip".into(), |number| format!("item-chip-{number}")))
            .flex().items_center().gap(space::S1_5).h(TypeSize::Body.line_height()).max_w(px(340.)).min_w_0()
            .px(space::S1_5).rounded(radius::NAV).bg(c.sunken).border_1().border_color(c.border)
            .cursor_pointer().hover(|d| d.bg(c.hover))
            .hoverable_tooltip(self.hover()).tooltip_show_delay(DELAY)
            .on_click(move |_, window, cx| open(window, cx))
            .child(mark)
            .children(number.map(|number| div().flex_none().text_ui(TypeSize::Body).text_color(c.text_muted).child(format!("#{number}"))))
            .child(div().min_w_0().truncate().text_ui(TypeSize::Body).text_color(c.text).child(self.item.title.clone()))
            .into_any_element()
    }
}

#[cfg(feature = "shots")]
pub struct ShotHoverPoint {
    pub number: u64,
    pub point: std::cell::Cell<Option<gpui::Point<gpui::Pixels>>>,
}

#[cfg(feature = "shots")]
impl gpui::Global for ShotHoverPoint {}

pub(crate) const DELAY: Duration = Duration::from_millis(300);

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
    let protected = kuvo_ui::markdown::image_ranges(body);
    let mut text = body.to_owned();
    for (range, _) in protected.into_iter().rev() { text.replace_range(range, " "); }
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(320).collect()
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
        hover_card_frame(div().debug_selector(|| "queue-item-hover-card".into()).flex().flex_col().gap(space::S2)
            .child(div().medium().text_ui(TypeSize::Body).line_clamp(2).child(item.title.clone()))
            .child(div().flex().items_center().gap(space::S1_5).text_ui(TypeSize::Caption).text_color(c.text_muted)
                .child(status_mark(&item.status, cx)).child(reference.status_name(&item.status).to_owned()).child("·").child(priority))
            .child(div().text_ui(TypeSize::Caption).truncate().text_color(c.text_muted).child(assignee.to_owned()))
            .when(!item.labels.is_empty(), |d| d.child(div().flex().flex_wrap().gap(space::S2)
                .children(labels.map(|label| div().flex().items_center().gap(space::S1).text_ui(TypeSize::Caption)
                    .text_color(c.text_muted).child(label_dot(&label.color)).child(label.name.clone())))))
            .when_some(item.computer.as_ref(), |d, computer| d.child(div().text_ui(TypeSize::Caption)
                .text_color(c.text_muted).child(crate::bots::computer_name(&computer.name).to_owned())))
            .when_some(self.whole.as_ref().map(|whole| &whole.body).filter(|body| !body.is_empty()), |d, body| d.child(div()
                .text_ui(TypeSize::Small).text_color(c.text_muted).line_clamp(3).child(body.clone()))), cx)
    }
}
