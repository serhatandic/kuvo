//! One group chat's conversation: its messages in the app's chat list, each bot's marked by who
//! wrote it (its still face and name, a wrap-up or a report said so), yours in your bubble, the
//! chat's own notes and meetings as quiet lines; who's talking and what they're doing at the end;
//! and the composer, whose "&" offers the chat's bots. Everything is the box's (`kuvo/groups/<id>`,
//! followed): the app draws it, and sends what you write, Stop and Keep Going to its routes.
//!
//! Its messages are drawn as a session's are (`entries::message_body`): your words as your
//! bubble, a bot's steps and words as an agent's. One selection runs over all of them.

use std::collections::HashSet;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Task, Window, div, prelude::FluentBuilder as _, px,
};
use kuvo_box::BoxClient;
use kuvo_box::areas::{Bot, Look};
use kuvo_box::follow::{Follow, Followed};
use kuvo_box::groups::{Activity, Chat, Group, GroupMessage, GroupTask};
use kuvo_box::messages::{Content, Message, Step, Time, UserMessage};
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TypeSize,
    chat_list::{ChatListState, chat_list},
    menu::{ContextMenuArea, Menu, MenuEntry},
    selection::{self, SelectionArea, SelectionState},
    small::spinner,
    theme::{radius, space},
};

use crate::areas::bot_face;
use crate::composer::{Composer, ComposerEvent};
use crate::entries::{self, COLUMN, Fold};

/// How far a bot's words sit in from its face, so its messages line up under its name.
const INDENT: f32 = 34.;

/// What the chat asks of the window around it.
#[derive(Clone, Debug, PartialEq)]
pub enum ChatEvent {
    /// Open a bot's own conversation.
    BotChat { bot: String, session: String },
    /// The chat changed (read, written to): the list is to be read again.
    Changed,
}

/// Who wrote a bot's message, as the box's lists say it: one of the chat's bots, a bot of the
/// box's that has left the chat, or one that's gone (none lists it).
#[derive(Clone, Debug, PartialEq)]
pub enum Author {
    Member { id: String, name: String, look: Option<Look>, session: Option<String> },
    Left { name: String, look: Look },
    Gone,
}

/// One row of the chat.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    /// A message, and how a session's conversation draws it.
    Message { said: Box<GroupMessage>, drawn: Box<Message> },
    /// Who's talking, and what they're doing.
    Talking,
}

impl Row {
    fn reference_text(&self) -> Vec<&str> {
        match self {
            Row::Message { drawn, .. } => crate::queue::chat_references::message_text(drawn),
            Row::Talking => Vec::new(),
        }
    }

    pub fn key(&self) -> &str {
        match self {
            Row::Message { said, .. } => &said.id,
            Row::Talking => "talking",
        }
    }
}

pub struct GroupChatView {
    client: BoxClient,
    chat_references: crate::queue::chat_references::ChatReferences,
    live: bool,
    id: String,
    /// The chat as the box lists it (its name, purpose, bots), and the box's bots (who a bot that
    /// left the chat was).
    group: Option<Group>,
    bots: Vec<Bot>,
    read: bool,
    problem: Option<String>,
    chat: Chat,
    messages: Vec<GroupMessage>,
    rows: Vec<Row>,
    list: ChatListState,
    open: HashSet<Fold>,
    selection: SelectionState,
    transcript: FocusHandle,
    focus: FocusHandle,
    composer: Entity<Composer>,
    /// Whether it's on screen: only then is what's in it marked read.
    shown: bool,
    /// The last message there was when it was last marked read.
    viewed: Option<Option<String>>,
    /// Why Keep Going or handing out the work didn't go, until the next change.
    act_failed: Option<String>,
    follow: Option<Rc<Follow<Followed<Chat>>>>,
    follow_task: Option<Task<()>>,
    tasks: Vec<Task<()>>,
    _composer: Subscription,
    /// When times are read from (none: the clock).
    now: Option<f64>,
}

impl EventEmitter<ChatEvent> for GroupChatView {}

impl Focusable for GroupChatView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.composer.focus_handle(cx)
    }
}

