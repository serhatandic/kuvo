//! Resolves numbered references in chat against the conversation's own queue, and draws them as
//! the user decided (11-22, in Linear's style): an item named in the text is a chip where it's
//! written (its status mark, its muted number, its title), and a Work Queue step's row carries its
//! item's chip. A chip opens its item in the Work Queue, and shows its card on hover.
use std::{borrow::Cow, cell::RefCell, collections::{HashMap, HashSet}, rc::Rc, time::Duration};

use gpui::{AnyElement, App, Context, Entity, Task, Window};
use kuvo_box::{BoxClient, messages::{Content, Message}, work_queue::{Item, Mention, QueueSaid}};
use kuvo_ui::{chat_list::ChatListState, markdown::{self, InlineItems}, selection::ItemOverlay};

use super::preview::Reference;

#[derive(Default)]
struct Read {
    busy: bool,
    queue: Option<Rc<QueueSaid>>,
    mentions: HashMap<String, Vec<Mention>>,
    /// What's drawn from the two above, made again as they change.
    items: Option<Rc<Items>>,
}

#[derive(Default)]
pub(crate) struct ChatReferences {
    read: Rc<RefCell<Read>>,
    pending: Option<Task<()>>,
}

pub(crate) fn message_text(message: &Message) -> Vec<&str> {
    match message {
        Message::User(user) => vec![user.metadata.as_ref().and_then(|metadata| metadata.pointer("/kuvo/parts/words")).and_then(serde_json::Value::as_str)
            .or_else(|| user.read_parts.as_ref().map(|parts| parts.words.as_str())).unwrap_or(&user.text)],
        Message::Assistant(step) => step.content.iter().filter_map(|content| match content {
            Content::Text(text) => Some(text.as_str()),
            _ => None,
        }).collect(),
        _ => Vec::new(),
    }
}

impl ChatReferences {
    /// Reads what `texts` name (and the queue they name it in) from the box, when it hasn't yet.
    pub fn refresh<'a, V: 'static>(&mut self, texts: impl Iterator<Item = Cow<'a, str>>, owner: &str, client: &BoxClient, live: bool, list: ChatListState, cx: &mut Context<V>) {
        let mut read = self.read.borrow_mut();
        if read.busy { return; }
        let mut candidates = HashSet::new();
        let texts: Vec<Cow<str>> = texts.filter(|text| text.as_bytes().windows(2).any(|pair| pair[0] == b'#' && pair[1].is_ascii_digit()))
            .filter(|text| candidates.insert(text.clone()))
            .collect();
        let before = read.mentions.len();
        read.mentions.retain(|text, _| candidates.contains(text.as_str()));
        let wanted: Vec<String> = texts.into_iter().filter(|text| !read.mentions.contains_key(text.as_ref())).take(12).map(Cow::into_owned).collect();
        if wanted.is_empty() {
            if read.mentions.len() != before { read.items = made(&read, client, live); }
            return;
        }
        if !live {
            let (queue, mentions) = resolve(client, owner, wanted);
            read.queue = queue.map(Rc::new);
            read.mentions.extend(mentions);
            read.items = made(&read, client, live);
            list.remeasure();
            return;
        }
        read.busy = true;
        drop(read);
        let owner = owner.to_owned();
        let client = client.clone();
        let read = self.read.clone();
        self.pending = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            let asked = client.clone();
            let answer = cx.background_executor().spawn(async move { resolve(&asked, &owner, wanted) }).await;
            this.update(cx, |_, cx| {
                let mut read = read.borrow_mut();
                read.busy = false;
                read.queue = answer.0.map(Rc::new);
                read.mentions.extend(answer.1);
                read.items = made(&read, &client, live);
                list.remeasure();
                cx.notify();
            }).ok();
        }));
    }

    /// What draws the items named, once their queue is read.
    pub fn items(&self) -> Option<Rc<Items>> {
        self.read.borrow().items.clone()
    }
}

fn made(read: &Read, client: &BoxClient, live: bool) -> Option<Rc<Items>> {
    let queue = read.queue.clone()?;
    Some(Rc::new(Items { queue, mentions: read.mentions.clone(), client: client.clone(), live }))
}

/// The items a conversation names, as its queue has them: how each reads in the text it's named in,
/// and its chip.
pub struct Items {
    queue: Rc<QueueSaid>,
    mentions: HashMap<String, Vec<Mention>>,
    client: BoxClient,
    live: bool,
}

impl Items {
    fn numbered(&self, number: u64) -> Option<&Item> {
        self.queue.items.iter().find(|item| item.number == Some(number))
    }

