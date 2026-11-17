use gpui::{AnyElement, AppContext as _, Context, Entity, Focusable as _, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px};
use kuvo_box::work_queue::ItemRelation;
use kuvo_ui::{ActiveTheme, Button, ButtonSize, IconName, StyledType, TextField, TypeSize, theme::{radius, space}};

use super::QueueView;

const KINDS: [(&str, &str); 4] = [("related", "Related"), ("blocks", "Blocks"), ("blocked-by", "Blocked by"), ("duplicate-of", "Duplicate of")];

fn kind_name(kind: &str) -> &str {
    KINDS.iter().find_map(|(key, name)| (*key == kind).then_some(*name)).unwrap_or(match kind {
        "mentions" => "Mentions",
        "mentioned-in" => "Mentioned in",
        "duplicated-by" => "Duplicated by",
        _ => "Linked to",
    })
}

pub(super) struct Picker {
    field: Entity<TextField>,
    kind: &'static str,
    _observe: gpui::Subscription,
}

fn name(number: Option<u64>, title: &str) -> String {
    number.map_or_else(|| title.to_owned(), |number| format!("#{number} {title}"))
}

impl QueueView {
    fn show_relation_picker(&mut self, cx: &mut Context<Self>) {
        let field = cx.new(|cx| TextField::new(cx).placeholder("Find an item"));
        self.wants_focus = Some(field.focus_handle(cx));
        self.relation_picker = Some(Picker { _observe: cx.observe(&field, |_, _, cx| cx.notify()), field, kind: "related" });
        cx.notify();
    }

    fn close_relation_picker(&mut self, cx: &mut Context<Self>) {
        self.relation_picker = None;
        self.wants_focus = Some(self.focus.clone());
        cx.notify();
    }

    fn set_relation(&mut self, other: String, kind: Option<&'static str>, cx: &mut Context<Self>) {
        let Some(item) = self.whole().map(|whole| whole.item.id.clone()) else { return };
        self.close_relation_picker(cx);
        let id = item.clone();
        self.change(&item, move |client| client.set_work_item_relation(&id, &other, kind), cx);
        self.rebuild(cx);
    }

    pub(super) fn relation_section(&self, relations: &[ItemRelation], can_link: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let header = div().flex().items_center().justify_between()
            .child(div().text_ui(TypeSize::Body).text_color(c.text_muted).child("Links"))
            .when(can_link, |header| header.child(Button::new("link-item", "Link item…").icon(IconName::Link)
                .size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.show_relation_picker(cx)))));
        let rows = relations.iter().enumerate().map(|(at, relation)| {
            let open = relation.id.clone();
            let remove = relation.id.clone();
            let manual = !matches!(relation.kind.as_str(), "mentions" | "mentioned-in");
            div().flex().items_center().gap(space::S2).min_w_0()
                .child(div().id(("item-relation", at)).debug_selector(move || format!("item-relation-{at}"))
                    .px(space::S2).py(space::S1).rounded(radius::NAV).hover(|d| d.bg(c.hover))
                    .flex().items_center().gap(space::S2).min_w_0().flex_1().cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.open_reference(open.clone(), cx)))
                    .child(div().flex_none().text_ui(TypeSize::Small).text_color(c.text_muted).child(kind_name(&relation.kind).to_owned()))
                    .child(div().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.accent).child(name(relation.number, &relation.title))))
                .when(manual && can_link, |row| row.child(Button::icon_only(("unlink-item", at), IconName::Close, "Remove link")
                    .ghost().size(ButtonSize::Small).on_click(cx.listener(move |this, _, _, cx| this.set_relation(remove.clone(), None, cx)))))
        });
        div().pt(space::S6).pb(space::S2).flex().flex_col().gap(space::S2).child(header)
            .when(!relations.is_empty(), |d| d.child(div().p(space::S2).rounded(radius::CARD).bg(c.card).children(rows)))
            .into_any_element()
    }

    pub(super) fn relation_sheet(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let picker = self.relation_picker.as_ref()?;
        let whole = self.whole()?;
        let c = cx.theme().colors.clone();
        let query = picker.field.read(cx).text().trim().to_lowercase();
        let computer = whole.item.computer.as_ref().map(|computer| computer.key.as_str());
        let choices: Vec<_> = self.queue().into_iter().flat_map(|queue| &queue.items).filter(|item| {
            item.id != whole.item.id && item.handing.is_none()
                && item.computer.as_ref().map(|computer| computer.key.as_str()) == computer
                && name(item.number, &item.title).to_lowercase().contains(&query)
        }).take(50).map(|item| (item.id.clone(), name(item.number, &item.title))).collect();
        let empty = choices.is_empty();
        let choices = choices.into_iter().enumerate().map(|(at, (id, name))| {
            let selector = format!("link-choice-{id}");
            div().id(("link-choice", at)).debug_selector(move || selector).px(space::S3).py(space::S2).min_w_0().truncate().rounded(radius::NAV)
                .text_ui(TypeSize::Small).cursor_pointer().hover(|d| d.bg(c.hover)).child(SharedString::from(name))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(kind) = this.relation_picker.as_ref().map(|picker| picker.kind) {
                        this.set_relation(id.clone(), Some(kind), cx);
                    }
                }))
        });
        let kinds = KINDS.iter().map(|&(kind, name)| {
            let button = Button::new(kind, name).size(ButtonSize::Small);
            let button = if kind == picker.kind { button.primary() } else { button.ghost() };
            button.on_click(cx.listener(move |this, _, _, cx| {
                if let Some(picker) = &mut this.relation_picker { picker.kind = kind; }
                cx.notify();
            }))
        });
        Some(div().id("link-item-sheet").absolute().inset_0().occlude().bg(crate::bots::dim(cx))
            .flex().justify_center().items_start().pt(px(70.)).px(space::S5)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" { this.close_relation_picker(cx); }
            }))
            .child(div().w(px(500.)).max_w_full().rounded(radius::CARD).border_1().border_color(c.border)
                .bg(c.elevated).shadow(cx.theme().elevated_shadow()).p(space::S4).flex().flex_col().gap(space::S2)
                .child(div().flex().justify_between().items_center().child("Link item")
                    .child(Button::icon_only("close-link-item", IconName::Close, "Close").ghost().size(ButtonSize::Small)
                        .on_click(cx.listener(|this, _, _, cx| this.close_relation_picker(cx)))))
                .child(div().flex().flex_wrap().gap(space::S1).children(kinds))
                .child(picker.field.clone())
                .child(div().id("link-item-results").max_h(px(260.)).overflow_y_scroll()
                    .when(empty, |d| d.p(space::S3).text_ui(TypeSize::Small).text_color(c.text_muted).child("No matching items on this computer."))
                    .children(choices)))
            .into_any_element())
    }
}