impl GroupChatView {
    /// Chat `id` of `client`'s box. `live`: follow it there (the app); else it's read at once as
    /// it opens and after each change (tests and shots).
    pub fn new(client: BoxClient, id: String, group: Option<Group>, live: bool, cx: &mut Context<Self>) -> GroupChatView {
        let name = group.as_ref().map(|group| group.name.clone()).unwrap_or_default();
        let composer = cx.new(|cx| Composer::for_group(client.clone(), id.clone(), &name, live, cx));
        if let Some(group) = &group {
            let members = group.members.clone();
            composer.update(cx, |composer, cx| composer.set_members(members, cx));
        }
        let subscription = cx.subscribe(&composer, |this: &mut GroupChatView, _, event: &ComposerEvent, cx| {
            if matches!(event, ComposerEvent::Wrote) {
                this.act_failed = None;
                this.refresh(cx);
            }
        });
        let mut view = GroupChatView {
            client: client.clone(),
            chat_references: Default::default(),
            live,
            id: id.clone(),
            group,
            bots: Vec::new(),
            read: false,
            problem: None,
            chat: Chat::default(),
            messages: Vec::new(),
            rows: Vec::new(),
            list: ChatListState::new(0, cx),
            open: HashSet::new(),
            selection: SelectionState::new(),
            transcript: cx.focus_handle(),
            focus: cx.focus_handle(),
            composer,
            shown: false,
            viewed: None,
            act_failed: None,
            follow: None,
            follow_task: None,
            tasks: Vec::new(),
            _composer: subscription,
            now: None,
        };
        // (The app follows it while it's on screen: `set_shown`.)
        if !live {
            view.refresh(cx);
        }
        view
    }

