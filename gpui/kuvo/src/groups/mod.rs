//! Group chats, in the Bots area: a chat's conversation (`chat`), under a strip with its bots and
//! Edit; New Group; and a chat's editor (its name, what it's for, its bots, Delete asked first).
//! Everything is the box's: the chats and their bots (`kuvo/groups`), who can be in one
//! (`kuvo/groups/joinable`), and every rule of what a chat may be, which it says when it turns
//! one down (`POST kuvo/groups`, `PATCH` and `DELETE kuvo/groups/<id>`). The app draws what it says.

pub mod chat;

use std::collections::HashMap;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Task, Window, div, prelude::FluentBuilder as _, px,
};
use kuvo_box::BoxClient;
use kuvo_box::areas::Bot;
use kuvo_box::groups::{Group, GroupChange, GroupMember, NewGroup};
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, Icon, IconName, StyledType, TextField, TypeSize,
    small::Checkbox,
    theme::{radius, space},
};

use crate::areas::{Reading, reading};
use chat::{ChatEvent, GroupChatView, face, faces, joined};

/// The widest the editor's cards grow.
const FORM_WIDTH: f32 = 600.;

/// What the view shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    Nothing,
    /// A chat's conversation, by its id.
    Chat(String),
    /// A chat's editor.
    Edit(String),
    New,
}

/// What the view asks of the window around it.
#[derive(Clone, Debug, PartialEq)]
pub enum GroupsEvent {
    /// Open a bot's own conversation.
    BotChat { bot: String, session: String },
    /// Pick this row in the sidebar (none: nothing).
    Select(Option<String>),
    /// The chats changed: the list is to be read again.
    Changed,
}

/// What a control asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Edit,
    /// A bot in or out of the chat being made or changed.
    Toggle(String),
    Save,
    Cancel,
    Delete,
    ConfirmDelete,
    CancelDelete,
}

/// The editor's form: what's picked, and how its change stands.
#[derive(Clone, Debug, Default, PartialEq)]
struct Form {
    /// Its bots, in the order they were picked (the chat's own order first).
    picked: Vec<String>,
    saving: bool,
    /// Why the box turned the last one down, in its words.
    error: Option<String>,
    asking_delete: bool,
}

pub struct GroupsView {
    client: BoxClient,
    moves: Option<Entity<crate::moves::MovesView>>,
    this_box: String,
    live: bool,
    shown: Shown,
    /// The chats as the box lists them, and its bots (who wrote a message in a chat they've left).
    groups: Vec<Group>,
    bots: Vec<Bot>,
    chats: HashMap<String, (Entity<GroupChatView>, Subscription)>,
    joinable: Reading<Vec<GroupMember>>,
    name: Entity<TextField>,
    purpose: Entity<TextField>,
    form: Form,
    /// Whether the box's list has been read (a chat it doesn't list is gone then), and whether
    /// this view is on screen (Bots shown, no bot's conversation over it).
    listed: bool,
    on_screen: bool,
    now: Option<f64>,
    focus: FocusHandle,
    tasks: Vec<Task<()>>,
}

impl EventEmitter<GroupsEvent> for GroupsView {}

impl Focusable for GroupsView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.shown {
            Shown::Chat(id) => self.chats.get(id).map(|(chat, _)| chat.focus_handle(cx)).unwrap_or_else(|| self.focus.clone()),
            Shown::New | Shown::Edit(_) => self.name.focus_handle(cx),
            Shown::Nothing => self.focus.clone(),
        }
    }
}

impl GroupsView {
    pub fn set_this_box(&mut self, name: &str, cx: &mut Context<Self>) {
        self.this_box = name.to_owned();
        cx.notify();
    }

    pub fn set_moves(&mut self, moves: Entity<crate::moves::MovesView>, cx: &mut Context<Self>) {
        self.moves = Some(moves);
        cx.notify();
    }

    pub fn new(client: BoxClient, live: bool, cx: &mut Context<Self>) -> GroupsView {
        let name = cx.new(|cx| TextField::new(cx).placeholder("Name"));
        let purpose = cx.new(|cx| TextField::new(cx).placeholder("Optional"));
        GroupsView { client, moves: None, this_box: "This Mac".into(), live, shown: Shown::Nothing, groups: Vec::new(), bots: Vec::new(), chats: HashMap::new(), joinable: Reading::NotYet, name, purpose, form: Form::default(), listed: false, on_screen: false, now: None, focus: cx.focus_handle(), tasks: Vec::new() }
    }

