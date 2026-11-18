use std::{ops::Range, rc::Rc, time::Duration};

use gpui::{App, Context, ElementId, Focusable as _, SharedString, Window};
use kuvo_box::work_queue::{Item, Mention};
use kuvo_ui::text_field::FieldLink;

use super::{Act, QueueView, Shown};

/// The box returns UTF-16 offsets. GPUI's text layouts and edits use UTF-8 byte offsets.
pub(super) fn ranges<'a>(text: &str, mentions: &'a [Mention]) -> Vec<(Range<usize>, &'a Mention)> {
    let offsets: std::collections::HashMap<_, _> = text.char_indices().chain([(text.len(), '\0')])
        .scan(0, |units, (offset, char)| {
            let pair = (*units, offset);
            *units += char.len_utf16();
            Some(pair)
        }).collect();
    mentions.iter().filter_map(|mention| {
        let (start, end) = (*offsets.get(&mention.start)?, *offsets.get(&mention.end)?);
        let token = text.get(start..end)?.strip_prefix('#')?;
        (token.parse::<u64>().ok() == Some(mention.number)).then_some((start..end, mention))
    }).collect()
}

pub(super) fn completion(text: &str, selection: Range<usize>) -> Option<(Range<usize>, &str)> {
    if !selection.is_empty() { return None; }
    let end = selection.end;
    let start = text[..end].rfind('#')?;
    let query = &text[start + 1..end];
    if query.len() > 48 || query.chars().any(|ch| !ch.is_alphanumeric() && ch != '-' && ch != '_') { return None; }
    if text[..start].chars().next_back().is_some_and(|ch| !ch.is_whitespace() && !"([{\"'/,–-“‘".contains(ch)) { return None; }
    Some((start..end, query))
}

impl QueueView {
    pub(super) fn numbered_item(&self, number: u64) -> Option<&Item> {
        self.queue()?.items.iter().find(|item| item.number == Some(number))
    }

    pub(super) fn suggest_items(&self, query: &str) -> Vec<(u64, String)> {
        let q = query.to_lowercase();
        let mut items: Vec<_> = self.queue().into_iter().flat_map(|queue| &queue.items)
            .filter_map(|item| {
                let number = item.number?;
                (number.to_string().starts_with(&q) || item.title.to_lowercase().contains(&q)).then_some((number, item.title.clone()))
            }).collect();
        items.sort_by_key(|(number, _)| (query.parse::<u64>().ok() != Some(*number), std::cmp::Reverse(*number)));
        items.truncate(6);
        items
    }

    pub(super) fn take_suggestion(&mut self, number: u64, cx: &mut Context<Self>) {
        self.body.update(cx, |field, cx| {
            if let Some((range, _)) = completion(field.text(), field.selection()) {
                field.select_range(range, cx);
                field.replace_selection(&format!("#{number} "), cx);
            }
        });
        self.wants_focus = Some(self.body.focus_handle(cx));
        self.suggestion_at = 0;
        cx.notify();
    }