    /// Follows the chat on its box, as it comes on screen.
    fn follow(&mut self, cx: &mut Context<Self>) {
        if !self.live || self.follow.is_some() {
            return;
        }
        let follow = Rc::new(self.client.follow_group(&self.id));
        let updates = follow.updates().clone();
        self.follow = Some(follow);
        self.follow_task = Some(cx.spawn(async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                if this.update(cx, |this, cx| this.take(update, cx)).is_err() {
                    break;
                }
            }
        }));
    }

    /// Lets it go, off screen: it's followed again as it comes back.
    fn unfollow(&mut self) {
        if let Some(follow) = self.follow.take() {
            follow.stop();
        }
        self.follow_task = None;
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn composer(&self) -> &Entity<Composer> {
        &self.composer
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Its messages' list, scrolled as it's drawn.
    pub fn list(&self) -> &ChatListState {
        &self.list
    }

    pub fn chat(&self) -> &Chat {
        &self.chat
    }

    pub fn group(&self) -> Option<&Group> {
        self.group.as_ref()
    }

    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// The chat as the box lists it now, and the box's bots.
    pub fn set_listed(&mut self, group: Option<Group>, bots: Vec<Bot>, cx: &mut Context<Self>) {
        if let Some(group) = &group {
            let (name, members) = (group.name.clone(), group.members.clone());
            self.composer.update(cx, |composer, cx| {
                composer.set_group_name(&name, cx);
                composer.set_members(members, cx);
            });
        }
        // Who wrote each message is drawn from these: every row again where any of them changed,
        // the reader kept where they are.
        // Not read yet (it couldn't be): asked again now it's listed (the app's follow retries).
        if !self.live && !self.read {
            let read = match self.client.group_chat(&self.id) {
                Ok(chat) => Followed::Now(chat),
                Err(error) => Followed::Failed(error),
            };
            self.take(read, cx);
        }
        let redraw = authors(self.group.as_ref(), &self.bots) != authors(group.as_ref(), &bots);
        self.group = group;
        self.bots = bots;
        if redraw {
            for ix in 0..self.rows.len() {
                self.list.changed(ix);
            }
        }
        cx.notify();
    }

    /// Whether it's on screen: what's in it is marked read only then.
    pub fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        if self.shown == shown {
            return;
        }
        self.shown = shown;
        self.composer.update(cx, |composer, cx| composer.set_shown(shown, cx));
        if shown {
            self.follow(cx);
            self.mark_viewed(cx);
        } else {
            self.unfollow();
        }
    }

    pub fn is_shown(&self) -> bool {
        self.shown
    }

    /// Reads it again at once (tests and shots; the app follows it).
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.live {
            // (Its follow says what changed in it; the list of chats is read again.)
            cx.emit(ChatEvent::Changed);
            return;
        }
        let read = match self.client.group_chat(&self.id) {
            Ok(chat) => Followed::Now(chat),
            Err(error) => Followed::Failed(error),
        };
        self.take(read, cx);
        cx.emit(ChatEvent::Changed);
    }

    /// What the box says of it now.
    pub fn take(&mut self, update: Followed<Chat>, cx: &mut Context<Self>) {
        match update {
            Followed::Now(chat) => {
                self.problem = None;
                self.read = true;
                // A line whose messages are as they were leaves them out: those shown stay.
                if let Some(messages) = &chat.messages {
                    self.messages = messages.clone();
                }
                self.chat = chat;
                self.composer.update(cx, |composer, cx| composer.set_running(self.chat.discussing, cx));
                self.show_rows();
                self.mark_viewed(cx);
            }
            Followed::Failed(error) => self.problem = Some(kuvo_box::accounts::said(&error)),
        }
        cx.notify();
    }

    /// The rows for what's said now, the list kept where the reader is: new ones added at the
    /// end (followed while the reader's at the bottom), changed ones measured again.
    fn show_rows(&mut self) {
        let mut rows: Vec<Row> = self.messages.iter().map(|said| Row::Message { drawn: Box::new(drawn(said)), said: Box::new(said.clone()) }).collect();
        if self.chat.speaking.is_some() {
            rows.push(Row::Talking);
        }
        let old = std::mem::replace(&mut self.rows, rows);
        self.selection.set_rows(self.rows.iter().map(Row::key));
        let kept = old.iter().filter(|row| matches!(row, Row::Message { .. })).count();
        let same_start = kept <= self.messages.len() && old.iter().zip(&self.rows).take(kept).all(|(was, now)| was.key() == now.key());
        if old.is_empty() || !same_start {
            self.list.reset(self.rows.len());
            return;
        }
        let state = self.list.list_state().clone();
        // Who's talking goes from the end before what's new is added after the rest.
        let mut count = old.len();
        if matches!(old.last(), Some(Row::Talking)) {
            state.splice(count - 1..count, 0);
            count -= 1;
        }
        for (ix, (was, now)) in old.iter().zip(&self.rows).take(count).enumerate() {
            if was != now {
                self.list.changed(ix);
            }
        }
        let added = self.rows.len() - count;
        if added > 0 {
            self.list.push(added);
        }
        debug_assert_eq!(state.item_count(), self.rows.len());
    }

    /// Says it's been read, once for each message come while it's on screen.
    fn mark_viewed(&mut self, cx: &mut Context<Self>) {
        let unread = self.group.as_ref().is_some_and(|group| group.unread);
        let last = self.messages.last().map(|message| message.id.clone());
        if !self.shown || !self.read || (self.viewed.as_ref() == Some(&last) && !unread) {
            return;
        }
        self.viewed = Some(last);
        let (client, id) = (self.client.clone(), self.id.clone());
        if !self.live {
            let _ = client.group_viewed(&id);
            if unread {
                cx.emit(ChatEvent::Changed);
            }
            return;
        }
        let task = cx.background_executor().spawn(async move { client.group_viewed(&id) });
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(cx.spawn(async move |this, cx| {
            if task.await.is_ok() && unread {
                this.update(cx, |_, cx| cx.emit(ChatEvent::Changed)).ok();
            }
        }));
    }

    /// Runs `work` against the box, then reads the chat again (or says why it didn't go).
    fn act(&mut self, cx: &mut Context<Self>, work: impl FnOnce(&BoxClient, &str) -> kuvo_box::Result<()> + Send + 'static) {
        let (client, id) = (self.client.clone(), self.id.clone());
        if !self.live {
            let result = work(&client, &id);
            self.acted(result, cx);
            return;
        }
        let task = cx.background_executor().spawn(async move { work(&client, &id) });
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| this.acted(result, cx)).ok();
        }));
    }

    fn acted(&mut self, result: kuvo_box::Result<()>, cx: &mut Context<Self>) {
        match result {
            Ok(()) => {
                self.act_failed = None;
                self.refresh(cx);
            }
            Err(error) => self.act_failed = Some(kuvo_box::accounts::said(&error)),
        }
        cx.notify();
    }

    pub fn keep_going(&mut self, cx: &mut Context<Self>) {
        self.act(cx, |client, id| client.keep_going(id));
    }

    /// Resumes a bot's own conversation, which its part waits on.
    pub fn resume(&mut self, session: String, cx: &mut Context<Self>) {
        self.act(cx, move |client, _| client.resume_queue(&session));
    }

    pub fn start_work(&mut self, message: String, cx: &mut Context<Self>) {
        self.act(cx, move |client, id| client.start_work(id, &message));
    }

    /// Opens or closes a bot's step.
    pub fn toggle(&mut self, fold: Fold, cx: &mut Context<Self>) {
        let step = fold.step().to_owned();
        if !self.open.remove(&fold) {
            self.open.insert(fold);
        }
        if let Some(ix) = self.rows.iter().position(|row| row.key() == step) {
            self.list.changed(ix);
        }
        cx.notify();
    }

    /// Who wrote a bot's message, as the box's lists say.
    pub fn author(&self, bot: &str) -> Author {
        if let Some(member) = self.group.as_ref().and_then(|group| group.member(bot)) {
            return Author::Member { id: member.id.clone(), name: member.name.clone(), look: member.look.clone(), session: member.openable_session().map(str::to_owned) };
        }
        match self.bots.iter().find(|each| each.id == bot) {
            Some(bot) => Author::Left { name: bot.name.clone(), look: bot.look.clone() },
            None => Author::Gone,
        }
    }

    /// A bot's name, as the chat or the box lists it.
    fn name_of(&self, bot: &str) -> Option<String> {
        match self.author(bot) {
            Author::Member { name, .. } | Author::Left { name, .. } => Some(name),
            Author::Gone => None,
        }
    }

    // MARK: Selecting

    fn units(&self) -> Vec<(SharedString, Vec<selection::Unit>)> {
        let items = self.chat_references.items();
        self.rows.iter().map(|row| (SharedString::from(row.key().to_owned()), row_units(row, &self.open, items.as_deref()))).collect()
    }

    /// How its rows draw what they show: the Work Queue items they name, as chips.
    fn looks(&self) -> crate::pictures::Looks<'static> {
        crate::pictures::Looks { items: self.chat_references.items(), ..crate::pictures::Looks::none() }
    }

    /// What's selected, as it's copied.
    pub fn selected_text(&self) -> String {
        let Some((start, end)) = self.selection.range() else { return String::new() };
        let order = self.selection.order();
        let rows = self.rows.iter().filter(|row| {
            let at = selection::Spot::row_start(row.key().to_owned());
            order.cmp(&at, &selection::Spot::row_start(start.unit.row.clone())) != Some(std::cmp::Ordering::Less)
                && order.cmp(&at, &selection::Spot::row_start(end.unit.row.clone())) != Some(std::cmp::Ordering::Greater)
        });
        let items = self.chat_references.items();
        let text = selection::copy_text(&order, rows.map(|row| (SharedString::from(row.key().to_owned()), row_units(row, &self.open, items.as_deref()))), &start, &end);
        kuvo_ui::markdown::plain_items(&text)
    }

    /// Selects from the first `from` to the end of the first `to` after it (for shots and tests).
    pub fn select_text(&mut self, from: &str, to: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let found = self.selection.select_text(&self.units(), from, to);
        window.focus(&self.transcript, cx);
        cx.notify();
        found
    }

    fn copy(&mut self, _: &selection::Copy, _: &mut Window, cx: &mut Context<Self>) {
        selection::copy_to_clipboard(self.selected_text(), cx);
    }

    fn select_all(&mut self, _: &selection::SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        let rows: Vec<&str> = self.rows.iter().filter(|row| !row_units(row, &self.open, None).is_empty()).map(Row::key).collect();
        if let (Some(first), Some(last)) = (rows.first(), rows.last()) {
            self.selection.select_all(first, last);
            cx.notify();
        }
    }

    fn transcript_menu(view: &Entity<GroupChatView>, window: &mut Window, cx: &mut App) -> Entity<Menu> {
        let selection = view.read(cx).selection.clone();
        let link = selection.link_at(window.mouse_position());
        let has_selection = !selection.is_empty();
        let (copy_view, all_view) = (view.clone(), view.clone());
        cx.new(|cx| {
            let mut menu = Menu::new(cx);
            if let Some(link) = link {
                menu = menu.entry(MenuEntry::new("Copy Link").on_select(move |_, cx| selection::copy_to_clipboard(link.clone(), cx))).separator();
            }
            menu.entry(MenuEntry::new("Copy").shortcut(selection::COPY_KEYS).disabled(!has_selection).on_select(move |_, cx| {
                let text = copy_view.read(cx).selected_text();
                selection::copy_to_clipboard(text, cx);
            }))
            .entry(MenuEntry::new("Select All").shortcut(selection::SELECT_ALL_KEYS).on_select(move |window, cx| {
                all_view.update(cx, |this, cx| this.select_all(&selection::SelectAll, window, cx));
            }))
        })
    }
}