    pub fn shown(&self) -> &Shown {
        &self.shown
    }

    /// The conversation of chat `id`, once it's been opened.
    pub fn chat(&self, id: &str) -> Option<&Entity<GroupChatView>> {
        self.chats.get(id).map(|(chat, _)| chat)
    }

    pub fn joinable(&self) -> &Reading<Vec<GroupMember>> {
        &self.joinable
    }

    pub fn picked(&self) -> &[String] {
        &self.form.picked
    }

    pub fn error(&self) -> Option<&str> {
        self.form.error.as_deref()
    }

    pub fn name_field(&self) -> &Entity<TextField> {
        &self.name
    }

    pub fn purpose_field(&self) -> &Entity<TextField> {
        &self.purpose
    }

    pub fn set_now(&mut self, now: f64, cx: &mut Context<Self>) {
        self.now = Some(now);
        for (chat, _) in self.chats.values() {
            chat.update(cx, |chat, _| chat.set_now(now));
        }
    }

    /// The chats and bots as the box lists them now.
    pub fn set_listed(&mut self, groups: Vec<Group>, bots: Vec<Bot>, cx: &mut Context<Self>) {
        // A chat no longer listed (deleted elsewhere, moved away) is let go: it's followed no more.
        let gone: Vec<String> = self.chats.keys().filter(|id| !groups.iter().any(|group| &group.id == *id)).cloned().collect();
        for id in gone {
            if let Some((chat, _)) = self.chats.remove(&id) {
                chat.update(cx, |chat, cx| chat.set_shown(false, cx));
            }
        }
        for (id, (chat, _)) in &self.chats {
            let group = groups.iter().find(|group| &group.id == id).cloned();
            let bots = bots.clone();
            chat.update(cx, |chat, cx| chat.set_listed(group, bots, cx));
        }
        self.groups = groups;
        self.bots = bots;
        self.listed = true;
        // The chat shown, listed again (back from a move, or listed later than it was made): its
        // conversation, as it would open.
        if let Shown::Chat(id) = self.shown.clone()
            && !self.gone(&id)
            && !self.chats.contains_key(&id)
        {
            let chat = self.open_chat(&id, cx);
            let on = self.on_screen;
            chat.update(cx, |chat, cx| chat.set_shown(on, cx));
        }
        cx.notify();
    }

    /// Whether it's on screen: only then is the chat shown followed, and marked read.
    pub fn set_on_screen(&mut self, on: bool, cx: &mut Context<Self>) {
        self.on_screen = on;
        if let Shown::Chat(id) = &self.shown
            && let Some((chat, _)) = self.chats.get(id)
        {
            chat.update(cx, |chat, cx| chat.set_shown(on, cx));
        }
    }

    /// Whether `id` is known to be gone: the box's list was read, and doesn't have it.
    fn gone(&self, id: &str) -> bool {
        self.listed && self.group(id).is_none()
    }

    fn group(&self, id: &str) -> Option<&Group> {
        self.groups.iter().find(|group| group.id == id)
    }

    /// Shows what the sidebar's key picks: `group:<id>`, `edit-group:<id>`, `new-group`; anything
    /// else, nothing.
    pub fn show(&mut self, key: Option<&str>, cx: &mut Context<Self>) {
        let shown = match key {
            Some("new-group") => Shown::New,
            Some(key) if key.starts_with("group:") => Shown::Chat(key["group:".len()..].to_owned()),
            Some(key) if key.starts_with("edit-group:") => Shown::Edit(key["edit-group:".len()..].to_owned()),
            _ => Shown::Nothing,
        };
        // What's left goes off screen: its chat isn't marked read meanwhile.
        if let Shown::Chat(id) = &self.shown
            && self.shown != shown
            && let Some((chat, _)) = self.chats.get(id)
        {
            chat.update(cx, |chat, cx| chat.set_shown(false, cx));
        }
        let fresh = self.shown != shown;
        self.shown = shown.clone();
        match shown {
            Shown::Chat(id) if !self.gone(&id) => {
                let chat = self.open_chat(&id, cx);
                let on = self.on_screen;
                chat.update(cx, |chat, cx| chat.set_shown(on, cx));
            }
            Shown::New if fresh => {
                self.fill(None, cx);
                self.read_joinable(cx);
            }
            Shown::Edit(id) if fresh => {
                let group = self.group(&id).cloned();
                self.fill(group.as_ref(), cx);
                self.read_joinable(cx);
            }
            _ => {}
        }
        cx.notify();
    }

