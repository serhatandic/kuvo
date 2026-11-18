//! Resolves numbered references in chat against the conversation's own queue.
use std::{cell::RefCell, collections::{HashMap, HashSet}, rc::Rc, time::Duration};

use gpui::{AnyElement, App, Context, IntoElement, ParentElement, Styled, Task, div};
use kuvo_box::{BoxClient, messages::{Content, Message}, work_queue::QueueSaid};
use kuvo_ui::{chat_list::ChatListState, theme::space};

use super::preview::Reference;

#[derive(Default)]
struct Read {
    queue: Option<Rc<QueueSaid>>,
    busy: bool,
    numbers: HashMap<String, Vec<u64>>,
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
    pub fn refresh<'a, V: 'static>(&mut self, texts: impl Iterator<Item = &'a str>, owner: &str, client: &BoxClient, live: bool, list: ChatListState, cx: &mut Context<V>) {
        let mut read = self.read.borrow_mut();
        if read.busy { return; }
        let mut candidates = HashSet::new();
        let texts: Vec<_> = texts.filter(|text| text.as_bytes().windows(2).any(|pair| pair[0] == b'#' && pair[1].is_ascii_digit()))
            .filter(|text| candidates.insert(*text))
            .collect();
        read.numbers.retain(|text, _| candidates.contains(text.as_str()));
        let wanted: Vec<_> = texts.into_iter().filter(|text| !read.numbers.contains_key(*text)).take(12).map(str::to_owned).collect();
        if wanted.is_empty() { return; }
        if !live {
            let (queue, numbers) = resolve(client, owner, wanted);
            read.queue = queue.map(Rc::new);
            read.numbers.extend(numbers);
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
            let answer = cx.background_executor().spawn(async move { resolve(&client, &owner, wanted) }).await;
            this.update(cx, |_, cx| {
                let mut read = read.borrow_mut();
                read.busy = false;
                read.queue = answer.0.map(Rc::new);
                read.numbers.extend(answer.1);
                list.remeasure();
                cx.notify();
            }).ok();
        }));
    }

    pub fn chips<'a>(&self, texts: impl IntoIterator<Item = &'a str>, client: &BoxClient, live: bool, cx: &App) -> Option<AnyElement> {
        let read = self.read.borrow();
        let queue = read.queue.as_ref()?;
        let mut seen = HashSet::new();
        let chips = texts.into_iter().flat_map(|text| read.numbers.get(text).into_iter().flatten().copied())
            .filter(|number| seen.insert(*number))
            .filter_map(|number| queue.items.iter().find(|item| item.number == Some(number)).map(|item| (number, item)))
            .map(|(number, item)| {
                let reference = Reference::new(item.clone(), queue.clone(), client.clone(), live);
                let id = item.id.clone();
                reference.chip(("chat-item-reference", number), move |window, cx| {
                    let workspace = window.root::<crate::Workspace>().flatten().or_else(|| {
                        window.root::<crate::home::Home>().flatten().map(|home| home.read(cx).workspace().clone())
                    });
                    if let Some(workspace) = workspace {
                        workspace.update(cx, |workspace, cx| workspace.open_queue_reference(&id, window, cx));
                    }
                }, cx)
            }).collect::<Vec<_>>();
        (!chips.is_empty()).then(|| div().flex().flex_wrap().gap(space::S1_5).mt(space::S1_5).children(chips).into_any_element())
    }
}


fn resolve(client: &BoxClient, owner: &str, wanted: Vec<String>) -> (Option<QueueSaid>, Vec<(String, Vec<u64>)>) {
    let queue = client.work_queue_for_owner(owner).ok();
    let numbers = wanted.into_iter().map(|text| {
        let numbers = if queue.as_ref().is_some_and(|queue| !queue.items.is_empty()) {
            client.work_item_mentions(&text).unwrap_or_default().into_iter().map(|mention| mention.number).collect()
        } else { Vec::new() };
        (text, numbers)
    }).collect();
    (queue, numbers)
}