/// What names a message's author: the chat's bots and the box's, each by id, name and look.
fn authors(group: Option<&Group>, bots: &[Bot]) -> Vec<(String, String, Option<Look>)> {
    let members = group.into_iter().flat_map(|group| group.members.iter().map(|member| (member.id.clone(), member.name.clone(), member.look.clone())));
    members.chain(bots.iter().map(|bot| (bot.id.clone(), bot.name.clone(), Some(bot.look.clone())))).collect()
}

/// A group message as a session's conversation draws it: yours as your message, a bot's as an
/// agent's step (its tool calls, then its words), the chat's own as a quiet line.
pub fn drawn(said: &GroupMessage) -> Message {
    let time = Time { created: said.at_ms(), completed: None };
    match (&said.author, said.kind.as_str()) {
        (None, "message") => Message::User(UserMessage { id: said.id.clone(), time, text: said.text.clone(), files: Vec::new(), metadata: None, read_parts: None }),
        (Some(_), _) => {
            let mut content: Vec<Content> = said.steps.iter().filter_map(|step| serde_json::from_value(step.clone()).ok()).filter(|step| matches!(step, Content::Tool { .. })).collect();
            content.push(Content::Text(said.text.clone()));
            Message::Assistant(Step { id: said.id.clone(), time, agent: None, model: None, content, finish: None, error: None })
        }
        // A note, a meeting, or a kind a newer box adds: its words, quietly.
        (None, _) => Message::System(kuvo_box::messages::Note { id: said.id.clone(), time, text: said.text.clone(), description: None, metadata: None }),
    }
}

fn row_units(row: &Row, open: &HashSet<Fold>, items: Option<&crate::queue::chat_references::Items>) -> Vec<selection::Unit> {
    match row {
        Row::Message { said, drawn } if said.author.is_some() || said.kind == "message" => entries::message_units(drawn, open, items),
        _ => Vec::new(),
    }
}