    /// It's shown again (the area came back): who can be in a chat is read again.
    pub fn read(&mut self, cx: &mut Context<Self>) {
        if matches!(self.shown, Shown::New | Shown::Edit(_)) {
            self.read_joinable(cx);
        }
    }

    fn open_chat(&mut self, id: &str, cx: &mut Context<Self>) -> Entity<GroupChatView> {
        if let Some((chat, _)) = self.chats.get(id) {
            return chat.clone();
        }
        let group = self.group(id).cloned();
        let (client, live, bots, now) = (self.client.clone(), self.live, self.bots.clone(), self.now);
        let chat = cx.new(|cx| {
            let mut chat = GroupChatView::new(client, id.to_owned(), group.clone(), live, cx);
            if let Some(now) = now {
                chat.set_now(now);
            }
            chat.set_listed(group, bots, cx);
            chat
        });
        let subscription = cx.subscribe(&chat, |_, _, event: &ChatEvent, cx| match event {
            ChatEvent::BotChat { bot, session } => cx.emit(GroupsEvent::BotChat { bot: bot.clone(), session: session.clone() }),
            ChatEvent::Changed => cx.emit(GroupsEvent::Changed),
        });
        self.chats.insert(id.to_owned(), (chat.clone(), subscription));
        chat
    }

    /// The form as `group` is (New Group: empty).
    fn fill(&mut self, group: Option<&Group>, cx: &mut Context<Self>) {
        let (name, purpose) = group.map(|group| (group.name.clone(), group.purpose.clone().unwrap_or_default())).unwrap_or_default();
        self.name.update(cx, |field, cx| field.set_text(name, cx));
        self.purpose.update(cx, |field, cx| field.set_text(purpose, cx));
        self.form = Form { picked: group.map(|group| group.members.iter().map(|member| member.id.clone()).collect()).unwrap_or_default(), ..Form::default() };
    }