    fn mentioned(&self, mention: &Mention) -> Option<&Item> {
        match mention.item.as_deref() {
            Some(id) => self.queue.items.iter().find(|item| item.id == id),
            None => self.numbered(mention.number),
        }
    }

    /// `text` with each item it names (outside code and links) as its label, which the text draws
    /// as its chip ([`markdown::item_label`]); escaped for Markdown when it's `markdown`.
    pub fn labeled<'a>(&self, text: &'a str, markdown: bool) -> Cow<'a, str> {
        let Some(mentions) = self.mentions.get(text) else { return Cow::Borrowed(text) };
        let kept = if markdown { markdown::protected_ranges(text) } else { Vec::new() };
        let edits: Vec<_> = super::references::ranges(text, mentions).into_iter()
            .filter(|(range, _)| !kept.iter().any(|kept| kept.start < range.end && range.start < kept.end))
            .filter_map(|(range, mention)| {
                let item = self.mentioned(mention)?;
                let label = markdown::item_label(item.number.unwrap_or(mention.number), &item.title);
                Some((range, if markdown { escaped(&label) } else { label }))
            }).collect();
        if edits.is_empty() { return Cow::Borrowed(text); }
        let mut out = text.to_owned();
        for (range, label) in edits.into_iter().rev() {
            out.replace_range(range, &label);
        }
        Cow::Owned(out)
    }

    fn reference(&self, number: u64) -> Option<Reference> {
        Some(Reference::new(self.numbered(number)?.clone(), self.queue.clone(), self.client.clone(), self.live))
    }

    /// What draws the chips of the items named in Markdown or plain text, by their numbers.
    pub fn inline(self: &Rc<Self>) -> InlineItems {
        let items = self.clone();
        Rc::new(move |number: u64, _: &App| {
            let reference = items.reference(number)?;
            let open = open_item(reference.item.id.clone());
            let overlay: ItemOverlay = Rc::new(move |bounds: gpui::Bounds<gpui::Pixels>, _: &mut Window, cx: &mut App| reference.inline_chip(bounds.size, open.clone(), cx));
            Some(overlay)
        })
    }

    /// Item `number`'s chip, as a Work Queue step's row shows it.
    pub fn row_chip(&self, number: u64, key: &str, cx: &App) -> Option<AnyElement> {
        let reference = self.reference(number)?;
        let open = open_item(reference.item.id.clone());
        Some(reference.row_chip(gpui::SharedString::from(format!("row-item-{key}")), move |window, cx| open(window, cx), cx))
    }

    /// Item `number`'s title, as its queue has it.
    pub fn title(&self, number: u64) -> Option<&str> {
        self.numbered(number).map(|item| item.title.as_str())
    }
}

/// Markdown that reads as `text`: its punctuation escaped.
fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for ch in text.chars() {
        if ch.is_ascii_punctuation() { out.push('\\'); }
        out.push(ch);
    }
    out
}

/// Opens item `id` in the Work Queue, picked, from whichever window the click was in.
fn open_item(id: String) -> Rc<dyn Fn(&mut Window, &mut App)> {
    Rc::new(move |window, cx| {
        if let Some(workspace) = workspace_in(window, cx) {
            workspace.update(cx, |workspace, cx| workspace.open_queue_reference(&id, window, cx));
        }
    })
}

/// The workspace a window shows: the window itself, the app's home in it, or the home the app's
/// opening ([`crate::launch::Launch`]) turned into, as the app's own window is.
pub(crate) fn workspace_in(window: &mut Window, cx: &App) -> Option<Entity<crate::Workspace>> {
    if let Some(workspace) = window.root::<crate::Workspace>().flatten() {
        return Some(workspace);
    }
    let home = window.root::<crate::home::Home>().flatten()
        .or_else(|| window.root::<crate::launch::Launch>().flatten().and_then(|launch| launch.read(cx).home().cloned()))?;
    Some(home.read(cx).workspace().clone())
}

fn resolve(client: &BoxClient, owner: &str, wanted: Vec<String>) -> (Option<QueueSaid>, Vec<(String, Vec<Mention>)>) {
    let queue = client.work_queue_for_owner(owner).ok();
    let mentions = wanted.into_iter().map(|text| {
        let mentions = if queue.as_ref().is_some_and(|queue| !queue.items.is_empty()) {
            client.work_item_mentions(&text).unwrap_or_default()
        } else { Vec::new() };
        (text, mentions)
    }).collect();
    (queue, mentions)
}