/// Names joined as a sentence: "Mira", "Mira and Bo", "Mira, Bo and Cy".
pub fn joined(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// A face for a bot no list knows: a quiet circle with a bot in it.
pub fn blank_face(size: gpui::Pixels, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div().flex().flex_none().items_center().justify_center().size(size).rounded(radius::PILL).bg(c.sunken).child(Icon::new(IconName::Bot).size(IconSize::Nav).color(c.icon)).into_any_element()
}

/// A member's face (the bot `id`'s creature), or the blank one when its look isn't known.
pub fn face(id: &str, look: Option<&Look>, size: gpui::Pixels, cx: &App) -> AnyElement {
    match look {
        Some(look) => bot_face(id, look, size),
        None => blank_face(size, cx),
    }
}

/// How long ago a message was said: "now", "12m ago", "1d ago".
fn sent_at(at_ms: f64, now_ms: f64) -> String {
    let ago = crate::time::ago(at_ms, now_ms);
    if ago == "now" { "now".into() } else { format!("{ago} ago") }
}

impl GroupChatView {
    fn row(&self, ix: usize, view: &Entity<GroupChatView>, cx: &App) -> AnyElement {
        let Some(row) = self.rows.get(ix) else { return div().into_any_element() };
        let toggle_view = view.clone();
        let toggle = move |fold: Fold, cx: &mut App| toggle_view.update(cx, |this, cx| this.toggle(fold, cx));
        let c = &cx.theme().colors;
        let body: AnyElement = match row {
            Row::Talking => self.talking(cx),
            Row::Message { said, drawn } => match (&said.author, said.kind.as_str()) {
                (None, "message") => entries::message_body(ix, &said.id, drawn, &self.open, toggle, &self.selection, &self.looks(), cx),
                (None, "meeting") => div()
                    .flex()
                    .justify_center()
                    .items_center()
                    .gap(space::S1_5)
                    .text_ui(TypeSize::Small)
                    .child(Icon::new(IconName::Clock).size(IconSize::Nav).color(c.icon))
                    .child(div().medium().text_color(c.text).child("Meeting"))
                    .child(div().text_color(c.text_muted).child(said.text.clone()))
                    .into_any_element(),
                (None, _) => div()
                    .flex()
                    .justify_center()
                    .items_center()
                    .gap(space::S1_5)
                    .text_ui(TypeSize::Caption)
                    .text_color(c.text_faint)
                    .child(Icon::new(IconName::Info).size(IconSize::Nav).color(c.text_faint))
                    .child(said.text.clone())
                    .into_any_element(),
                (Some(bot), _) => self.bot_message(ix, bot, said, drawn, view, toggle, cx),
            },
        };
        entries::frame(ix, row.key(), body, matches!(row, Row::Talking))
    }

    /// A bot's message: who wrote it (face, name, a wrap-up or a report said so, when), then its
    /// steps and words under its name, and a wrap-up's parts.
    #[allow(clippy::too_many_arguments)]
    fn bot_message(&self, ix: usize, bot: &str, said: &GroupMessage, drawn: &Message, view: &Entity<GroupChatView>, toggle: impl Fn(Fold, &mut App) + Clone + 'static, cx: &App) -> AnyElement {
        let c = &cx.theme().colors;
        let author = self.author(bot);
        let (face, name, note, opens): (AnyElement, String, Option<&str>, Option<(String, String)>) = match &author {
            Author::Member { id, name, look, session } => (face(id, look.as_ref(), px(24.), cx), name.clone(), None, session.clone().map(|session| (id.clone(), session))),
            Author::Left { name, look } => (bot_face(bot, look, px(24.)), name.clone(), Some("Left the chat"), None),
            Author::Gone => (blank_face(px(24.), cx), "A bot that left".into(), None, None),
        };
        let kind = match (said.kind.as_str(), &said.report) {
            ("wrapUp", _) => Some((IconName::Queue, "Wrap-Up")),
            (_, Some(_)) => Some((IconName::Check, "Report")),
            _ => None,
        };
        let now = crate::time::now_ms();
        let selector = format!("author-{}", said.id);
        let name_selector = format!("author-{}-{}", said.id, name);
        let mut head = div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .flex()
            .items_center()
            .gap(space::S2)
            .h(px(24.))
            .child(face)
            .child(div().debug_selector(move || name_selector).flex_none().text_ui(TypeSize::Small).medium().text_color(if matches!(author, Author::Gone) { c.text_muted } else { c.text }).child(name.clone()))
            .children(note.map(|note| div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_faint).child(note)))
            .children(kind.map(|(icon, label)| {
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(space::S1)
                    .px(space::S1_5)
                    .h(kuvo_ui::theme::scaled(px(18.)))
                    .rounded(radius::PILL)
                    .bg(c.sunken)
                    .text_ui(TypeSize::Caption)
                    .text_color(c.text_muted)
                    .child(Icon::new(icon).size(IconSize::Nav).color(c.icon))
                    .child(label)
            }))
            .child(div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_faint).child(sent_at(said.at_ms(), self.now.unwrap_or(now))));
        // Its face and name open its own conversation, where the box can open it from here.
        if let Some((id, session)) = opens {
            let view = view.clone();
            head = head.cursor_pointer().on_click(move |_, _, cx| view.update(cx, |_, cx| cx.emit(ChatEvent::BotChat { bot: id.clone(), session: session.clone() })));
        }
        let body = entries::message_body(ix, &said.id, drawn, &self.open, toggle, &self.selection, &self.looks(), cx);
        div()
            .flex()
            .flex_col()
            .gap(space::S1_5)
            .child(head)
            .child(div().pl(px(INDENT)).child(body))
            .when(!said.tasks.is_empty(), |d| d.child(div().pl(px(INDENT)).pt(space::S1).child(self.tasks_card(said, view, cx))))
            .into_any_element()
    }

    /// A wrap-up's parts: each bot, its part and how it's going; Hand Out the Work while any waits
    /// for you.
    fn tasks_card(&self, said: &GroupMessage, view: &Entity<GroupChatView>, cx: &App) -> AnyElement {
        let c = &cx.theme().colors;
        let row = |task: &GroupTask, last: bool| {
            let (look, name, session) = match self.author(&task.bot) {
                Author::Member { name, look, session, .. } => (look, name, session),
                Author::Left { name, look } => (Some(look), name, None),
                Author::Gone => (None, "A bot that left".into(), None),
            };
            let after: Vec<String> = task.after.iter().filter_map(|bot| self.name_of(bot)).collect();
            let stopped = task.state == "working" && self.chat.stopped.contains(&task.bot);
            let state = task.title.clone();
            let tone = match task.state.as_str() {
                "done" => c.success,
                "failed" => c.danger,
                _ if stopped => c.warning,
                _ => c.text_muted,
            };
            let selector = format!("task-{}", task.id);
            div()
                .debug_selector(move || selector)
                .flex()
                .items_start()
                .gap(space::S3)
                .px(space::S4)
                .py(px(10.))
                .when(!last, |d| d.border_b_1().border_color(c.border))
                .child(div().pt(px(1.)).child(face(&task.bot, look.as_ref(), px(18.), cx)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(space::S0_5)
                        .child(div().text_ui(TypeSize::Small).text_color(c.text).child(div().medium().child(format!("{name}  "))).child(task.task.clone()))
                        .when(!after.is_empty(), |d| d.child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(format!("After {}", joined(&after))))),
                )
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(space::S1)
                        .text_ui(TypeSize::Caption)
                        .text_color(tone)
                        .when(task.state == "working" && !stopped, |d| d.child(spinner(SharedString::from(format!("task-spin-{}", task.id)), IconSize::Nav, c.text_muted)))
                        .child(state)
                        // Its conversation resumed on this box (one on another computer can't be from here yet).
                        .when_some(session.filter(|_| stopped), |d, session| {
                            let view = view.clone();
                            let selector = format!("resume-{}", task.id);
                            d.child(div().debug_selector(move || selector).pl(space::S1).child(Button::new(SharedString::from(format!("resume-{}", task.id)), "Resume").size(ButtonSize::Small).disabled(self.client.gate().is_held()).on_click(move |_, _, cx| {
                                view.update(cx, |this, cx| this.resume(session.clone(), cx));
                            })))
                        }),
                )
        };
        let count = said.tasks.len();
        let proposed = said.tasks.iter().any(|task| task.state == "proposed");
        let message = said.id.clone();
        let view = view.clone();
        div()
            .debug_selector(|| "tasks".into())
            .flex()
            .flex_col()
            .rounded(radius::CARD)
            .border_1()
            .border_color(c.border)
            .bg(c.surface)
            .overflow_hidden()
            .children(said.tasks.iter().enumerate().map(|(n, task)| row(task, n + 1 == count && !proposed)))
            .when(proposed, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S3)
                        .px(space::S4)
                        .py(space::S2)
                        .child(div().flex_1().text_ui(TypeSize::Caption).text_color(c.text_muted).child("Each bot does its part in its own conversation, and reports back here."))
                        .child(div().debug_selector(|| "start-work".into()).child(Button::new("start-work", "Hand Out the Work").size(ButtonSize::Small).primary().disabled(self.client.gate().is_held()).on_click(move |_, _, cx| {
                            view.update(cx, |this, cx| this.start_work(message.clone(), cx));
                        }))),
                )
            })
            .into_any_element()
    }

    /// Who's talking now, and what they're doing (its last step), so a long turn doesn't look stuck.
    fn talking(&self, cx: &App) -> AnyElement {
        let c = &cx.theme().colors;
        let Some(bot) = self.chat.speaking.as_deref() else { return div().into_any_element() };
        let (look, name) = match self.author(bot) {
            Author::Member { name, look, .. } => (look, name),
            Author::Left { name, look } => (Some(look), name),
            Author::Gone => (None, "A bot".into()),
        };
        let lines: Vec<String> = self
            .chat
            .activity
            .iter()
            .filter_map(|activity| match activity {
                Activity::Tool(call) => {
                    let name = call.get("name").and_then(|name| name.as_str()).unwrap_or("");
                    let state = call.get("state").cloned().unwrap_or_default();
                    let said = |field: &str| call.get(field).and_then(|said| said.as_str());
                    let (_, label, target) = entries::call_label(name, said("title"), said("doing"), &state, true);
                    Some(match target {
                        Some(target) => format!("{label} {target}"),
                        None => label.to_string(),
                    })
                }
                Activity::Thinking(text) if !text.trim().is_empty() => Some(text.trim().to_owned()),
                Activity::Writing => Some("Writing its reply".into()),
                _ => None,
            })
            .collect();
        div()
            .debug_selector(|| "talking".into())
            .flex()
            .flex_col()
            .gap(space::S1)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .child(face(bot, look.as_ref(), px(24.), cx))
                    .child(div().debug_selector(|| "talking-name".into()).text_ui(TypeSize::Small).text_color(c.text_muted).child(if matches!(self.chat.activity.last(), Some(Activity::Writing)) { format!("{name} is writing…") } else { format!("{name} is working…") }))
                    .child(spinner("talking-spin", IconSize::Nav, c.text_faint)),
            )
            .children(lines.into_iter().map(|line| div().pl(px(INDENT)).truncate().text_ui(TypeSize::Caption).text_color(c.text_faint).child(line)))
            .into_any_element()
    }

    /// Before anything's been said: who's here, and how it goes.
    fn intro(&self, cx: &App) -> AnyElement {
        let c = &cx.theme().colors;
        let Some(group) = &self.group else { return centred("Nothing’s been said here yet.".into(), false, cx) };
        let names: Vec<String> = group.members.iter().map(|member| member.name.clone()).collect();
        let line = group.purpose.clone().filter(|purpose| !purpose.trim().is_empty()).unwrap_or_else(|| joined(&names));
        div()
            .debug_selector(|| "group-intro".into())
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(space::S3)
            .px(space::S8)
            .pb(px(80.))
            .child(faces(group, px(44.), cx))
            .child(div().pt(space::S2).text_ui(TypeSize::Title).medium().text_color(c.text).child(group.name.clone()))
            .child(div().max_w(px(420.)).text_center().text_ui(TypeSize::Body).text_color(c.text_muted).child(line))
            .child(
                div()
                    .max_w(px(380.))
                    .pt(space::S2)
                    .text_center()
                    .text_ui(TypeSize::Small)
                    .text_color(c.text_faint)
                    .child("Bring them something to talk through, and they’ll weigh in and answer each other. Write & and a name to ask one of them directly."),
            )
            .into_any_element()
    }

    /// Over the composer, once a discussion paused after its turns: carry on, or leave it.
    fn paused_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.chat.paused || self.chat.discussing {
            return None;
        }
        let c = cx.theme().colors.clone();
        Some(
            div()
                .debug_selector(|| "paused-bar".into())
                .w_full()
                .max_w(px(COLUMN))
                .mb(space::S2)
                .flex()
                .items_center()
                .gap(space::S2)
                .px(space::S3)
                .py(space::S2)
                .rounded(radius::CARD)
                .bg(c.sunken)
                .border_1()
                .border_color(c.border)
                .child(Icon::new(IconName::Clock).size(IconSize::Nav).color(c.icon))
                .child(div().flex_1().min_w_0().text_ui(TypeSize::Small).text_color(c.text_muted).child("Paused after a good few turns, so it doesn’t run on."))
                .child(div().debug_selector(|| "keep-going".into()).child(Button::new("keep-going", "Keep Going").size(ButtonSize::Small).disabled(self.client.gate().is_held()).on_click(cx.listener(|this, _, _, cx| this.keep_going(cx)))))
                .into_any_element(),
        )
    }
}

