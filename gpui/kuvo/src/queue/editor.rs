use std::{collections::HashSet, rc::Rc};

use gpui::{AnyElement, App, Context, Focusable as _, InteractiveElement, IntoElement, KeyBinding, ParentElement, StatefulInteractiveElement, Styled, Window, actions, div, prelude::FluentBuilder as _, px};
use kuvo_ui::{ActiveTheme, StyledType, TypeSize, text_field::InlineBlock, theme::{radius, space}};

use super::{QueueView, attachments, references};

actions!(queue_editor, [NextField, PreviousField, SelectReference, NextReference, PreviousReference, HideReferences]);

pub(super) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", NextField, Some("QueueEditor")),
        KeyBinding::new("shift-tab", PreviousField, Some("QueueEditor")),
        KeyBinding::new("enter", SelectReference, Some("QueueReferences > KuvoTextField")),
        KeyBinding::new("down", NextReference, Some("QueueReferences > KuvoTextField")),
        KeyBinding::new("up", PreviousReference, Some("QueueReferences > KuvoTextField")),
        KeyBinding::new("escape", HideReferences, Some("QueueReferences > KuvoTextField")),
    ]);
}

impl QueueView {
    pub(super) fn focus_field(&mut self, description: bool, window: &mut Window, cx: &mut Context<Self>) {
        let field = if description { &self.body } else { &self.title };
        window.focus(&field.focus_handle(cx), cx);
        cx.notify();
    }

    pub(super) fn suggestions(&self, window: &Window, cx: &App) -> Vec<(u64, String)> {
        if self.saving || self.suggestions_dismissed || self.picker.is_some() || !self.body.focus_handle(cx).is_focused(window) {
            return Vec::new();
        }
        let field = self.body.read(cx);
        references::completion(field.text(), field.selection()).map_or_else(Vec::new, |(_, query)| self.suggest_items(query))
    }

    pub(super) fn move_suggestion(&mut self, step: isize, window: &Window, cx: &mut Context<Self>) -> bool {
        let count = self.suggestions(window, cx).len();
        if count == 0 { return false; }
        self.suggestion_at = (self.suggestion_at as isize + step).rem_euclid(count as isize) as usize;
        cx.notify();
        true
    }

    pub(super) fn suggestions_menu(&self, suggestions: Vec<(u64, String)>, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let selected = self.suggestion_at.min(suggestions.len().saturating_sub(1));
        div().mx(space::S5).mb(space::S2).p(space::S1).rounded(radius::NAV).bg(colors.elevated)
            .border_1().border_color(colors.border).shadow(cx.theme().card_shadow())
            .children(suggestions.into_iter().enumerate().map(|(at, (number, title))| {
                div().id(("reference-choice", at)).debug_selector(move || format!("reference-choice-{number}"))
                    .min_w_0().px(space::S2).py(space::S0_5).flex().items_center().gap(space::S2).rounded(radius::NAV)
                    .cursor_pointer().hover(|d| d.bg(colors.hover)).when(at == selected, |d| d.bg(colors.hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.take_suggestion(number, cx)))
                    .child(div().flex_none().text_ui(TypeSize::Caption).text_color(colors.text_muted).child(format!("#{number}")))
                    .child(div().min_w_0().truncate().text_ui(TypeSize::Small).text_color(colors.text).child(title))
            })).into_any_element()
    }

    pub(super) fn decorate_editor(&mut self, new: bool, suggestions: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let text = self.body.read(cx).text().to_owned();
        let mut files = if new { Vec::new() } else { self.whole().map(attachments::kept).unwrap_or_default() };
        files.extend(attachments::drafts(self.body_composer.read(cx)));
        let anchors = attachments::anchors(&text, &files);
        let mut referenced = HashSet::new();
        for file in &files { self.load_attachment(file, false, cx); }
        let renderer = self.attachment_cards(cx);
        let cap = if self.enlarged { 520. } else { 410. };
        let can_remove = !self.saving;
        let blocks = anchors.into_iter().map(|(range, url)| {
            let file = attachments::resolve(&url, &files).cloned();
            if let Some(file) = &file { referenced.insert(file.key()); }
            let renderer = renderer.clone();
            let measure = renderer.clone();
            let measured_file = file.clone();
            InlineBlock {
                range,
                height: Rc::new(move |width| match &measured_file {
                    Some(file) if file.image => px(measure.image_height(file, f32::from(width), cap) + 8.),
                    Some(_) => px(0.),
                    None => px(34.),
                }),
                draw: Rc::new(move |width, cx| match &file {
                    Some(file) if file.image => renderer.draw(file, f32::from(width), cap, can_remove.then(|| (url.clone(), true)), cx),
                    Some(_) => div().into_any_element(),
                    None => attachments::unavailable(cx),
                }),
            }
        }).collect();
        let extras: Vec<_> = files.iter().filter(|file| {
            if file.image { matches!(file.origin, attachments::Origin::Kept(..)) && !referenced.contains(&file.key()) }
            else { matches!(file.origin, attachments::Origin::Kept(..)) || referenced.contains(&file.key()) }
        }).collect();
        let links = self.field_links(&text, cx);
        let height = f32::from(self.viewport.height);
        let reserve = if self.enlarged { 294. } else { 247. };
        let reserve = reserve + if suggestions { 170. } else { 0. } + if !extras.is_empty() { 70. } else { 0. }
            + if self.sheet_error.is_some() { 48. } else { 0. };
        let limit = if self.enlarged { (height - reserve).max(72.) } else { (height - reserve - 32.).clamp(76., 455.) };
        self.body.update(cx, |field, _| {
            field.set_editor_height(px(if !self.enlarged && text.is_empty() { limit.min(76.) } else { limit }), self.enlarged);
            field.set_decorations(text, blocks, links);
        });
        extras.into_iter().map(|file| renderer.draw(file, 220., 100., None, cx)).collect()
    }
}