    fn read_joinable(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        if !self.live {
            let read = client.joinable();
            self.joinable = reading(read, &self.joinable);
            return;
        }
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(cx.spawn(async move |this, cx| {
            let read = cx.background_executor().spawn(async move { client.joinable() }).await;
            this.update(cx, |this, cx| {
                this.joinable = reading(read, &this.joinable);
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn act(&mut self, act: Act, cx: &mut Context<Self>) {
        match act {
            Act::Edit => {
                if let Shown::Chat(id) = &self.shown {
                    cx.emit(GroupsEvent::Select(Some(format!("edit-group:{id}"))));
                }
            }
            Act::Toggle(bot) => {
                if self.form.saving {
                    return;
                }
                if let Some(at) = self.form.picked.iter().position(|each| each == &bot) {
                    self.form.picked.remove(at);
                } else {
                    self.form.picked.push(bot);
                }
                self.form.error = None;
            }
            Act::Cancel => match &self.shown {
                Shown::Edit(id) => cx.emit(GroupsEvent::Select(Some(format!("group:{id}")))),
                _ => cx.emit(GroupsEvent::Select(None)),
            },
            Act::Save => self.save(cx),
            Act::Delete => self.form.asking_delete = true,
            Act::CancelDelete => self.form.asking_delete = false,
            Act::ConfirmDelete => self.delete(cx),
        }
        cx.notify();
    }

    /// Makes the chat, or sends what changed of it; the box's answer is what's shown after.
    fn save(&mut self, cx: &mut Context<Self>) {
        if self.form.saving || self.client.gate().is_held() {
            return;
        }
        let name = self.name.read(cx).text().trim().to_owned();
        let purpose = self.purpose.read(cx).text().trim().to_owned();
        let members = self.form.picked.clone();
        let client = self.client.clone();
        let shown = self.shown.clone();
        let work: Box<dyn FnOnce() -> kuvo_box::Result<Option<Group>> + Send> = match &shown {
            Shown::Edit(id) => {
                let Some(group) = self.group(id) else { return };
                let before: Vec<String> = group.members.iter().map(|member| member.id.clone()).collect();
                // The same bots in another order (one taken out and put back) is no change.
                let sorted = |ids: &[String]| {
                    let mut ids = ids.to_vec();
                    ids.sort();
                    ids
                };
                let change = GroupChange {
                    name: (name != group.name).then_some(name),
                    purpose: (purpose != group.purpose.clone().unwrap_or_default()).then_some(purpose),
                    members: (sorted(&members) != sorted(&before)).then_some(members),
                };
                // Nothing changed: back to the chat, as it is.
                if change.is_empty() {
                    cx.emit(GroupsEvent::Select(Some(format!("group:{id}"))));
                    return;
                }
                let id = id.clone();
                Box::new(move || client.change_group(&id, &change))
            }
            Shown::New => {
                let new = NewGroup { name, members, purpose };
                Box::new(move || client.make_group(&new))
            }
            _ => return,
        };
        self.form.saving = true;
        self.form.error = None;
        if !self.live {
            let result = work();
            self.saved(&shown, result, cx);
            return;
        }
        let task = cx.background_executor().spawn(async move { work() });
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| this.saved(&shown, result, cx)).ok();
        }));
    }

    fn saved(&mut self, from: &Shown, result: kuvo_box::Result<Option<Group>>, cx: &mut Context<Self>) {
        self.form.saving = false;
        // (Gone elsewhere meanwhile: the window doesn't move.)
        let here = &self.shown == from;
        match result {
            Ok(group) => {
                if let Some(group) = &group {
                    match self.groups.iter_mut().find(|each| each.id == group.id) {
                        Some(listed) => *listed = group.clone(),
                        None => self.groups.push(group.clone()),
                    }
                }
                cx.emit(GroupsEvent::Changed);
                if here {
                    // As the box has it now; one it no longer lists here (moving away) isn't opened.
                    cx.emit(GroupsEvent::Select(group.map(|group| format!("group:{}", group.id))));
                }
            }
            Err(error) => {
                if here {
                    self.form.error = Some(kuvo_box::accounts::said(&error));
                }
            }
        }
        cx.notify();
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        let Shown::Edit(id) = self.shown.clone() else { return };
        if self.form.saving || self.client.gate().is_held() {
            return;
        }
        self.form.saving = true;
        self.form.error = None;
        let client = self.client.clone();
        let from = self.shown.clone();
        let done = move |this: &mut GroupsView, result: kuvo_box::Result<()>, cx: &mut Context<GroupsView>| {
            this.form.saving = false;
            match result {
                Ok(()) => {
                    this.groups.retain(|group| group.id != id);
                    if let Some((chat, _)) = this.chats.remove(&id) {
                        chat.update(cx, |chat, cx| chat.set_shown(false, cx));
                    }
                    this.form.asking_delete = false;
                    cx.emit(GroupsEvent::Changed);
                    if this.shown == from {
                        cx.emit(GroupsEvent::Select(None));
                    }
                }
                Err(error) => this.form.error = Some(kuvo_box::accounts::said(&error)),
            }
            cx.notify();
        };
        let target = match &self.shown {
            Shown::Edit(id) => id.clone(),
            _ => return,
        };
        if !self.live {
            let result = client.delete_group(&target);
            done(self, result, cx);
            return;
        }
        let task = cx.background_executor().spawn(async move { client.delete_group(&target) });
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| done(this, result, cx)).ok();
        }));
    }

    /// The bots the form offers: those the box says can be in a chat made here, then any of the
    /// chat's own it doesn't list now (so taking one out is still yours to do).
    fn offered(&self) -> Vec<GroupMember> {
        let mut offered = match &self.joinable {
            Reading::Ready(bots) => bots.clone(),
            _ => Vec::new(),
        };
        if let Shown::Edit(id) = &self.shown
            && let Some(group) = self.group(id)
        {
            for member in &group.members {
                if !offered.iter().any(|each| each.id == member.id) {
                    offered.push(member.clone());
                }
            }
        }
        offered
    }
}