/// A chat's bots' faces side by side, overlapping a little (a few, then how many more).
pub fn faces(group: &Group, size: gpui::Pixels, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    const SHOWN: usize = 4;
    let more = group.members.len().saturating_sub(SHOWN);
    div()
        .flex()
        .flex_none()
        .items_center()
        .children(group.members.iter().take(SHOWN).enumerate().map(|(n, member)| {
            div().when(n > 0, |d| d.ml(-(size * 0.28))).p(px(2.)).rounded(radius::PILL).bg(c.surface).child(face(&member.id, member.look.as_ref(), size, cx))
        }))
        .when(more > 0, |d| d.child(div().pl(space::S1).text_ui(TypeSize::Caption).text_color(c.text_faint).child(format!("+{more}"))))
        .into_any_element()
}

impl GroupChatView {
    /// When the times are read (shots and tests set it, so they read the same).
    pub fn set_now(&mut self, now: f64) {
        self.now = Some(now);
    }
}

impl Render for GroupChatView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.chat_references.refresh(self.rows.iter().rev().flat_map(Row::reference_text).map(std::borrow::Cow::Borrowed), &self.id, &self.client, self.live, self.list.clone(), cx);
        let c = cx.theme().colors.clone();
        let view = cx.entity();
        let body: AnyElement = if !self.read {
            match &self.problem {
                Some(why) => centred(format!("This chat couldn’t be read: {why}").into(), false, cx),
                None => centred("Loading the chat…".into(), true, cx),
            }
        } else if self.rows.is_empty() {
            self.intro(cx)
        } else {
            let list_view = view.clone();
            let list = chat_list(&self.list, move |ix, _window, cx| {
                let cx: &App = cx;
                list_view.read(cx).row(ix, &list_view, cx)
            });
            let scroller = self.list.clone();
            let area = SelectionArea::new(self.selection.clone(), self.transcript.clone(), list).on_scroll(move |by, _, _| scroller.scroll_by(by));
            let menu_view = view.clone();
            div()
                .key_context(selection::CONTEXT)
                .track_focus(&self.transcript)
                .on_action(cx.listener(Self::copy))
                .on_action(cx.listener(Self::select_all))
                .size_full()
                .child(ContextMenuArea::new("group-transcript-menu", area, move |window, cx| Self::transcript_menu(&menu_view, window, cx)).fill())
                .into_any_element()
        };
        // A problem while it's shown: said above the composer; it's tried again meanwhile.
        let banner = self.problem.clone().filter(|_| self.read).or_else(|| self.act_failed.clone());
        // (Words in the list's place sit centred above the composer.)
        let (listed, inset) = (self.read && !self.rows.is_empty(), self.list.bottom_inset());
        div()
            .debug_selector(|| "group-chat".into())
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            // The transcript fills the height and scrolls under the composer, which floats over
            // its bottom; the list's end is padded by the composer's height, as it's measured.
            .child(div().relative().flex_1().min_h_0().child(div().size_full().when(!listed, |d| d.pb(inset)).child(body)).child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .px(space::S6)
                    .pb(space::S5)
                    .child(self.list.floating())
                    .child(
                        div()
                            .id("group-composer-stack")
                            .block_mouse_except_scroll()
                            .w_full()
                            .max_w(px(COLUMN))
                            .flex()
                            .flex_col()
                            .items_center()
                            .children(banner.map(|why| div().w_full().max_w(px(COLUMN)).pb(space::S2).child(kuvo_ui::Notice::said("group-problem", kuvo_ui::Tone::Warning, why))))
                            .children(self.paused_bar(cx))
                            .child(div().w_full().max_w(px(COLUMN)).child(self.composer.clone())),
                    ),
            ))
    }
}