    pub(super) fn refresh_draft_mentions(&mut self, cx: &mut Context<Self>) {
        self.suggestion_at = 0;
        self.suggestions_dismissed = false;
        let text = self.body.read(cx).text().to_owned();
        if self.draft_mentions.0 == text || !text.contains('#') {
            if !text.contains('#') { self.draft_mentions = (text, Vec::new()); }
            self.tasks.remove("draft-mentions");
            cx.notify();
            return;
        }
        let opened = self.sheet_opened;
        if !self.live {
            self.draft_mentions = (text.clone(), self.client.work_item_mentions(&text).unwrap_or_default());
            cx.notify();
            return;
        }
        let client = self.client.clone();
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(140)).await;
            let typed = text.clone();
            let mentions = cx.background_executor().spawn(async move { client.work_item_mentions(&typed).unwrap_or_default() }).await;
            this.update(cx, |this, cx| {
                if this.sheet_opened == opened && this.body.read(cx).text() == text {
                    this.draft_mentions = (text, mentions);
                    cx.notify();
                }
            }).ok();
        });
        self.tasks.insert("draft-mentions".into(), task);
        cx.notify();
    }

    pub(super) fn field_links(&self, text: &str, cx: &Context<Self>) -> Vec<FieldLink> {
        let mentions = if let Some(whole) = self.whole().filter(|whole| whole.body == text) {
            &whole.mentions
        } else if self.draft_mentions.0 == text {
            &self.draft_mentions.1
        } else {
            return Vec::new();
        };
        ranges(text, mentions).into_iter().filter_map(|(range, mention)| {
            self.mentioned_item(mention)?;
            let open = self.reference_click(cx);
            let url = format!("kuvo-item:{}", mention_key(mention));
            Some(FieldLink { range, open: Rc::new(move |window, cx| open(&url, window, cx)) })
        }).collect()
    }

    pub(super) fn reference_click(&self, cx: &Context<Self>) -> kuvo_ui::markdown::LinkClick {
        let queue = cx.entity().downgrade();
        Rc::new(move |url: &str, _: &mut Window, cx: &mut App| {
            let Some(key) = url.strip_prefix("kuvo-item:") else { return };
            queue.update(cx, |queue, cx| {
                let item = match key.parse() {
                    Ok(number) => queue.numbered_item(number),
                    Err(_) => queue.queue().and_then(|said| said.items.iter().find(|item| item.id == key)),
                };
                if let Some(id) = item.map(|item| item.id.clone()) {
                    queue.open_reference(id, cx);
                }
            }).ok();
        })
    }

    pub(super) fn open_reference(&mut self, id: String, cx: &mut Context<Self>) {
        if self.shown == Shown::Item(id.clone()) || self.saving { return; }
        if self.sheet.is_none() {
            self.act(Act::Open(id), cx);
            return;
        }
        if self.return_to_draft.is_none() {
            self.return_to_draft = Some((self.shown.clone(), self.whole.clone()));
        }
        self.picker = None;
        self.shown = Shown::Item(id);
        self.whole = crate::areas::Reading::NotYet;
        self.list.scroll_to(gpui::ListOffset::default());
        self.wants_focus = Some(self.focus.clone());
        self.read(cx);
        self.rebuild(cx);
    }

    fn mentioned_item(&self, mention: &Mention) -> Option<&Item> {
        match mention.item.as_deref() {
            Some(id) => self.queue()?.items.iter().find(|item| item.id == id),
            None => self.numbered_item(mention.number),
        }
    }

    pub(super) fn item_reference_markdown(&self, id: impl Into<ElementId>, text: &str, mentions: &[Mention], files: &[super::attachments::Attachment], cx: &Context<Self>) -> kuvo_ui::markdown::Markdown {
        let mut edits: Vec<_> = ranges(text, mentions).into_iter().filter(|(_, mention)| self.mentioned_item(mention).is_some())
            .map(|(range, mention)| {
                let label = &text[range.clone()];
                (range, format!("[{label}](kuvo-item:{})", mention_key(mention)))
            }).collect();
        edits.extend(super::attachments::anchors(text, files).into_iter()
            .filter_map(|(range, url)| {
                if super::attachments::resolve(&url, files).is_some_and(|file| !file.image) {
                    Some((range, String::new()))
                } else if super::attachments::is_legacy(&url) {
                    Some((range, super::attachments::legacy_markdown(&url)))
                } else {
                    None
                }
            }));
        edits.sort_by_key(|(range, _)| range.start);
        let mut source = text.to_owned();
        for (range, value) in edits.into_iter().rev() {
            source.replace_range(range, &value);
        }
        let queue = self.queue().cloned().map(Rc::new);
        let items: std::collections::HashMap<_, _> = mentions.iter().filter_map(|mention| {
            let item = self.mentioned_item(mention)?.clone();
            Some((mention_key(mention), super::preview::Reference::new(item, queue.clone()?, self.client.clone(), self.live)))
        }).collect();
        let open = self.reference_click(cx);
        let chip_open = open.clone();
        kuvo_ui::markdown::Markdown::new(id, "").item_links(&source, open)
            .item_chips(Rc::new(move |key, cx| {
                let item = items.get(key)?;
                let open = chip_open.clone();
                let url = format!("kuvo-item:{key}");
                Some(item.chip(SharedString::from(key.to_owned()), move |window, cx| open(&url, window, cx), cx))
            }))
    }
}

fn mention_key(mention: &Mention) -> String {
    mention.item.clone().unwrap_or_else(|| mention.number.to_string())
}