impl GroupsView {
    /// Over a chat: its bots, and Edit.
    fn strip(&self, group: Option<&Group>, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let names: Vec<String> = group.map(|group| group.members.iter().map(|member| member.name.clone()).collect()).unwrap_or_default();
        div()
            .debug_selector(|| "group-strip".into())
            .flex()
            .flex_none()
            .items_center()
            .gap(space::S2)
            .h(px(44.))
            .px(space::S4)
            .border_b_1()
            .border_color(c.border)
            .children(group.map(|group| faces(group, px(20.), cx)))
            .child(div().debug_selector(|| "group-members".into()).flex_1().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text_muted).child(joined(&names)))
            .child(
                div().debug_selector(|| "group-edit".into()).child(
                    Button::new("group-edit", "Edit")
                        .icon(IconName::Pencil)
                        .ghost()
                        .size(ButtonSize::Small)
                        .tooltip("Edit Group")
                        .disabled(group.is_none() || self.client.gate().is_held())
                        .on_click(cx.listener(|this, _, _, cx| this.act(Act::Edit, cx))),
                ),
            )
            .into_any_element()
    }

    /// New Group or a chat's editor: its name, what it's for, its bots; Delete asked first.
    fn form_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let editing = matches!(self.shown, Shown::Edit(_));
        let title = if editing { "Edit Group" } else { "New Group" };
        let field_row = |key: &'static str, title: &'static str, detail: String, control: AnyElement, last: bool| {
            div()
                .debug_selector(move || key.into())
                .flex()
                .items_center()
                .gap(space::S3)
                .px(space::S5)
                .py(space::S3)
                .when(!last, |d| d.border_b_1().border_color(c.border))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(space::S0_5)
                        .child(div().text_ui(TypeSize::Body).medium().text_color(c.text).child(title))
                        .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(detail)),
                )
                .child(div().flex_none().child(control))
        };
        let card = |rows: Vec<AnyElement>| div().flex().flex_col().rounded(radius::POPOVER).bg(c.card).shadow(cx.theme().card_shadow()).overflow_hidden().children(rows);
        let computer = match (&self.shown, &self.moves) {
            (Shown::Edit(id), Some(moves)) if self.group(id).is_some() => {
                let target = kuvo_box::moves::Target::new(kuvo_box::moves::Kind::Group, id);
                Some(field_row("group-computer", "Computer", format!("Lives on {}", self.this_box),
                    crate::moves::MovesView::settings_button(moves, "group-computer", target, None, cx), true).into_any_element())
            }
            _ => None,
        };
        let field = |field: Entity<TextField>| div().w(px(280.)).child(field).into_any_element();
        let mut rows = vec![
            field_row("group-name", "Name", "What the chat is called.".into(), field(self.name.clone()), false).into_any_element(),
            field_row("group-purpose", "What it’s for", "The bots read it as they talk.".into(), field(self.purpose.clone()), computer.is_none()).into_any_element(),
        ];
        rows.extend(computer);
        let about = card(rows);
        let offered = self.offered();
        let bots: AnyElement = match (&self.joinable, offered.is_empty()) {
            (Reading::NotYet, true) => kuvo_ui::small::LoadingLabel::new("joinable-loading", "Loading the bots…").compact().into_any_element(),
            (Reading::Failed(why), true) => div().debug_selector(|| "joinable-failed".into()).p(space::S5).text_ui(TypeSize::Small).text_color(c.warning).child(format!("The bots couldn’t be read: {why}")).into_any_element(),
            (_, true) => div().debug_selector(|| "joinable-none".into()).p(space::S5).text_ui(TypeSize::Small).text_color(c.text_muted).child("There are no bots here to bring together yet. Make one with the + beside Bots.").into_any_element(),
            (_, false) => {
                let count = offered.len();
                card(
                    offered
                        .iter()
                        .enumerate()
                        .map(|(n, bot)| {
                            let picked = self.form.picked.contains(&bot.id);
                            let id = bot.id.clone();
                            let selector = format!("pick-{}", bot.id);
                            let line = bot.computer.as_ref().map(|computer| format!("On {computer}"));
                            div()
                                .id(SharedString::from(selector.clone()))
                                .debug_selector(move || selector)
                                .flex()
                                .items_center()
                                .gap(space::S3)
                                .px(space::S5)
                                .py(px(10.))
                                .when(n + 1 < count, |d| d.border_b_1().border_color(c.border))
                                .cursor_pointer()
                                .hover(|s| s.bg(c.hover))
                                .on_click(cx.listener(move |this, _, _, cx| this.act(Act::Toggle(id.clone()), cx)))
                                .child(face(&bot.id, bot.look.as_ref(), px(28.), cx))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .child(div().truncate().text_ui(TypeSize::Body).text_color(c.text).child(bot.name.clone()))
                                        .children(line.map(|line| div().truncate().text_ui(TypeSize::Small).text_color(c.text_muted).child(line))),
                                )
                                // (The row takes the click: the box is what it shows.)
                                .child(Checkbox::new(SharedString::from(format!("check-{}", bot.id)), picked))
                                .into_any_element()
                        })
                        .collect(),
                )
                .into_any_element()
            }
        };
        let saving = self.form.saving;
        let foot = div()
            .pt(space::S5)
            .flex()
            .items_center()
            .gap(space::S3)
            .child(div().flex_1().min_w_0().children(self.form.error.clone().map(|error| div().debug_selector(|| "group-error".into()).text_ui(TypeSize::Small).text_color(c.danger).child(error))))
            .child(div().debug_selector(|| "group-cancel".into()).child(Button::new("group-cancel", "Cancel").disabled(saving).on_click(cx.listener(|this, _, _, cx| this.act(Act::Cancel, cx)))))
            .child(
                div()
                    .debug_selector(|| "group-save".into())
                    .child(Button::new("group-save", if editing { "Save" } else { "Create Group" }).primary().loading(saving && !self.form.asking_delete).disabled(self.client.gate().is_held()).on_click(cx.listener(|this, _, _, cx| this.act(Act::Save, cx)))),
            );
        let delete = editing.then(|| {
            let name = match &self.shown {
                Shown::Edit(id) => self.group(id).map(|group| group.name.clone()).unwrap_or_default(),
                _ => String::new(),
            };
            let body: AnyElement = if self.form.asking_delete {
                div()
                    .debug_selector(|| "group-delete-ask".into())
                    .flex()
                    .items_center()
                    .gap(space::S3)
                    .px(space::S5)
                    .py(space::S4)
                    .child(Icon::new(IconName::Trash).color(c.danger))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(space::S0_5)
                            .child(div().text_ui(TypeSize::Body).medium().text_color(c.text).child(format!("Delete “{name}”?")))
                            .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child("Its messages are deleted, and its bots forget it. The bots themselves stay.")),
                    )
                    .child(Button::new("group-delete-cancel", "Cancel").size(ButtonSize::Small).disabled(saving).on_click(cx.listener(|this, _, _, cx| this.act(Act::CancelDelete, cx))))
                    .child(div().debug_selector(|| "group-delete-confirm".into()).child(Button::new("group-delete-confirm", "Delete").size(ButtonSize::Small).destructive().loading(saving).on_click(cx.listener(|this, _, _, cx| this.act(Act::ConfirmDelete, cx)))))
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .items_center()
                    .gap(space::S3)
                    .px(space::S5)
                    .py(space::S4)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(space::S0_5)
                            .child(div().text_ui(TypeSize::Body).medium().text_color(c.text).child("Delete Group"))
                            .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child("You’ll be asked before anything is deleted.")),
                    )
                    .child(div().debug_selector(|| "group-delete".into()).child(Button::new("group-delete", "Delete…").size(ButtonSize::Small).destructive().disabled(saving || self.client.gate().is_held()).on_click(cx.listener(|this, _, _, cx| this.act(Act::Delete, cx)))))
                    .into_any_element()
            };
            div().pt(space::S8).child(card(vec![body])).into_any_element()
        });
        let section = |label: &'static str| div().pt(space::S6).pb(space::S2).text_ui(TypeSize::Body).text_color(c.text_muted).child(label);
        div()
            .id("group-form")
            .debug_selector(|| "group-form".into())
            .size_full()
            .overflow_y_scroll()
            .flex()
            .justify_center()
            .px(space::S8)
            .child(
                div()
                    .w_full()
                    .max_w(px(FORM_WIDTH))
                    .pb(space::S10)
                    .child(div().pt(space::S8).pb(space::S4).text_ui(TypeSize::Title).medium().text_color(c.text).child(title))
                    .child(about)
                    .child(section("Bots"))
                    .child(bots)
                    .child(foot)
                    .children(delete),
            )
            .into_any_element()
    }
}

impl Render for GroupsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        let gone = match &self.shown {
            Shown::Chat(id) | Shown::Edit(id) => self.gone(id),
            _ => false,
        };
        let body: AnyElement = match self.shown.clone() {
            _ if gone => div()
                .debug_selector(|| "group-gone".into())
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_ui(TypeSize::Small)
                .text_color(c.text_muted)
                .child("This group chat isn’t here anymore.")
                .into_any_element(),
            Shown::Chat(id) => {
                let group = self.group(&id).cloned();
                let chat = self.chats.get(&id).map(|(chat, _)| chat.clone());
                div().size_full().flex().flex_col().child(self.strip(group.as_ref(), cx)).child(div().flex_1().min_h_0().children(chat)).into_any_element()
            }
            Shown::New | Shown::Edit(_) => self.form_view(cx),
            Shown::Nothing => div().size_full().into_any_element(),
        };
        div().track_focus(&self.focus).size_full().child(body)
    }
}