/// Centred words in the chat's place (loading, or why it can't be read).
fn centred(words: SharedString, busy: bool, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .gap(space::S2)
        .text_ui(TypeSize::Small)
        .text_color(c.text_muted)
        .children(busy.then(|| spinner("group-loading", IconSize::Nav, c.text_muted)))
        .child(words)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_bots_message_is_drawn_as_an_agents_its_steps_then_its_words_and_yours_as_yours() {
        let said: GroupMessage = serde_json::from_value(json!({ "id": "m1", "author": "bot_a", "text": "Done.", "time": 10.0, "kind": "message", "steps": [{ "type": "tool", "id": "t1", "name": "read", "state": { "status": "completed", "input": {} } }] })).unwrap();
        let Message::Assistant(step) = drawn(&said) else { panic!("an agent's step") };
        assert!(matches!(step.content.as_slice(), [Content::Tool { name, .. }, Content::Text(text)] if name == "read" && text == "Done."));
        assert_eq!(step.time.created, 10_000.0, "the box's seconds, as the app's ms");
        let yours: GroupMessage = serde_json::from_value(json!({ "id": "m2", "text": "Go", "time": 1.0, "kind": "message" })).unwrap();
        assert!(matches!(drawn(&yours), Message::User(user) if user.text == "Go"));
        let note: GroupMessage = serde_json::from_value(json!({ "id": "m3", "text": "Quill left the chat.", "time": 1.0, "kind": "note" })).unwrap();
        assert!(matches!(drawn(&note), Message::System(_)));
    }

    #[test]
    fn names_are_joined_as_a_sentence() {
        let names = |names: &[&str]| joined(&names.iter().map(|name| (*name).to_owned()).collect::<Vec<_>>());
        assert_eq!(names(&["Mira"]), "Mira");
        assert_eq!(names(&["Mira", "Bo"]), "Mira and Bo");
        assert_eq!(names(&["Mira", "Bo", "Cy"]), "Mira, Bo and Cy");
    }
}
