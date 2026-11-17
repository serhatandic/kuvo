//! The composer: the text field, the model and effort chips, and Send (Stop while a run is under
//! way and nothing's written). What's written goes through `kuvo_box`'s typed send: a plain message
//! straight to the box as fields, "/" and "!" to the box first, which says what they mean. "/"
//! offers what the box says it offers there, as you type.
//!
//! In a new conversation (the landing view) Send starts it on the box, in the project picked, then
//! sends the same way.

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, ExternalPaths, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyBinding, ObjectFit, ParentElement, PathPromptOptions, Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    StyledImage as _, Subscription, Task, Window, actions, div, img, prelude::FluentBuilder as _, px,
};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use crate::model_picker::{Catalog, ModelPicker};

use kuvo_box::approval::{Approval, Here, Picked, Shown};
use kuvo_box::models::{ModelState, Offered, OfferedModel, Side, StartHow, StartIn, Starred, effort_title};
use kuvo_box::typed::{self, Answer, ModelRef, Offers};
use kuvo_box::groups::GroupMember;
use kuvo_box::mentions::{For, Mention};
use kuvo_box::image_names;
use kuvo_box::quoting::{self, Quote};
use kuvo_box::parts::{Delivery, File};
use kuvo_box::{BoxClient, Draft, Sent, Where};
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, Pasted, StyledType, TextField, TextFieldEvent, TypeSize,
    menu::{Menu, MenuEntry, PopoverMenu, PopoverMenuHandle},
    small::{Chip, ChipTone},
    theme::{radius, space},
};

actions!(composer, [SlashNext, SlashPrevious, SlashPick, SlashDismiss, SendQueued, ChooseModel]);

const CONTEXT: &str = "Composer";
const SLASH: &str = "SlashMenu";
/// How long before what the box couldn't say of its models is asked again, at first and at most.
const RETRY_FIRST: Duration = Duration::from_secs(1);
const RETRY_MOST: Duration = Duration::from_secs(30);
/// How long typing a mention pauses before the box is asked what it suggests.
const MENTION_PAUSE: Duration = Duration::from_millis(150);
/// How many of "/"'s offers show at once (the rest scroll).
const SLASH_MOST: usize = 8;

/// The keys "/"'s list takes while it shows, ahead of the field's own; ⌘Return, for after this run.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("cmd-enter", SendQueued, Some(CONTEXT)), KeyBinding::new("cmd-shift-m", ChooseModel, Some(CONTEXT))]);
    let context = format!("{SLASH} > {}", kuvo_ui::text_field::CONTEXT);
    let context = Some(context.as_str());
    cx.bind_keys([
        KeyBinding::new("down", SlashNext, context),
        KeyBinding::new("up", SlashPrevious, context),
        KeyBinding::new("tab", SlashPick, context),
        KeyBinding::new("enter", SlashPick, context),
        KeyBinding::new("escape", SlashDismiss, context),
    ]);
}

/// Where what's written goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A conversation on the box.
    Session { id: String, place: Where },
    /// A new one, started in a project (or as a chat) when it's sent.
    New(StartIn),
    /// A group chat on the box: only words go to it (`POST kuvo/groups/<id>`), and the box there
    /// names whoever they name.
    Group { id: String },
}

/// How a message goes while the agent works (0.7's choice on send): into the run under way, at the
/// agent's next step (Return, the default); after the run (⌘Return); or now, the step under way
/// stopped for it (Send Now).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendAs {
    Steer,
    Queue,
    Now,
}

/// The placeholder while the agent works (0.7's).
const WHILE_RUNNING: &str = "Add to the task…";

/// What the composer tells its owner.
#[derive(Clone, Debug, PartialEq)]
pub enum ComposerEvent {
    /// A new conversation was started on the box (in `folder`, filed under `project`), and what
    /// was written (`written`) and its attached `files` sent to it: what the box said of it, or
    /// why it couldn't go, is for the conversation's own composer.
    Started { session: String, folder: Option<String>, project: Option<String>, written: String, files: Vec<PathBuf>, sent: Result<Sent, String> },
    /// A group chat took what was written, or stopped: it's to be read again.
    Wrote,
    /// What's on its way from here changed (`outgoing`): the conversation draws it again.
    Outgoing,
}

/// A message sent from here, drawn in its conversation at once as your message (0.7's "Sending"
/// bubble) until its box's copy, made under the same id, takes its place; one that couldn't go
/// stays there, saying why, with Retry. This is the client's own record of what it sent
/// (PROTOCOL.md, "The queue": the in-flight record stays with the client that sends it).
#[derive(Clone, Debug, PartialEq)]
pub struct Outgoing {
    pub id: String,
    /// As written.
    pub words: String,
    pub files: Vec<PathBuf>,
    /// Why it couldn't go, once it couldn't.
    pub failed: Option<String>,
    /// Its box took it: it's the agent's to read, at its next step.
    pub taken: bool,
    /// The conversation has its copy, which came without its files: they show under it.
    pub arrived: bool,
    /// What goes again on Retry, under the same id (the box takes one id once).
    draft: Draft,
}

impl Outgoing {
    /// Sent for after the run: its box puts it in the queue, not the agent's next step.
    pub fn is_queued(&self) -> bool {
        self.draft.delivery == Delivery::Queue
    }
}

/// Your answer to a quote, under it: its own field, shown while it has words or once it's opened
/// (the quote added, or clicked), as 0.7's `QuoteList`.
struct QuoteAnswer {
    field: Entity<TextField>,
    open: bool,
    /// Whether it had the keyboard when last drawn: an empty one closes as it lets go of it.
    had_focus: bool,
    _events: Subscription,
}

/// What the chips show, as the box says it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Models {
    /// What its box says of the conversation's model.
    pub state: Option<ModelState>,
    /// What the box offers (`kuvo/models/offered`): each model with its side and efforts, and where
    /// a new conversation starts.
    pub offered: Option<Offered>,
}

/// A line under the composer: what the box did with a typed text, or what this app can't do yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub text: SharedString,
    pub warning: bool,
}

/// One of "/"'s offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub name: String,
    pub summary: String,
    /// What it is, for a mention: a file, a folder, a bot or a session.
    pub icon: Option<IconName>,
}

pub struct Composer {
    client: BoxClient,
    target: Target,
    field: Entity<TextField>,
    field_held: bool,
    /// Whether the conversation has a run under way (Send turns to Stop with nothing written).
    running: bool,
    sending: bool,
    /// The message on its way to the conversation's box, for Stop to take with it.
    on_its_way: Option<String>,
    /// Whether anything waits in the conversation's queue (as its box says): ⌘Return queues after
    /// it even with nothing running.
    has_queue: bool,
    /// The menu beside Send while the agent works.
    send_menu: PopoverMenuHandle,
    /// The model picker, open from its chip or ⇧⌘M.
    model_menu: PopoverMenuHandle,
    /// What the picker offers, worked out once for what the box last said.
    catalog: Option<Arc<Catalog>>,
    /// Messages sent from here that the conversation doesn't have yet, in the order sent.
    outgoing: Vec<Outgoing>,
    /// Whether it reads the box by itself (the app), or shows what it's handed (tests and shots).
    live: bool,
    models: Models,
    /// The model a new conversation starts on, as picked here (the box's `starting` until then).
    picked: Option<ModelRef>,
    /// The conversation's folder, where its project's own settings may add models (`set_folder`).
    folder: Option<String>,
    /// Which read of the models is the latest: an older one coming back late is left.
    reads: u64,
    /// How long before what the box couldn't say is asked again (longer each time).
    retry: Duration,
    retrying: Option<Task<()>>,
    /// Whether the box couldn't say all of it last time: asked again in a while, while it's shown.
    unsaid: bool,
    /// A model picked whose change hasn't come back yet: what's said of another meanwhile is left
    /// (as KuvoClient's `SessionModelState.isAbout`).
    picking: Option<ModelRef>,
    /// Whether it's known where the conversation runs (its engine, or its coding agents' bridge):
    /// its model can't be changed in place until then.
    place_known: bool,
    /// Whether it's on screen (a kept conversation's isn't): only then is it asked again.
    shown: bool,
    offers: Option<Offers>,
    slash_scroll: ScrollHandle,
    slash_at: usize,
    /// The text "/"'s list was put away for (with Escape): it shows again once that changes.
    slash_dismissed: Option<String>,
    note: Option<Note>,
    /// What the field held as the note was shown: the note goes once that changes.
    noted: String,
    /// Files and pictures that go with what's written, in the order they were added.
    attached: Vec<PathBuf>,
    /// An item's description (`Composer::item`) in place of a message: the same paste and drop of
    /// files, without a conversation's controls.
    item_editor: bool,
    /// An item's description takes files: only where the item's computer keeps them
    /// (`set_attaching`); with none offered, ⌘V pastes text and nothing can be dropped.
    item_attaching: bool,
    /// Which draft of attachments is the latest: a paste or copy finishing for an earlier one is left.
    attachment_generation: u64,
    /// Stable keys for attachments in an item description.
    item_keys: Vec<usize>,
    item_key: usize,
    /// What you're replying to (a message, or a part of one), ahead of your words as it's sent,
    /// each with your answer to it in a field of its own (`answers`, in the same order), as 0.7's.
    quotes: Vec<Quote>,
    answers: Vec<QuoteAnswer>,
    /// What "@" or "&" suggests for the mention being typed (the box's: `sessions.mentions`, or in a
    /// group chat `groups.mentions`), and which mention that was; which read of them is the latest.
    mentions: Vec<Mention>,
    mentions_for: Option<(For, String)>,
    mention_reads: u64,
    /// A mention's suggestions asked for and not answered yet.
    mention_pending: bool,
    /// The agents and sessions picked from "&"'s list, by what went in the message: the send names
    /// those still in it.
    named: Vec<(String, String)>,
    /// A group chat's bots, as its box lists them: "&" offers them, and each face asks one.
    members: Vec<GroupMember>,
    /// What the empty field says.
    placeholder: SharedString,
    /// The conversation's mode as its box shows it (a new one's: as this computer's would be).
    approval: Option<Shown>,
    /// This computer's modes, for a new conversation: the one it starts with, unless one's picked.
    here: Option<Here>,
    picked_approval: Option<Approval>,
    /// Which pick of the mode is the latest: an older one's answer coming back late is left.
    approval_picks: u64,
    /// Full Access picked where it isn't confirmed on this computer yet: asked here first.
    confirming_full_access: bool,
    /// A new conversation in a project starts in a worktree of its own.
    worktree: bool,
    /// The computer it's on, as the Full Access question names it.
    computer: SharedString,
    tasks: Vec<Task<()>>,
    _field: Subscription,
    _pasting: Subscription,
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Focusable for Composer {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.field.focus_handle(cx)
    }
}

impl Composer {
    /// A composer for `target`. `read`: whether to read what it shows (the model, the efforts,
    /// "/"'s offers) from the box now.
    pub fn new(client: BoxClient, target: Target, read: bool, cx: &mut Context<Self>) -> Composer {
        Composer::in_folder(client, target, None, read, cx)
    }

    /// The same, for a conversation in `folder` (as its box lists it): what's offered is read
    /// there from the first read (`set_folder`).
    pub fn in_folder(client: BoxClient, target: Target, folder: Option<String>, read: bool, cx: &mut Context<Self>) -> Composer {
        Self::make(client, target, folder, read, false, cx)
    }

    /// The shared composer attachment flow, with an item description instead of chat controls.
    pub fn item(client: BoxClient, cx: &mut Context<Self>) -> Composer {
        Self::make(client, Target::New(StartIn::Chat), None, false, true, cx)
    }

    fn make(client: BoxClient, target: Target, folder: Option<String>, read: bool, item_editor: bool, cx: &mut Context<Self>) -> Composer {
        let placeholder: SharedString = match &target {
            Target::Session { .. } => "Ask anything, / for commands".into(),
            Target::New(_) => "What should we work on?".into(),
            Target::Group { .. } => "Message the group…".into(),
        };
        // (A group chat's messages carry no files.)
        let takes_files = !matches!(target, Target::Group { .. });
        let field = cx.new(|cx| {
            let field = TextField::new(cx).multi_line(if item_editor { 3 } else { 1 }, if item_editor { 12 } else { 10 }).bare().submits_on_enter(!item_editor).placeholder(if item_editor { "Add description…".into() } else { placeholder.clone() });
            if takes_files && !item_editor { field.takes_files() } else { field }
        });
        let pasting = cx.subscribe(&field, |this: &mut Composer, _, pasted: &Pasted, cx| this.paste(pasted.clone(), cx));
        let subscription = cx.subscribe(&field, |this: &mut Composer, _, event: &TextFieldEvent, cx| {
            if this.item_editor {
                cx.notify();
                return;
            }
            match event {
                TextFieldEvent::Submit(_) => this.send(cx),
                TextFieldEvent::Cancel => {
                    if this.running && this.text(cx).trim().is_empty() {
                        this.stop(cx);
                    }
                }
                TextFieldEvent::Changed => {
                    this.slash_at = 0;
                    this.read_mentions(cx);
                    if this.note.is_some() && this.text(cx) != this.noted {
                        this.note = None;
                    }
                    cx.notify();
                }
            }
        });
        let mut composer = Composer {
            item_editor,
            item_attaching: false,
            attachment_generation: 0,
            item_keys: Vec::new(),
            item_key: 0,
            client,
            target,
            field,
            field_held: false,
            running: false,
            sending: false,
            on_its_way: None,
            has_queue: false,
            send_menu: PopoverMenuHandle::default(),
            model_menu: PopoverMenuHandle::default(),
            catalog: None,
            outgoing: Vec::new(),
            live: read,
            models: Models::default(),
            picked: None,
            folder,
            reads: 0,
            retry: RETRY_FIRST,
            retrying: None,
            unsaid: false,
            picking: None,
            place_known: false,
            shown: true,
            offers: None,
            slash_scroll: ScrollHandle::new(),
            slash_at: 0,
            slash_dismissed: None,
            note: None,
            noted: String::new(),
            attached: Vec::new(),
            quotes: Vec::new(),
            answers: Vec::new(),
            mentions: Vec::new(),
            mentions_for: None,
            mention_reads: 0,
            mention_pending: false,
            named: Vec::new(),
            members: Vec::new(),
            placeholder,
            approval: None,
            here: None,
            picked_approval: None,
            approval_picks: 0,
            confirming_full_access: false,
            worktree: false,
            computer: "this computer".into(),
            tasks: Vec::new(),
            _field: subscription,
            _pasting: pasting,
        };
        // A group chat has no model of its own to pick, nor "/" to offer: its bots each have theirs.
        if read && !matches!(composer.target, Target::Group { .. }) {
            composer.read_models(cx);
            composer.read_offers(cx);
            // (A conversation's mode is read as it's shown: `set_shown`.)
            if matches!(composer.target, Target::New(_)) {
                composer.read_approval(cx);
            }
        }
        composer
    }

    /// A composer for group chat `id`, named `name` in its placeholder.
    pub fn for_group(client: BoxClient, id: String, name: &str, live: bool, cx: &mut Context<Self>) -> Composer {
        let mut composer = Composer::new(client, Target::Group { id }, live, cx);
        composer.set_group_name(name, cx);
        composer
    }

    /// The group chat's name, as its placeholder says it.
    pub fn set_group_name(&mut self, name: &str, cx: &mut Context<Self>) {
        let placeholder: SharedString = format!("Message {name}…").into();
        if self.placeholder != placeholder {
            self.placeholder = placeholder.clone();
            if !self.client.gate().is_held() {
                self.field.update(cx, |field, _| field.set_placeholder(placeholder));
            }
            cx.notify();
        }
    }

    /// A group chat's bots, as its box lists them now.
    pub fn set_members(&mut self, members: Vec<GroupMember>, cx: &mut Context<Self>) {
        if self.members != members {
            self.members = members;
            self.mentions_for = None;
            self.read_mentions(cx);
            cx.notify();
        }
    }

    /// "&Name " where the message goes on: the bot's asked by name.
    pub fn ask(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mut text = self.text(cx);
        if !text.is_empty() && !text.ends_with(char::is_whitespace) {
            text.push(' ');
        }
        text.push_str(&format!("&{name} "));
        self.field.update(cx, |field, cx| field.set_text(text, cx));
        window.focus(&self.field.focus_handle(cx), cx);
        cx.notify();
    }

    pub fn field(&self) -> &Entity<TextField> {
        &self.field
    }

    pub fn target(&self) -> &Target {
        &self.target
    }

    /// What the chips show, as the box last said it.
    pub fn models(&self) -> &Models {
        &self.models
    }

    pub fn note(&self) -> Option<&Note> {
        self.note.as_ref()
    }

    fn text(&self, cx: &App) -> String {
        self.field.read(cx).text().to_owned()
    }

    /// Where a new conversation starts (the landing view's project chip).
    pub fn set_start(&mut self, place: StartIn, cx: &mut Context<Self>) {
        // (Its models are read again by what moved it: opening the landing view, or a pick.)
        if let Target::New(was) = &self.target
            && *was != place
        {
            self.target = Target::New(place);
            cx.notify();
        }
    }

    /// The conversation's folder, where what's offered is read (none known yet).
    pub fn folder(&self) -> Option<&str> {
        self.folder.as_deref()
    }

    /// The conversation's folder, as its box lists it: what's offered is read there, its project's
    /// own models too (`models.offered.folder`). Whether it was read again for it (a folder it
    /// didn't have, in the app).
    pub fn set_folder(&mut self, folder: Option<String>, cx: &mut Context<Self>) -> bool {
        if self.folder == folder {
            return false;
        }
        self.folder = folder;
        if self.live {
            self.read_models(cx);
        }
        self.live
    }

    /// The folder what's offered is read in: a conversation's, or the project a new one starts in.
    fn offered_in(&self) -> Option<String> {
        match &self.target {
            Target::Session { .. } => self.folder.clone(),
            // (The box reads its own lists for one that isn't a folder there.)
            Target::New(StartIn::Project(project)) => Some(project.clone()),
            Target::New(_) | Target::Group { .. } => None,
        }
    }

    /// What became of `written` and its attached `files`, sent from the landing view to the
    /// conversation this composer is for: the box's answer (what goes back, its note), or, where it
    /// couldn't go, both and why.
    pub fn take_sent(&mut self, written: &str, files: Vec<PathBuf>, sent: Result<Sent, String>, cx: &mut Context<Self>) {
        match sent {
            // Shown in it at once, until its read has it.
            Ok(Sent::Message { id }) => {
                let draft = Draft::new(id.clone(), written.to_owned());
                self.outgoing.push(Outgoing { id, words: written.to_owned(), files, failed: None, taken: true, arrived: false, draft });
                cx.emit(ComposerEvent::Outgoing);
            }
            Ok(sent) => self.sent(sent, written, files, cx),
            Err(error) => {
                self.attach(files, cx);
                self.put_back(written, Some(error), cx);
            }
        }
        cx.notify();
    }

    pub fn set_running(&mut self, running: bool, cx: &mut Context<Self>) {
        if self.running != running {
            self.running = running;
            cx.notify();
        }
    }

    /// Whether anything waits in the conversation's queue, as its box says.
    pub fn set_has_queue(&mut self, has_queue: bool, cx: &mut Context<Self>) {
        if self.has_queue != has_queue {
            self.has_queue = has_queue;
            cx.notify();
        }
    }

    /// Whether "Queue for After This Run" has anything to wait for: a run, or a queue.
    pub fn can_queue(&self) -> bool {
        matches!(self.target, Target::Session { .. }) && (self.running || self.has_queue)
    }

    /// The model the chip names: the conversation's as its box says, else the one picked here.
    pub fn model(&self) -> Option<ModelRef> {
        match &self.target {
            Target::Session { .. } => self.models.state.as_ref().and_then(|state| state.model.clone()),
            Target::New(_) => self.picked.clone(),
            Target::Group { .. } => None,
        }
    }

    /// What the box offers: for a conversation, its own side's models (it's put on one of those in
    /// place); for a new one, all of them. Those a picker leaves out (retiring, hidden) aren't here.
    pub fn menu_models(&self) -> Vec<OfferedModel> {
        let side = match &self.target {
            Target::Session { place, .. } => Some(Side::of(*place)),
            Target::New(_) | Target::Group { .. } => None,
        };
        let models = self.models.offered.as_ref().map(|offered| offered.models.as_slice()).unwrap_or_default();
        models.iter().filter(|model| model.is_offered() && (self.models.offered.as_ref().is_some_and(|offered| offered.scoped) || side.is_none_or(|side| model.side == side))).cloned().collect()
    }

    /// Whether it's on screen. A kept conversation's composer isn't asked again in the background;
    /// shown again, what the box couldn't say is asked for at once.
    pub fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        if self.shown == shown {
            return;
        }
        self.shown = shown;
        if !shown {
            self.retrying = None;
        } else {
            if self.unsaid && self.live {
                self.read_models(cx);
            }
            // (Its mode may have changed elsewhere meanwhile: on the phone, or by its bot.)
            if self.live && matches!(self.target, Target::Session { .. }) {
                self.read_approval(cx);
            }
        }
    }

    /// What the model chip says: the model's name, "Choose a Model", or (its model not said yet, or
    /// not read) "Model", so one can still be picked; none before the box has said anything.
    pub fn chip_label(&self) -> Option<String> {
        let model = self.model();
        if self.models.offered.is_none() && model.is_none() {
            return None;
        }
        if self.needs_model() {
            return Some("Choose a Model".to_owned());
        }
        let named = self.models.state.as_ref().and_then(|state| state.name.clone());
        let listed = model.as_ref().and_then(|model| self.menu_models().into_iter().find(|offered| offered.is(model))).map(|offered| offered.name);
        Some(named.or(listed).or_else(|| model.map(|model| model.id)).unwrap_or_else(|| "Model".to_owned()))
    }

    /// Whether the box offers models of another side than the conversation's, which it can't be
    /// put on in place.
    pub fn left_out(&self) -> bool {
        if self.models.offered.as_ref().is_some_and(|offered| offered.scoped) { return false; }
        let Target::Session { place, .. } = &self.target else { return false };
        let models = self.models.offered.as_ref().map(|offered| offered.models.as_slice()).unwrap_or_default();
        models.iter().any(|model| model.is_offered() && model.side != Side::of(*place))
    }

    /// The model's efforts, as the box offers it.
    pub fn efforts(&self) -> Vec<String> {
        let Some(model) = self.model() else { return Vec::new() };
        let models = self.models.offered.as_ref().map(|offered| offered.models.as_slice()).unwrap_or_default();
        models.iter().find(|offered| offered.is(&model)).map(|offered| offered.efforts.clone()).unwrap_or_default()
    }

    /// Whether Send waits for a model to be chosen: the conversation's is gone or there's none, or
    /// (a new one) the box says nothing runs and none's picked.
    pub fn needs_model(&self) -> bool {
        match &self.target {
            Target::Session { .. } => self.models.state.as_ref().is_some_and(|state| state.choose),
            Target::New(_) => self.picked.is_none() && self.models.offered.as_ref().is_some_and(|offered| offered.starting == Some(None)),
            Target::Group { .. } => false,
        }
    }

    /// Takes what the box says of the models, as a read brings it (tests and shots hand it in).
    pub fn take_models(&mut self, models: Models, cx: &mut Context<Self>) {
        if self.picked.is_none() {
            self.picked = models.offered.as_ref().and_then(|offered| offered.starting.clone().flatten());
        }
        self.models = models;
        self.catalog = None;
        cx.notify();
    }

    pub fn take_offers(&mut self, offers: Offers, cx: &mut Context<Self>) {
        self.offers = Some(offers);
        cx.notify();
    }

    /// Runs `work` off the UI thread, then `then` with what it gave.
    fn spawn<T: Send + 'static>(&mut self, cx: &mut Context<Self>, work: impl FnOnce() -> T + Send + 'static, then: impl FnOnce(&mut Composer, T, &mut Context<Self>) + 'static) {
        let task = cx.background_executor().spawn(async move { work() });
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| then(this, result, cx)).ok();
        }));
    }

    /// Reads the conversation's model and what the box offers. What a side couldn't say is kept
    /// as it said it before, and asked again in a while (PROTOCOL.md, "The models offered"); a model
    /// whose state couldn't be read keeps what was said of it.
    pub fn read_models(&mut self, cx: &mut Context<Self>) {
        self.reads += 1;
        let read = self.reads;
        let (client, target, picked, before, folder) = (self.client.clone(), self.target.clone(), self.picked.clone(), self.models.offered.clone(), self.offered_in());
        self.spawn(cx, move || models_of(&client, &target, picked.as_ref(), folder.as_deref(), before.as_ref()), move |this, (offered, said_all, state), cx| {
            if this.reads == read {
                this.took_models(offered, said_all, state, cx);
            }
        });
    }

    /// What a read of the models said, taken: a failed part keeps what was there. `said_all`:
    /// whether the box itself could say both sides (before what it said earlier was kept).
    fn took_models(&mut self, offered: Option<Offered>, said_all: bool, state: Option<Said>, cx: &mut Context<Self>) {
        let mut said_all = said_all;
        if let Some(offered) = offered {
            if self.picked.is_none() {
                self.picked = offered.starting.clone().flatten();
            }
            if self.models.offered.as_ref() != Some(&offered) {
                self.catalog = None;
            }
            self.models.offered = Some(offered);
        }
        match state {
            // While a pick is on its way, what's said of another model is of the one before.
            Some(Said::Is(state)) if self.picking.as_ref().is_some_and(|picking| state.model.as_ref() != Some(picking)) => {}
            Some(Said::Is(state)) => self.models.state = Some(*state),
            // Left out (the box can't say yet), or not read: its model and name stay, with no
            // refusal of its own (PROTOCOL.md, "A session's model"), and it's asked again.
            Some(Said::Unsaid) => {
                if let Some(state) = self.models.state.as_mut() {
                    state.unsaid();
                }
                said_all = false;
            }
            None => {}
        }
        // A side that couldn't say (or a read that failed): asked again in a while, longer each
        // time, while it's on screen.
        self.unsaid = !said_all;
        if said_all {
            self.retry = RETRY_FIRST;
            self.retrying = None;
        } else if self.live && self.shown {
            let wait = self.retry;
            self.retry = (self.retry * 2).min(RETRY_MOST);
            self.retrying = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(wait).await;
                this.update(cx, |this, cx| this.read_models(cx)).ok();
            }));
        }
        cx.notify();
    }

    /// What its box says of the conversation's model, handed in as a read would bring it (tests).
    #[cfg(feature = "stand-in")]
    pub fn take_state(&mut self, state: ModelState, cx: &mut Context<Self>) {
        self.took_models(None, true, Some(Said::Is(Box::new(state))), cx);
    }

    /// Reads what the chips and "/" show at once, on this thread (shots and tests, against a stand-in).
    #[cfg(feature = "stand-in")]
    pub fn read_at_once(&mut self, cx: &mut Context<Self>) {
        let (offered, said_all, state) = models_of(&self.client, &self.target, self.picked.as_ref(), self.offered_in().as_deref(), self.models.offered.as_ref());
        self.took_models(offered, said_all, state, cx);
        match &self.target {
            Target::Session { id, .. } => self.approval = self.client.session_approval(id).ok(),
            Target::New(_) => self.here = self.client.approval_here().ok(),
            Target::Group { .. } => {}
        }
        if let Target::Session { id, .. } = &self.target
            && let Ok(offers) = self.client.typed_offers(id)
        {
            self.take_offers(offers, cx);
        }
    }

    /// Where the conversation lives on its box, once its follow has found it.
    pub fn set_place(&mut self, place: Where, cx: &mut Context<Self>) {
        if !self.place_known {
            self.place_known = true;
            cx.notify();
        }
        if let Target::Session { place: was, .. } = &mut self.target
            && *was != place
        {
            *was = place;
            // (Its picker offers that side's models.)
            self.catalog = None;
            self.read_models(cx);
        }
    }

    /// Whether its model can be changed here now: a new one's always; a conversation's once it's
    /// known where it runs.
    pub fn can_pick(&self) -> bool {
        matches!(self.target, Target::New(_)) || self.place_known
    }

    fn read_offers(&mut self, cx: &mut Context<Self>) {
        let Target::Session { id, .. } = &self.target else { return };
        let (client, id) = (self.client.clone(), id.clone());
        self.spawn(cx, move || client.typed_offers(&id).ok(), |this, offers, cx| {
            if let Some(offers) = offers {
                this.take_offers(offers, cx);
            }
        });
    }

    /// Picks a model (and effort) for the conversation, or for the new one.
    pub fn pick(&mut self, model: ModelRef, cx: &mut Context<Self>) {
        match self.target.clone() {
            // (Each of a group chat's bots has its own model.)
            Target::Group { .. } => {}
            Target::New(_) => {
                self.picked = Some(model);
                self.read_models(cx);
            }
            Target::Session { id, place } => {
                // A read already on its way says what was before: it's left, and so is what's said
                // of another model until the change comes back.
                self.reads += 1;
                self.picking = Some(model.clone());
                // Shown at once; what its box says of it comes with the next read.
                // As without the box's word on it, until it's asked again.
                let name = self.menu_models().into_iter().find(|offered| offered.is(&model)).map(|offered| offered.name);
                if let Some(state) = self.models.state.as_mut() {
                    state.model = Some(model.clone());
                    state.name = name.or(state.name.take());
                    state.unsaid();
                }
                cx.notify();
                let client = self.client.clone();
                let picked = model.clone();
                self.spawn(cx, move || client.select_model(&id, place, &model), move |this, result, cx| {
                    // (Only the latest pick's change ends the wait: an earlier one's coming back
                    // first leaves the latest's.)
                    if this.picking.as_ref() == Some(&picked) {
                        this.picking = None;
                    }
                    if let Err(error) = result {
                        this.set_note(Note { text: format!("The model wasn’t changed: {error}").into(), warning: true }, cx);
                    }
                    this.read_models(cx);
                });
            }
        }
    }

    /// The computer it's on, by name, for the Full Access question ("This Mac", a machine's name).
    pub fn set_computer(&mut self, name: SharedString, cx: &mut Context<Self>) {
        if self.computer != name {
            self.computer = name;
            cx.notify();
        }
    }

    /// Whether it reads the box by itself (the app), rather than showing what it's handed.
    pub fn reads(&self) -> bool {
        self.live
    }

    /// Reads the mode: the conversation's, or for a new one, this computer's.
    pub fn read_approval(&mut self, cx: &mut Context<Self>) {
        match self.target.clone() {
            Target::Group { .. } => {}
            Target::Session { id, .. } => {
                let (client, picks) = (self.client.clone(), self.approval_picks);
                self.spawn(cx, move || client.session_approval(&id), move |this, read, cx| {
                    // (A pick since says newer than this read.)
                    if let (Ok(shown), true) = (read, this.approval_picks == picks) {
                        this.approval = Some(shown);
                        cx.notify();
                    }
                });
            }
            Target::New(_) => {
                let client = self.client.clone();
                self.spawn(cx, move || client.approval_here(), |this, read, cx| {
                    if let Ok(here) = read {
                        this.here = Some(here);
                        cx.notify();
                    }
                });
            }
        }
    }

    /// The mode as the menu shows it: the conversation's as its box says, or the one a new
    /// conversation will start with.
    pub fn approval(&self) -> Option<Shown> {
        match &self.target {
            Target::Session { .. } => self.approval.clone(),
            Target::New(_) => self.here.as_ref().map(|here| here.shown(self.picked_approval.clone().unwrap_or_else(|| here.default.clone()))),
            Target::Group { .. } => None,
        }
    }

    /// Whether Full Access waits for you to confirm it on this computer.
    pub fn is_confirming_full_access(&self) -> bool {
        self.confirming_full_access
    }

    /// Picks the mode: set on the box at once for a conversation (its box's answer shows), kept for
    /// a new one until it starts. Full Access not confirmed on this computer yet asks first.
    pub fn pick_approval(&mut self, approval: Approval, cx: &mut Context<Self>) {
        let Some(shown) = self.approval() else { return };
        match shown.choice(&approval).and_then(|choice| choice.unavailable.as_ref()).map(|why| why.reason.as_str()) {
            None => {}
            Some("fullAccessOff") => {
                self.confirming_full_access = true;
                cx.notify();
                return;
            }
            // Not to be had here, as its box says: shown, not picked.
            Some(_) => return,
        }
        self.confirming_full_access = false;
        match self.target.clone() {
            Target::Group { .. } => {}
            Target::New(_) => {
                self.picked_approval = Some(approval);
                cx.notify();
            }
            Target::Session { id, .. } => {
                self.approval_picks += 1;
                let pick = self.approval_picks;
                let before = self.approval.clone();
                if let Some(shown) = self.approval.as_mut() {
                    shown.approval = approval.clone();
                }
                cx.notify();
                let client = self.client.clone();
                self.spawn(cx, move || client.set_session_approval(&id, &approval), move |this, result, cx| {
                    if this.approval_picks != pick {
                        return;
                    }
                    match result {
                        Ok(Picked::Set(shown)) => this.approval = Some(shown),
                        Ok(Picked::ConfirmFirst) => {
                            this.approval = before;
                            this.confirming_full_access = true;
                        }
                        Err(error) => {
                            this.approval = before;
                            this.set_note(Note { text: format!("The mode wasn’t changed: {}", kuvo_box::accounts::said(&error)).into(), warning: true }, cx);
                        }
                    }
                    cx.notify();
                });
            }
        }
    }

    /// You confirmed Full Access on this computer: it's switched on there, then picked.
    pub fn confirm_full_access(&mut self, cx: &mut Context<Self>) {
        self.confirming_full_access = false;
        cx.notify();
        let client = self.client.clone();
        self.spawn(cx, move || client.allow_full_access(), |this, result, cx| {
            match result {
                Ok(()) => {
                    if let Some(here) = this.here.as_mut() {
                        here.full_access = true;
                    }
                    if let Some(choice) = this.approval.as_mut().and_then(|shown| shown.options.iter_mut().find(|choice| choice.approval == Approval::FullAccess)) {
                        choice.unavailable = None;
                    }
                    this.pick_approval(Approval::FullAccess, cx);
                }
                Err(error) => this.set_note(Note { text: format!("Full Access wasn’t switched on: {}", kuvo_box::accounts::said(&error)).into(), warning: true }, cx),
            }
            cx.notify();
        });
    }

    /// Full Access left as it was.
    pub fn cancel_full_access(&mut self, cx: &mut Context<Self>) {
        self.confirming_full_access = false;
        cx.notify();
    }

    /// Whether a new conversation in a project starts in a worktree of its own (Local: no).
    pub fn set_worktree(&mut self, worktree: bool, cx: &mut Context<Self>) {
        if self.worktree != worktree {
            self.worktree = worktree;
            cx.notify();
        }
    }

    /// Whether Local or Worktree is offered: a new conversation in a project.
    pub fn offers_worktree(&self) -> bool {
        matches!(self.target, Target::New(StartIn::Project(_)))
    }

    /// Whether what's sent starts in a worktree of its own.
    pub fn starts_in_worktree(&self) -> bool {
        self.worktree && self.offers_worktree()
    }

    /// Sends what's written (or, in a new conversation, starts it on the box and sends it there).
    pub fn send(&mut self, cx: &mut Context<Self>) {
        self.send_as(SendAs::Steer, cx);
    }

    /// Sends what's written as `how` says: with nothing to wait for (no run, no queue), what's for
    /// after the run goes now, as 0.7's did.
    pub fn send_as(&mut self, how: SendAs, cx: &mut Context<Self>) {
        if self.client.gate().is_held() { return; }
        let text = self.text(cx);
        let group = matches!(self.target, Target::Group { .. });
        if (text.trim().is_empty() && (group || (self.attached.is_empty() && self.quotes.is_empty()))) || self.sending {
            return;
        }
        let mut draft = Draft::new(BoxClient::new_id(), text.clone());
        // How it goes is its box's to work out from what it asks: for after the run (its box sends
        // it at once with nothing to wait for), or now, the step under way stopped for it.
        draft.delivery = match how {
            SendAs::Steer => Delivery::Steer,
            SendAs::Queue => Delivery::Queue,
            SendAs::Now => Delivery::Now,
        };
        // (Its files go with it as its box takes them, read as it goes: `go`.)
        let files = std::mem::take(&mut self.attached);
        // The agents picked from "&"'s list that are still named in it.
        draft.named = self.named.iter().filter(|(text, _)| mentioned(&draft.text, text)).map(|(_, session)| session.clone()).collect();
        draft.named.dedup();
        draft.refusal = self.models.state.as_ref().and_then(|state| state.refusal.clone());
        // ("/" and "!" texts go to the box, which says whether they need the model: "!" commands and
        // a goal's pause don't.)
        if self.needs_model() && !(matches!(self.target, Target::Session { .. }) && typed::asks_box(&text)) {
            self.attached = files;
            // As its box words it, for a conversation; a new one's box says only that none runs.
            let words = self.models.state.as_ref().and_then(|state| state.refusal.clone()).unwrap_or_else(|| "Choose a model to send it.".to_owned());
            self.set_note(Note { text: words.into(), warning: true }, cx);
            cx.notify();
            return;
        }
        self.sending = true;
        self.note = None;
        self.slash_dismissed = None;
        self.field.update(cx, |field, cx| {
            field.set_text("", cx);
            field.set_error(None, cx);
        });
        cx.notify();
        let client = self.client.clone();
        match self.target.clone() {
            Target::Group { id } => {
                self.attached = files;
                self.named.clear();
                let words = text.clone();
                self.spawn(cx, move || client.write_to_group(&id, &words), move |this, result, cx| {
                    this.sending = false;
                    match result {
                        Ok(()) => cx.emit(ComposerEvent::Wrote),
                        // Turned down (moving, its bridge not there): it stays to send again, with why.
                        Err(error) => this.put_back(&text, Some(kuvo_box::accounts::said(&error)), cx),
                    }
                    cx.notify();
                });
            }
            Target::Session { id, place } => {
                draft.place = self.place_known.then_some(place);
                // What it replies to goes ahead of its words: a plain message's now (so it shows so
                // at once), a typed one's as its box says it sends (`send_draft`'s `quote`).
                self.sync_answers(cx);
                let quotes = std::mem::take(&mut self.quotes);
                self.answers.clear();
                // A plain message shows in the conversation at once, as you wrote it ("/" and "!"
                // texts are the box's to say what they are; one kept for a model that can't run
                // goes back to the composer, saying why).
                let shows = !typed::asks_box(&text) && typed::plain_answer(&text, draft.side, draft.refusal.as_deref()).is_none();
                let (text, quotes) = if shows && !quotes.is_empty() { (quoting::message(&quotes, &text), Vec::new()) } else { (text, quotes) };
                draft.text = text.clone();
                if shows {
                    self.outgoing.push(Outgoing { id: draft.id.clone(), words: text.clone(), files: files.clone(), failed: None, taken: false, arrived: false, draft: draft.clone() });
                    cx.emit(ComposerEvent::Outgoing);
                }
                self.go(id, draft, text, files, quotes, shows, cx);
            }
            Target::New(place) => {
                let model = self.picked.clone();
                let how = StartHow { worktree: self.starts_in_worktree(), approval: self.picked_approval.clone() };
                let (paths, here, pasted) = (files.clone(), client.here(), crate::pasted::folder(cx));
                self.spawn(
                    cx,
                    move || {
                        // (Its files read first: one that can't go starts nothing.)
                        draft.files = kuvo_box::parts::carried(&paths, here, &|path| path.starts_with(&pasted))?;
                        let started = client.start_session(&place, model.as_ref(), &how).map_err(|error| error.to_string())?;
                        let sent = client.send_draft(&started.session, &draft, None).map_err(|error| error.to_string());
                        Ok::<_, String>((started, sent))
                    },
                    move |this, result, cx| {
                        this.sending = false;
                        match result {
                            // Started: what was written, its attached files, and what the box said
                            // of it go with the conversation; this composer is left empty for the
                            // next one.
                            Ok((started, sent)) => {
                                // (The box keeps the mode picked as the next one's to start in.)
                                this.picked_approval = None;
                                cx.emit(ComposerEvent::Started {
                                session: started.session,
                                folder: started.folder,
                                project: started.project,
                                written: text.clone(),
                                files,
                                sent,
                            })
                            }
                            Err(error) => {
                                this.attach(files, cx);
                                this.put_back(&text, Some(error), cx)
                            }
                        }
                        cx.notify();
                    },
                );
            }
        }
    }

    /// Sends `draft` to conversation `session` (`send` or Retry): one at a time, Stop taking it as
    /// it goes. `shows`: it's drawn in the conversation (`outgoing`), which says what became of it;
    /// else what was written goes back to the composer if it couldn't go.
    fn go(&mut self, session: String, draft: Draft, text: String, files: Vec<PathBuf>, quotes: Vec<Quote>, shows: bool, cx: &mut Context<Self>) {
        // (One at a time: `send` and Retry refuse another while this one's on its way.)
        debug_assert!(self.on_its_way.is_none(), "a second send while one was on its way");
        self.sending = true;
        let client = self.client.clone();
        let id = draft.id.clone();
        self.on_its_way = Some(id.clone());
        // Its files as its box takes them: where they are, for this computer's; with their bytes,
        // for another's, which keeps them there (`parts::carried`). One that can't go: not sent.
        let (paths, here, pasted) = (files.clone(), client.here(), crate::pasted::folder(cx));
        let kept_quotes = quotes.clone();
        let work = move || {
            let mut draft = draft;
            draft.files = kuvo_box::parts::carried(&paths, here, &|path| path.starts_with(&pasted))?;
            let quote = |words: &str| quoting::message(&quotes, words);
            let quote: Option<&dyn Fn(&str) -> String> = if quotes.is_empty() { None } else { Some(&quote) };
            client.send_draft(&session, &draft, quote).map_err(|error| error.to_string())
        };
        self.spawn(cx, work, move |this, result, cx| {
            this.sending = false;
            this.on_its_way = None;
            match result {
                Ok(Sent::Message { .. }) if shows => this.mark(&id, |outgoing| outgoing.taken = true, cx),
                Ok(sent) => {
                    this.forget(&id, cx);
                    this.sent(sent, &text, files, cx);
                }
                Err(why) if shows => this.mark(&id, |outgoing| outgoing.failed = Some(why), cx),
                Err(why) => {
                    this.attach(files, cx);
                    // (What it replied to goes back with it.)
                    let mut quotes = kept_quotes;
                    this.sync_answers(cx);
                    quotes.append(&mut this.quotes);
                    this.set_quotes(quotes, cx);
                    this.put_back(&text, Some(why), cx)
                }
            }
            cx.notify();
        });
    }

    /// Whether a message is on its way now (another waits until it's gone).
    pub fn is_sending(&self) -> bool {
        self.sending
    }

    /// Messages sent from here that the conversation doesn't have yet, in the order sent.
    pub fn outgoing(&self) -> &[Outgoing] {
        &self.outgoing
    }

    /// Those the conversation (or its queue) now has (`has`, by id: whether its copy carries
    /// files), let go: its copy takes their place. (One said not to have gone that the box has did
    /// go: its answer was lost on the way.) One whose copy came without the files sent with it
    /// keeps them, shown under its copy (`arrived`), until a copy with its own comes.
    pub fn arrived(&mut self, has: impl Fn(&str) -> Option<bool>, cx: &mut Context<Self>) {
        let before = self.outgoing.clone();
        self.outgoing.retain_mut(|outgoing| match has(&outgoing.id) {
            None => true,
            Some(false) if !outgoing.files.is_empty() => {
                outgoing.arrived = true;
                outgoing.failed = None;
                true
            }
            Some(_) => false,
        });
        if self.outgoing != before {
            cx.emit(ComposerEvent::Outgoing);
        }
    }

    /// Sends one that couldn't go again, under its id; not while another's on its way.
    pub fn retry(&mut self, id: &str, cx: &mut Context<Self>) {
        let Target::Session { id: session, .. } = self.target.clone() else { return };
        if self.sending || self.client.gate().is_held() {
            return;
        }
        let Some(outgoing) = self.outgoing.iter_mut().find(|outgoing| outgoing.id == id && outgoing.failed.is_some()) else { return };
        outgoing.failed = None;
        let (draft, text, files) = (outgoing.draft.clone(), outgoing.words.clone(), outgoing.files.clone());
        cx.emit(ComposerEvent::Outgoing);
        self.go(session, draft, text, files, Vec::new(), true, cx);
        cx.notify();
    }

    /// One that couldn't go, back in the composer (ahead of anything written since) to change.
    pub fn edit_unsent(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(at) = self.outgoing.iter().position(|outgoing| outgoing.id == id && outgoing.failed.is_some()) else { return };
        let outgoing = self.outgoing.remove(at);
        self.attach(outgoing.files, cx);
        self.put_back(&outgoing.words, None, cx);
        cx.emit(ComposerEvent::Outgoing);
        cx.notify();
    }

    fn mark(&mut self, id: &str, change: impl FnOnce(&mut Outgoing), cx: &mut Context<Self>) {
        if let Some(outgoing) = self.outgoing.iter_mut().find(|outgoing| outgoing.id == id) {
            change(outgoing);
            cx.emit(ComposerEvent::Outgoing);
        }
    }

    /// One taken back from its box (Edit or Remove while it waited): it isn't on its way any more.
    pub fn let_go(&mut self, id: &str, cx: &mut Context<Self>) {
        self.forget(id, cx);
    }

    fn forget(&mut self, id: &str, cx: &mut Context<Self>) {
        let before = self.outgoing.len();
        self.outgoing.retain(|outgoing| outgoing.id != id);
        if self.outgoing.len() != before {
            cx.emit(ComposerEvent::Outgoing);
        }
    }

    /// What the box did with a typed text, shown; what goes back to the composer, put back. Files
    /// stay attached unless the answer took them (`send` or `command`). What this client is to do
    /// that it can't yet keeps what was written (`written`).
    fn sent(&mut self, sent: Sent, written: &str, files: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Sent::Typed { answer, .. } = sent else { return };
        if !answer.takes_files() {
            self.attach(files, cx);
        }
        let Answer { restore, error, .. } = (*answer).clone();
        let note = said(&answer);
        if restore.is_none() && is_clients_to_do(&answer) {
            self.put_back(written, None, cx);
        }
        if let Some(restore) = restore {
            let now = self.text(cx);
            let joined = ahead_of(restore.text_beside(&now), &now);
            self.field.update(cx, |field, cx| field.set_text(joined, cx));
        }
        self.field.update(cx, |field, cx| field.set_error(error.map(SharedString::from), cx));
        if let Some(note) = note {
            self.set_note(note, cx);
        }
    }

    /// Shows `note` under the composer, until what's written changes.
    fn set_note(&mut self, note: Note, cx: &mut Context<Self>) {
        self.note = Some(note);
        self.noted = self.text(cx);
    }

    /// What was written, back in the field (ahead of anything written since), with why.
    fn put_back(&mut self, text: &str, error: Option<String>, cx: &mut Context<Self>) {
        let now = self.text(cx);
        let joined = ahead_of(text, &now);
        self.field.update(cx, |field, cx| {
            field.set_text(joined, cx);
            field.set_error(error.map(SharedString::from), cx);
        });
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if let Target::Group { id } = self.target.clone() {
            let client = self.client.clone();
            self.spawn(cx, move || client.stop_group(&id), |this, result, cx| {
                match result {
                    Ok(()) => cx.emit(ComposerEvent::Wrote),
                    Err(error) => this.set_note(Note { text: format!("It didn’t stop: {}", kuvo_box::accounts::said(&error)).into(), warning: true }, cx),
                }
                cx.notify();
            });
            return;
        }
        let Target::Session { id, .. } = self.target.clone() else { return };
        let client = self.client.clone();
        // (What's still on its way goes with it: its box holds it as it comes.)
        let on_its_way: Vec<String> = self.on_its_way.iter().cloned().collect();
        self.spawn(cx, move || client.stop_session(&id, &on_its_way), |this, result, cx| {
            if let Err(error) = result {
                this.set_note(Note { text: format!("It didn’t stop: {error}").into(), warning: true }, cx);
                cx.notify();
            }
        });
    }

    /// A message taken back from the queue to edit: what its box says goes back (`draft`: a
    /// command with its arguments, say) ahead of anything written since, and its files (those on
    /// this Mac's box; one sent inline can't come back as a file).
    pub fn restore(&mut self, words: &str, files: &[File], cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = files.iter().filter_map(|file| file.uri.strip_prefix("file://")).map(|path| PathBuf::from(percent_decoded(path))).collect();
        self.attach(paths, cx);
        self.put_back(words, None, cx);
        cx.notify();
    }

    /// Start a fresh attachment draft, invalidating pending paste and copy completions.
    pub fn clear_attachments(&mut self, cx: &mut Context<Self>) {
        self.attachment_generation += 1;
        self.item_keys.clear();
        self.item_key = 0;
        if self.item_editor { self.note = None; }
        self.attached.clear();
        self.note = None;
        cx.notify();
    }

    /// Whether an item's description takes files (its computer keeps them, as its box says): where
    /// it doesn't, there's simply nothing to paste or drop them into.
    pub fn set_attaching(&mut self, attaching: bool, cx: &mut Context<Self>) {
        self.item_attaching = attaching;
        self.field.update(cx, |field, _| field.set_takes_files(attaching));
        cx.notify();
    }

    /// What goes with what's written: files and pictures, in order.
    pub fn item_attaching(&self) -> bool {
        self.item_attaching
    }

    pub fn item_attachments(&self) -> impl Iterator<Item = (usize, &PathBuf)> {
        self.item_keys.iter().copied().zip(&self.attached)
    }

    pub fn attached(&self) -> &[PathBuf] {
        &self.attached
    }

    /// Files and pictures to go with what's written (put back, restored). Folders aren't files to
    /// send; one already here isn't added twice. The pictures' names among them are in the words
    /// already.
    pub fn attach(&mut self, paths: impl IntoIterator<Item = PathBuf>, cx: &mut Context<Self>) {
        for path in paths {
            if !path.is_dir() && !self.attached.contains(&path) {
                self.attached.push(path);
            }
        }
        cx.notify();
    }

    /// Files and pictures you just attached (picked, dropped, pasted): each picture's name ("[Image
    /// 2]") written where you're writing, as 0.7 did, so you and the agent can say which you mean.
    /// One from another app's data (a picture dragged from Notes) is copied into this app's own
    /// folder at once, while you hand it over: read later, by the box, macOS would say Kuvo is
    /// accessing another app's data.
    pub fn attach_named(&mut self, paths: impl IntoIterator<Item = PathBuf>, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = paths.into_iter().collect();
        if !paths.iter().any(|path| crate::pasted::is_another_apps(path)) {
            return self.name_attached(paths, cx);
        }
        // (Copied off the UI thread; in the order they came.)
        let folder = crate::pasted::folder(cx);
        let copy = move || {
            paths.into_iter().map(|path| if crate::pasted::is_another_apps(&path) { crate::pasted::keep_copy(&path, &folder, std::time::SystemTime::now()).unwrap_or(path) } else { path }).collect::<Vec<_>>()
        };
        let generation = self.attachment_generation;
        self.spawn(cx, copy, move |this, paths, cx| { if this.attachment_generation == generation { this.name_attached(paths, cx); } });
    }

    fn name_attached(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.item_editor {
            if !self.item_attaching { return; }
            let mut anchors = Vec::new();
            for path in paths {
                if path.is_dir() || self.attached.contains(&path) { continue; }
                let name = path.file_name().map(|name| name.to_string_lossy()).unwrap_or_default();
                anchors.push(crate::queue::attachments::draft_marker(self.item_key, &name));
                self.item_keys.push(self.item_key);
                self.item_key += 1;
                self.attached.push(path);
            }
            if !anchors.is_empty() {
                self.field.update(cx, |field, cx| {
                    let selection = field.selection();
                    let text = field.text();
                    let lead = if selection.start == 0 || text[..selection.start].ends_with('\n') { "" } else { "\n" };
                    field.replace_selection(&format!("{lead}{}
", anchors.join("\n")), cx);
                });
            }
            cx.notify();
            return;
        }
        let mut names = Vec::new();
        for path in paths {
            if path.is_dir() || self.attached.contains(&path) {
                continue;
            }
            self.attached.push(path.clone());
            if is_picture(&path) {
                names.push(image_names::name(self.attached.iter().filter(|each| is_picture(each)).count()));
            }
        }
        // (Not into a "!" command, which is the shell's.)
        if !names.is_empty() && !self.text(cx).starts_with('!') {
            let words = names.join(" ");
            self.field.update(cx, |field, cx| field.write(&words, cx));
        }
        cx.notify();
    }

    /// What ⌘V brought that isn't text: files copied in the Finder, attached; a picture (a copied
    /// screenshot, an image copied from a browser), kept as a file to attach (`pasted`). One that
    /// can't be says so under the composer.
    pub fn paste(&mut self, pasted: Pasted, cx: &mut Context<Self>) {
        match pasted {
            Pasted::Files(paths) => self.attach_named(paths, cx),
            Pasted::Picture(image) => {
                let folder = crate::pasted::folder(cx);
                let generation = self.attachment_generation;
                self.spawn(cx, move || crate::pasted::save(&image, &folder, std::time::SystemTime::now()), move |this, saved, cx| {
                    if this.attachment_generation != generation { return; }
                    match saved {
                        Ok(path) => this.attach_named([path], cx),
                        Err(error) => {
                            this.set_note(Note { text: error.into(), warning: true }, cx);
                            cx.notify();
                        }
                    }
                });
            }
        }
    }

    /// Replies to `text` (a message, or what's selected in one): it goes above what you write, and
    /// ahead of it as it's sent. The same quoted twice is there once.
    /// Its answer field opens under it, to take the keyboard (`answer_focus`).
    pub fn quote(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = text.trim();
        if text.is_empty() || matches!(self.target, Target::Group { .. }) || self.quotes.iter().any(|quote| quote.text == text) {
            return;
        }
        self.quotes.push(Quote::new(text));
        let answer = self.answer("", true, cx);
        self.answers.push(answer);
        cx.notify();
    }

    /// Quote `at`'s answer field (for shots).
    pub fn answer_field(&self, at: usize) -> Option<Entity<TextField>> {
        self.answers.get(at).map(|answer| answer.field.clone())
    }

    /// The field of the last quote's answer, which takes the keyboard as the quote is added.
    pub fn answer_focus(&self, cx: &App) -> Option<FocusHandle> {
        self.answers.last().map(|answer| answer.field.focus_handle(cx))
    }

    /// An answer's field: Return sends the message, as the composer's own field does.
    fn answer(&mut self, response: &str, open: bool, cx: &mut Context<Self>) -> QuoteAnswer {
        let field = cx.new(|cx| {
            let mut field = TextField::new(cx).multi_line(1, 10).bare().submits_on_enter(true).placeholder("Your answer to this…");
            field.set_text(response, cx);
            field
        });
        let events = cx.subscribe(&field, |this: &mut Composer, _, event: &TextFieldEvent, cx| match event {
            TextFieldEvent::Submit(_) => this.send(cx),
            TextFieldEvent::Changed | TextFieldEvent::Cancel => cx.notify(),
        });
        QuoteAnswer { field, open: open || !response.trim().is_empty(), had_focus: false, _events: events }
    }

    /// The answers as their fields hold them, into the quotes.
    fn sync_answers(&mut self, cx: &App) {
        for (quote, answer) in self.quotes.iter_mut().zip(&self.answers) {
            quote.response = answer.field.read(cx).text().to_owned();
        }
    }

    /// `quotes` in place of what's there, each answer in its field.
    fn set_quotes(&mut self, quotes: Vec<Quote>, cx: &mut Context<Self>) {
        self.answers = quotes.iter().map(|quote| self.answer(&quote.response, false, cx)).collect();
        self.quotes = quotes;
        cx.notify();
    }

    /// What you're replying to, in order, each with your answer as written.
    pub fn quotes(&self, cx: &App) -> Vec<Quote> {
        self.quotes.iter().zip(&self.answers).map(|(quote, answer)| Quote { text: quote.text.clone(), response: answer.field.read(cx).text().to_owned() }).collect()
    }

    pub fn unquote(&mut self, at: usize, cx: &mut Context<Self>) {
        if at < self.quotes.len() {
            self.quotes.remove(at);
            self.answers.remove(at);
            cx.notify();
        }
    }

    /// Every quote taken off (Remove All).
    pub fn unquote_all(&mut self, cx: &mut Context<Self>) {
        self.quotes.clear();
        self.answers.clear();
        cx.notify();
    }

    /// Opens a quote's answer, with the keyboard in it (a click on the quote).
    fn open_answer(&mut self, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(answer) = self.answers.get_mut(at) {
            answer.open = true;
            let focus = answer.field.focus_handle(cx);
            window.focus(&focus, cx);
            cx.notify();
        }
    }

    /// What you're replying to, above the field, as 0.7's `QuoteList`: each quote's first lines
    /// ("Replying to" over it while it's the only one), × to take it off, and your answer to it
    /// under it while it has words or is opened (click the quote to answer it); with several, how
    /// many and Remove All. A long list scrolls.
    fn quote_cards(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.quotes.is_empty() {
            return None;
        }
        // An empty answer closes once the keyboard goes on to write somewhere else (the composer's
        // own field, or another answer): not as a button takes it, which a closing answer would
        // move before it's let go.
        let writing_elsewhere = self.field.focus_handle(cx).is_focused(window) || self.answers.iter().any(|answer| answer.field.focus_handle(cx).is_focused(window));
        for answer in &mut self.answers {
            let focused = answer.field.focus_handle(cx).is_focused(window);
            if answer.had_focus && !focused && writing_elsewhere && answer.field.read(cx).text().trim().is_empty() {
                answer.open = false;
            }
            if focused || writing_elsewhere {
                answer.had_focus = focused;
            }
        }
        let c = cx.theme().colors.clone();
        let single = self.quotes.len() == 1;
        let cards = self.quotes.iter().zip(&self.answers).enumerate().map(|(at, (quote, answer))| {
            let selector = format!("quote-{at}");
            let card = div()
                .id(("quote-card", at))
                .debug_selector(move || selector)
                .flex()
                .items_start()
                .gap(space::S2)
                .py(space::S1_5)
                .px(space::S2)
                .rounded(radius::NAV)
                .bg(c.sunken)
                .cursor_pointer()
                // (As it's pressed: an empty answer that closes as the keyboard leaves it would move the
                // quote under the pointer before it's let go.)
                .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, window, cx| {
                    window.prevent_default();
                    this.open_answer(at, window, cx);
                }))
                .child(div().flex_none().w(px(3.)).h(px(18.)).mt(px(1.)).rounded(px(2.)).bg(c.accent))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(space::S0_5)
                        .when(single, |d| d.child(div().text_ui(TypeSize::Caption).medium().text_color(c.text_muted).child("Replying to")))
                        .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).line_clamp(3).child(quote.text.clone())),
                )
                .child(Button::icon_only(("unquote", at), IconName::Close, "Don’t Reply to This").ghost().size(ButtonSize::Small).on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.unquote(at, cx);
                })));
            let shows = answer.open || !answer.field.read(cx).text().trim().is_empty();
            let selector = format!("quote-answer-{at}");
            div()
                .flex()
                .flex_col()
                .gap(space::S1_5)
                .child(card)
                .when(shows, |d| d.child(div().debug_selector(move || selector).pl(space::S3).child(answer.field.clone())))
        });
        let header = (!single).then(|| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .text_ui(TypeSize::Small)
                .text_color(c.text_muted)
                .child(format!("{} quotes", self.quotes.len()))
                .child(div().debug_selector(|| "quotes-remove-all".into()).child(Button::new("quotes-remove-all", "Remove All").ghost().size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.unquote_all(cx)))))
        });
        Some(
            div()
                .debug_selector(|| "quotes".into())
                .flex()
                .flex_col()
                .gap(space::S1)
                .children(header)
                .child(div().id("quote-list").max_h(px(260.)).overflow_y_scroll().flex().flex_col().gap(space::S2).children(cards))
                .into_any_element(),
        )
    }

    /// Takes a file off the message, and a picture's name with it (the later ones a number down).
    pub fn detach(&mut self, at: usize, cx: &mut Context<Self>) {
        if self.item_editor {
            let Some(index) = self.item_keys.iter().position(|key| *key == at) else { return };
            self.item_keys.remove(index);
            self.attached.remove(index);
            self.field.update(cx, |field, cx| {
                let ranges = crate::queue::attachments::draft_ranges(field.text(), at);
                for range in ranges.into_iter().rev() {
                    field.select_range(range, cx);
                    field.replace_selection("", cx);
                }
            });
            cx.notify();
            return;
        }
        if at < self.attached.len() {
            let number = is_picture(&self.attached[at]).then(|| self.attached[..=at].iter().filter(|each| is_picture(each)).count());
            self.attached.remove(at);
            if let Some(number) = number {
                let (now, words) = (self.text(cx), image_names::removing(number, &self.text(cx)));
                // (Only where a name changes: the words, their undo and the caret stay otherwise.)
                if words != now {
                    self.field.update(cx, |field, cx| field.set_text(words, cx));
                }
            }
            cx.notify();
        }
    }

    /// The system's file picker, for files and pictures to attach.
    pub fn pick_files(&mut self, cx: &mut Context<Self>) {
        let generation = self.attachment_generation;
        let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Attach".into()) });
        self.tasks.push(cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                this.update(cx, |this, cx| {
                    if !this.item_editor || this.attachment_generation == generation { this.attach_named(paths, cx); }
                }).ok();
            }
        }));
    }

    /// What's attached, above the field: pictures as thumbnails, other files as chips, each with ×.
    fn attachments(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.attached.is_empty() {
            return None;
        }
        let c = cx.theme().colors.clone();
        let items = self.attached.iter().enumerate().map(|(at, path)| {
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            let remove = cx.listener(move |this, _, _, cx| this.detach(at, cx));
            if is_picture(path) {
                div()
                    .id(("attached", at))
                    .relative()
                    .size(px(52.))
                    .flex_none()
                    .rounded(radius::NAV)
                    .overflow_hidden()
                    .border_1()
                    .border_color(c.border)
                    .bg(c.sunken)
                    .child(img(path.clone()).size_full().object_fit(ObjectFit::Cover).with_fallback(|| Icon::new(IconName::Image).size(IconSize::Card).into_any_element()))
                    .child(
                        div()
                            .id(("detach", at))
                            .absolute()
                            .top(px(3.))
                            .right(px(3.))
                            .rounded(radius::PILL)
                            .bg(c.elevated)
                            .border_1()
                            .border_color(c.border)
                            .cursor_pointer()
                            .on_click(remove)
                            .child(Icon::new(IconName::Close).size(IconSize::Nav).color(c.text_muted)),
                    )
                    .into_any_element()
            } else {
                Chip::new(("attached", at), name).icon(IconName::File).on_remove(move |window, cx| remove(&gpui::ClickEvent::default(), window, cx)).into_any_element()
            }
        });
        Some(div().debug_selector(|| "attachments".into()).flex().flex_wrap().items_center().gap(space::S1_5).children(items).into_any_element())
    }

    /// "/"'s offers for what's written: shown while it's "/" and a name being typed.
    pub fn slash_offers(&self, cx: &App) -> Vec<Offer> {
        let text = self.text(cx);
        let Some(typed) = text.strip_prefix('/') else { return Vec::new() };
        if typed.contains(char::is_whitespace) || self.slash_dismissed.as_deref() == Some(text.as_str()) {
            return Vec::new();
        }
        let Some(offers) = &self.offers else { return Vec::new() };
        let typed = typed.to_lowercase();
        let keywords = offers.keywords.iter().map(|keyword| Offer { name: keyword.name.clone(), summary: keyword.summary.clone(), icon: None });
        let commands = offers.commands.iter().map(|command| Offer { name: command.name.clone(), summary: command.description.clone().unwrap_or_default(), icon: None });
        // Names starting with what's typed first, then those containing it, each by name, as
        // KuvoClient's `SlashCommands.matches` orders them.
        let mut matched: Vec<(bool, Offer)> = keywords
            .chain(commands)
            .filter_map(|offer| {
                let name = offer.name.to_lowercase();
                name.contains(&typed).then(|| (!name.starts_with(&typed), offer))
            })
            .collect();
        matched.sort_by(|(a_later, a), (b_later, b)| a_later.cmp(b_later).then_with(|| a.name.cmp(&b.name)));
        matched.into_iter().map(|(_, offer)| offer).collect()
    }

    /// The mention being typed, if one is, and what the box suggests for it: read as it changes.
    fn read_mentions(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx);
        // A group chat's "&" (its bots, then guests) or a conversation's "@" and "&", by where it goes.
        let (place, typed) = match &self.target {
            Target::Group { id } => (Place::Group(id.clone()), group_mention_query(&text).map(|(_, query)| (For::Agents, query.to_owned()))),
            Target::Session { id, .. } => (Place::Session(id.clone()), mention_query(&text).map(|(wanted, _, query)| (wanted, query.to_owned()))),
            _ => return,
        };
        if typed == self.mentions_for {
            return;
        }
        self.mentions_for = typed.clone();
        self.mention_reads += 1;
        // What was suggested for what was typed before goes: the list waits for the box's answer.
        self.mentions.clear();
        let read = self.mention_reads;
        let Some((wanted, query)) = typed else {
            self.mention_pending = false;
            return;
        };
        // Until the box answers, Return and Tab wait with the list rather than send the mention
        // half typed.
        self.mention_pending = true;
        let client = self.client.clone();
        let ask = move || match &place {
            Place::Group(group) => client.group_mentions(group, &query),
            Place::Session(session) => client.mentions(session, wanted, &query),
        };
        if self.live {
            // Asked once typing pauses a moment, not on every key.
            self.tasks.retain(|task| !task.is_ready());
            self.tasks.push(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(MENTION_PAUSE).await;
                if this.read_with(cx, |this, _| this.mention_reads != read).unwrap_or(true) {
                    return;
                }
                let found = cx.background_executor().spawn(async move { ask() }).await;
                this.update(cx, |this, cx| {
                    if this.mention_reads == read {
                        this.mentions = found.unwrap_or_default();
                        this.mention_pending = false;
                        cx.notify();
                    }
                })
                .ok();
            }));
        } else {
            // (Tests and shots: at once, as the box would answer.)
            self.mentions = ask().unwrap_or_default();
            self.mention_pending = false;
        }
    }

    /// What the list over the field offers now: what "@" or "&" suggests while a mention's being
    /// typed, else "/"'s. Each as it shows: its name, and what's beside it.
    pub fn shown_offers(&self, cx: &App) -> (Vec<Offer>, bool) {
        let text = self.text(cx);
        let typing = match self.target {
            Target::Group { .. } => group_mention_query(&text).is_some(),
            _ => mention_query(&text).is_some(),
        };
        if typing && self.slash_dismissed.as_deref() != Some(text.as_str()) {
            let offers = self.mentions.iter().map(|mention| Offer {
                    name: mention.title.clone(),
                    summary: mention.detail.clone().unwrap_or_default(),
                    icon: Some(match mention.kind.as_str() {
                        "folder" => IconName::Folder,
                        "bot" => IconName::Bot,
                        "session" => IconName::Message,
                        _ => IconName::File,
                    }),
                }).collect();
            return (offers, true);
        }
        let offers = self.slash_offers(cx).into_iter().map(|offer| Offer { name: format!("/{}", offer.name), summary: offer.summary, icon: None }).collect();
        (offers, false)
    }

    /// Puts mention `at` in place of the one being typed, and a space after it; an agent picked is
    /// named when it's sent.
    fn mention_pick(&mut self, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.text(cx);
        let start = match self.target {
            Target::Group { .. } => group_mention_query(&text).map(|(start, _)| start),
            _ => mention_query(&text).map(|(_, start, _)| start),
        };
        let Some(start) = start else { return };
        let Some(mention) = self.mentions.get(at.min(self.mentions.len().saturating_sub(1))).cloned() else { return };
        if let Some(session) = &mention.session {
            self.named.retain(|(text, _)| text != &mention.text);
            self.named.push((mention.text.clone(), session.clone()));
        }
        let joined = format!("{}{} ", &text[..start], mention.text);
        self.field.update(cx, |field, cx| field.set_text(joined, cx));
        window.focus(&self.field.focus_handle(cx), cx);
        cx.notify();
    }

    fn slash_step(&mut self, by: isize, cx: &mut Context<Self>) {
        let count = self.shown_offers(cx).0.len() as isize;
        if count > 0 {
            self.slash_at = (self.slash_at as isize + by).rem_euclid(count) as usize;
            self.slash_scroll.scroll_to_item(self.slash_at);
            cx.notify();
        }
    }

    /// Puts offer `at` in the field, ready for what follows it.
    fn slash_pick(&mut self, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.shown_offers(cx).1 {
            return self.mention_pick(at, window, cx);
        }
        let offers = self.slash_offers(cx);
        let Some(offer) = offers.get(at.min(offers.len().saturating_sub(1))) else { return };
        let text = format!("/{} ", offer.name);
        self.field.update(cx, |field, cx| field.set_text(text, cx));
        window.focus(&self.field.focus_handle(cx), cx);
        cx.notify();
    }

    fn slash_list(&self, offers: &[Offer], cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let c = &theme.colors;
        let rows = offers.iter().enumerate().map(|(ix, offer)| {
            let selected = ix == self.slash_at;
            div()
                .id(("slash", ix))
                .flex()
                .items_center()
                .gap(space::S3)
                .h(px(30.))
                .px(space::S2)
                .rounded(radius::SMALL)
                .cursor_pointer()
                .when(selected, |d| d.bg(c.selected))
                .hover(|s| s.bg(c.hover))
                .on_click(cx.listener(move |this, _, window, cx| this.slash_pick(ix, window, cx)))
                .children(offer.icon.map(|icon| Icon::new(icon).size(IconSize::Nav).color(c.text_muted)))
                .child(div().flex_none().text_ui(TypeSize::Small).medium().text_color(c.text).child(offer.name.clone()))
                .child(div().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text_muted).child(offer.summary.clone()))
        });
        div()
            .id("slash-list")
            .track_scroll(&self.slash_scroll)
            .overflow_y_scroll()
            .max_h(px(30. * SLASH_MOST as f32 + 8.))
            .absolute()
            .bottom_full()
            .left_0()
            .right_0()
            .mb(space::S2)
            .p(space::S1)
            .rounded(radius::POPOVER)
            .bg(c.elevated)
            .border_1()
            .border_color(c.border)
            .shadow(theme.elevated_shadow())
            .children(rows)
            .into_any_element()
    }

    /// A group chat's bots, each a still face that asks it by name.
    fn member_faces(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = cx.theme().colors.clone();
        let held = self.client.gate().is_held();
        self.members
            .iter()
            .map(|member| {
                let name = member.name.clone();
                let selector = format!("ask-{}", member.id);
                let face = crate::groups::chat::face(&member.id, member.look.as_ref(), px(20.), cx);
                div()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector)
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(30.))
                    .h(px(28.))
                    .rounded(radius::PILL)
                    .when(!held, |d| d.cursor_pointer().hover(|s| s.bg(c.hover)).on_click(cx.listener(move |this, _, window, cx| this.ask(&name, window, cx))))
                    .tooltip({
                        let tip = format!("Ask {}", member.name);
                        move |_, cx| kuvo_ui::menu::Tooltip::new(tip.clone()).build(cx)
                    })
                    .child(face)
                    .into_any_element()
            })
            .collect()
    }

    /// What the picker offers here, worked out once for what the box last said.
    /// COMPAT: local side filtering is only for boxes before conversation-scoped offerings;
    /// remove it when those boxes are no longer supported.
    fn catalog(&mut self) -> Arc<Catalog> {
        if let Some(catalog) = &self.catalog {
            return catalog.clone();
        }
        let side = match &self.target {
            Target::Session { place, .. } => Some(Side::of(*place)),
            Target::New(_) | Target::Group { .. } => None,
        };
        let catalog = Arc::new(self.models.offered.as_ref().map(|offered| Catalog::new(offered, |model| self.models.offered.as_ref().is_some_and(|offered| offered.scoped) || side.is_none_or(|side| model.side == side))).unwrap_or_default());
        self.catalog = Some(catalog.clone());
        catalog
    }

    /// The picker, as it opens: what the box offers, the model now marked, your stars.
    fn model_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<ModelPicker> {
        let catalog = self.catalog();
        let starred = self.models.offered.as_ref().and_then(|offered| offered.starred.clone());
        let notice = (!self.can_pick()).then(|| "Finding where this conversation runs…".into());
        // Another side's models (the engine's, or the coding agents') carry the conversation
        // over to a new session there, which this app can't do yet.
        let note = self.models.offered.as_ref().and_then(|offered| offered.note.clone()).map(Into::into).or_else(|| self.left_out().then(|| "This conversation can use models from its own engine or coding agent. Start a new session to use other providers.".into()));
        let (this, kept) = (cx.entity().downgrade(), cx.entity().downgrade());
        let (current, client) = (self.model(), self.client.clone());
        cx.new(|cx| {
            ModelPicker::new(
                catalog,
                starred,
                current,
                Some(client),
                Rc::new(move |picked, _, cx| {
                    if let Some(picked) = picked {
                        this.update(cx, |this, cx| this.pick(picked, cx)).ok();
                    }
                }),
                window,
                cx,
            )
            .notice(notice)
            .note(note)
            .on_starred(Rc::new(move |starred: Vec<Starred>, cx| {
                kept.update(cx, |this, _| {
                    if let Some(offered) = this.models.offered.as_mut() {
                        offered.starred = Some(starred);
                    }
                })
                .ok();
            }))
        })
    }

    /// Opens the model picker (⇧⌘M, and shots).
    pub fn open_model_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<ModelPicker>> {
        if self.chip_label().is_none() || self.client.gate().is_held() || self.model_menu.is_open() {
            return None;
        }
        let picker = self.model_picker(window, cx);
        self.model_menu.open(picker.clone(), window, cx);
        cx.notify();
        Some(picker)
    }

    fn model_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let name = self.chip_label()?;
        let trigger = div().debug_selector(|| "model-chip".into()).child(Button::new("model-chip", name.clone()).disabled(self.client.gate().is_held()).ghost().size(ButtonSize::Small).tooltip(name).tooltip_keys("⇧⌘M"));
        let this = cx.entity().downgrade();
        Some(
            PopoverMenu::view("model-menu", trigger, move |window, cx| this.update(cx, |this, cx| this.model_picker(window, cx)).expect("the composer opens its picker"))
                .with_handle(self.model_menu.clone())
                .above()
                .centered()
                .into_any_element(),
        )
    }

    /// "Auto ⌄": how much the agent does on its own, and the menu of the four; a dot while it does
    /// less than picked for now (Auto without the sandbox), as its box says.
    fn mode_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let shown = self.approval()?;
        let c = cx.theme().colors.clone();
        let holding = shown.holding().cloned();
        let tip: SharedString = match &holding {
            Some(why) => format!("{} asks first for now. {}", shown.approval.title(), why.why).into(),
            None => format!("{}: {}", shown.approval.title(), lowercased_first(shown.approval.detail())).into(),
        };
        let trigger = div()
            .debug_selector(|| "mode-chip".into())
            .relative()
            .child(
                Button::new("mode-chip", shown.approval.title().to_owned())
                    .disabled(self.client.gate().is_held())
                    .ghost()
                    .size(ButtonSize::Small)
                    .icon(approval_icon(&shown.approval))
                    .tooltip(tip),
            )
            .children(holding.is_some().then(|| div().absolute().top(px(5.)).right(px(5.)).size(px(6.)).rounded_full().bg(c.warning)));
        let this = cx.entity().downgrade();
        Some(
            PopoverMenu::new("mode-menu", trigger, move |_, cx| {
                let (shown, this) = (shown.clone(), this.clone());
                cx.new(|cx| {
                    let mut menu = Menu::new(cx).width(px(280.));
                    for approval in Approval::ALL {
                        let choice = shown.choice(&approval);
                        // Not to be had here, as its box says (Full Access unconfirmed still can: it asks first).
                        let refused = choice.and_then(|choice| choice.unavailable.as_ref()).filter(|why| why.reason != "fullAccessOff");
                        let held = if approval == shown.approval { shown.holding() } else { choice.and_then(|choice| choice.held.as_ref()) };
                        let detail = match (refused, held) {
                            (Some(why), _) => why.why.clone(),
                            (None, Some(why)) => format!("Asks first for now. {}", why.why),
                            (None, None) => approval.detail().to_owned(),
                        };
                        let this = this.clone();
                        let picked = approval.clone();
                        menu = menu.entry(
                            MenuEntry::new(approval.title().to_owned())
                                .icon(approval_icon(&approval))
                                .detail(detail)
                                .checked(approval == shown.approval)
                                .disabled(refused.is_some())
                                .on_select(move |_, cx| {
                                    this.update(cx, |this, cx| this.pick_approval(picked.clone(), cx)).ok();
                                }),
                        );
                    }
                    menu
                })
            })
            .above()
            .centered()
            .into_any_element(),
        )
    }

    /// "Local ⌄" or "Worktree ⌄", for a new conversation in a project: whether it works in the
    /// project's folder or in a worktree of its own, on a branch of its own.
    fn worktree_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.offers_worktree() {
            return None;
        }
        let worktree = self.worktree;
        let trigger = div().debug_selector(|| "worktree-chip".into()).child(
            Button::new("worktree-chip", if worktree { "Worktree" } else { "Local" })
                .disabled(self.client.gate().is_held())
                .ghost()
                .size(ButtonSize::Small)
                .icon(if worktree { IconName::Branch } else { IconName::Folder })
                .tooltip("Where the session works: in the project’s folder, or in a worktree of its own"),
        );
        let this = cx.entity().downgrade();
        Some(
            PopoverMenu::new("worktree-menu", trigger, move |_, cx| {
                let this = this.clone();
                cx.new(|cx| {
                    let (local, apart) = (this.clone(), this.clone());
                    Menu::new(cx)
                        .width(px(280.))
                        .entry(MenuEntry::new("Local").icon(IconName::Folder).detail("Works in the project’s folder").checked(!worktree).on_select(move |_, cx| {
                            local.update(cx, |this, cx| this.set_worktree(false, cx)).ok();
                        }))
                        .entry(MenuEntry::new("Worktree").icon(IconName::Branch).detail("Works in a copy on a branch of its own").checked(worktree).on_select(move |_, cx| {
                            apart.update(cx, |this, cx| this.set_worktree(true, cx)).ok();
                        }))
                })
            })
            .above()
            .centered()
            .into_any_element(),
        )
    }

    /// Full Access, the first time it's picked on this computer: asked here, in the composer.
    fn full_access_question(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.confirming_full_access {
            return None;
        }
        let c = cx.theme().colors.clone();
        Some(
            div()
                .debug_selector(|| "full-access-question".into())
                .flex()
                .flex_col()
                .gap(space::S2)
                .p(space::S3)
                .mr(space::S1)
                .rounded(radius::CARD - px(4.))
                .bg(c.sunken)
                .border_1()
                .border_color(c.border)
                .child(div().flex().items_center().gap(space::S2).text_ui(TypeSize::Body).medium().text_color(c.text).child(Icon::new(IconName::Bolt).size(IconSize::Nav).color(c.warning)).child(format!("Allow Full Access on {}?", self.computer)))
                .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child("A conversation in Full Access runs every command without the sandbox and never asks you, so a mistake can reach anything you can: your keys, other projects, your accounts."))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(space::S2)
                        .child(div().debug_selector(|| "full-access-cancel".into()).child(Button::new("full-access-cancel", "Cancel").size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.cancel_full_access(cx)))))
                        .child(div().debug_selector(|| "full-access-allow".into()).child(Button::new("full-access-allow", "Allow Full Access").destructive().size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.confirm_full_access(cx))))),
                )
                .into_any_element(),
        )
    }

    fn effort_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let model = self.model()?;
        let efforts = self.efforts();
        if efforts.is_empty() || self.needs_model() || !self.can_pick() {
            return None;
        }
        let label = model.variant.as_deref().map(effort_title).unwrap_or_else(|| "Default".into());
        let this = cx.entity().downgrade();
        // (Its own pick beside the model's, of only the efforts that model offers.)
        let trigger = div().debug_selector(|| "effort-chip".into()).child(Button::new("effort-chip", label).disabled(self.client.gate().is_held()).ghost().size(ButtonSize::Small).tooltip("Effort: how hard it thinks before it answers"));
        Some(
            PopoverMenu::new("effort-menu", trigger, move |_, cx| {
                let (efforts, model, this) = (efforts.clone(), model.clone(), this.clone());
                cx.new(|cx| {
                    let mut menu = Menu::new(cx).width(px(180.)).header("Effort");
                    for effort in efforts {
                        let checked = model.variant.as_deref() == Some(effort.as_str());
                        let picked = ModelRef { variant: Some(effort.clone()), ..model.clone() };
                        let this = this.clone();
                        menu = menu.entry(MenuEntry::new(effort_title(&effort)).checked(checked).on_select(move |_, cx| {
                            this.update(cx, |this, cx| this.pick(picked.clone(), cx)).ok();
                        }));
                    }
                    menu
                })
            })
            .above()
            .centered()
            .into_any_element(),
        )
    }
}

/// A mode's icon, as the Mac's menu shows it (a hand, a pencil, a shield, a bolt).
fn approval_icon(approval: &Approval) -> IconName {
    match approval {
        Approval::AskFirst => IconName::Hand,
        Approval::AcceptEdits => IconName::Pencil,
        Approval::FullAccess => IconName::Bolt,
        Approval::Auto | Approval::Other(_) => IconName::Shield,
    }
}

fn lowercased_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|first| first.to_lowercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}

/// What a read says of a conversation's model.
enum Said {
    Is(Box<ModelState>),
    /// Left out (its box can't say yet), or not read.
    Unsaid,
}

/// What the box offers in `folder` (kept, for a side it couldn't say now, as it said before; none
/// for a read that failed), whether it could say all of it itself, and what it says of the conversation's
/// model (none for a new one).
fn models_of(client: &BoxClient, target: &Target, picked: Option<&ModelRef>, folder: Option<&str>, before: Option<&Offered>) -> (Option<Offered>, bool, Option<Said>) {
    let session = match target { Target::Session { id, .. } => Some(id.as_str()), _ => None };
    let offered = client.offered_models_for_session(picked, folder, session).ok();
    let said_all = offered.as_ref().is_some_and(Offered::is_whole);
    let offered = offered.map(|offered| offered.keeping(before));
    let state = match target {
        Target::Session { id, .. } => Some(client.session_models(&[id.as_str()]).ok().and_then(|mut all| all.remove(id)).map_or(Said::Unsaid, |state| Said::Is(Box::new(state)))),
        Target::New(_) | Target::Group { .. } => None,
    };
    (offered, said_all, state)
}

/// The mention being typed at the end of `text`: "@" a file (a word with no spaces), or "&" an
/// agent or session (names have spaces, so it runs on to the line's end), each starting a word.
/// Which, where its mark is, and what's typed after it. As the Mac's `Mentions.query` and
/// `AgentReferences.query` read it.
pub fn mention_query(text: &str) -> Option<(For, usize, &str)> {
    let starts_word = |at: usize| at == 0 || text[..at].chars().next_back().is_some_and(char::is_whitespace);
    let file = text.rfind('@').filter(|at| starts_word(*at)).filter(|at| !text[at + 1..].contains(char::is_whitespace));
    let agent = text.rfind('&').filter(|at| starts_word(*at)).filter(|at| {
        let rest = &text[at + 1..];
        !rest.starts_with(char::is_whitespace) && !rest.contains('\n')
    });
    match (file, agent) {
        (Some(file), Some(agent)) if agent > file => Some((For::Agents, agent, &text[agent + 1..])),
        (Some(file), _) => Some((For::Files, file, &text[file + 1..])),
        (None, Some(agent)) => Some((For::Agents, agent, &text[agent + 1..])),
        (None, None) => None,
    }
}

/// The bot being named at the end of `text` in a group chat: "&" (as the chat names its bots), or
/// "@" (as you might write it), starting a word, a name running over spaces to the line's end.
/// Where its mark is, and what's typed after it.
pub fn group_mention_query(text: &str) -> Option<(usize, &str)> {
    let starts_word = |at: usize| at == 0 || text[..at].chars().next_back().is_some_and(char::is_whitespace);
    let at = text.rfind(['&', '@']).filter(|at| starts_word(*at))?;
    let rest = &text[at + 1..];
    (!rest.starts_with(char::is_whitespace) && !rest.contains('\n')).then_some((at, rest))
}

/// Where a mention's suggestions are asked: a group chat, or a conversation.
enum Place {
    Group(String),
    Session(String),
}

/// Whether `name` (as "&" put it in) is still in `text`, as a word of its own.
fn mentioned(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(at, _)| {
        let after = text[at + name.len()..].chars().next();
        after.is_none_or(|c| !c.is_alphanumeric())
    })
}

/// A `file://` path's escapes decoded.
fn percent_decoded(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && let Some(byte) = path.get(at + 1..at + 3).and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            at += 3;
            continue;
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether a file is a picture, shown as one while it waits to go.
fn is_picture(path: &Path) -> bool {
    let extension = path.extension().map(|extension| extension.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "bmp" | "tiff")
}

/// `front` put back ahead of what's written since, as one text.
fn ahead_of(front: &str, now: &str) -> String {
    if now.is_empty() {
        front.to_owned()
    } else if front.is_empty() {
        now.to_owned()
    } else if front.ends_with(char::is_whitespace) {
        format!("{front}{now}")
    } else {
        format!("{front} {now}")
    }
}

/// Whether the box's answer leaves the client a step this one can't take yet: what was written is
/// kept in the field rather than lost.
fn is_clients_to_do(answer: &Answer) -> bool {
    answer.toggle_autopilot == Some(true) || answer.side_thread == Some(true) || answer.show_goal == Some(true)
}

/// What the box did with a typed text, as a line under the composer; and, for what's this
/// client's to do that it can't yet, that plainly (what was written is put back by the box's
/// `restore`, else kept by [`Composer::sent`]).
fn said(answer: &Answer) -> Option<Note> {
    let note = |text: &str, warning: bool| Some(Note { text: text.to_owned().into(), warning });
    if let Some(agent) = &answer.agent {
        let mode = match agent.as_str() {
            "plan" => "Plan".to_owned(),
            "build" => "Build".to_owned(),
            other => other.to_owned(),
        };
        return note(&format!("Switched to {mode}."), false);
    }
    if answer.shell.is_some() {
        return None;
    }
    if answer.compact == Some(true) {
        // (The box's words; a box that sends none shows none: the summary's own row says it.)
        return answer.said.as_deref().and_then(|said| note(said, false));
    }
    if answer.toggle_autopilot == Some(true) {
        return note("Turning Autopilot on or off isn’t in this app yet.", true);
    }
    if answer.autopilot.is_some() {
        return note("Autopilot changed. It isn’t shown in this app yet.", false);
    }
    if answer.side_question.is_some() {
        return note("Asked aside. The answer isn’t shown in this app yet.", false);
    }
    if answer.handoff.is_some() {
        return note("Writing a handoff note. The new session shows in the sidebar once it’s ready.", false);
    }
    if answer.asked_bot == Some(true) {
        return note("Asked the bot for a plan.", false);
    }
    if answer.goal_cleared == Some(true) {
        return note("Goal cleared.", false);
    }
    if answer.side_thread == Some(true) {
        return note("Side threads aren’t in this app yet.", true);
    }
    if answer.show_goal == Some(true) || answer.goal.is_some() {
        return note("Goals aren’t shown in this app yet.", true);
    }
    if answer.wake_up.is_some() {
        return note("Set to come back on its schedule.", false);
    }
    if answer.review.is_some() {
        return note("Review started.", false);
    }
    None
}

impl Composer {
    /// While the agent works: Send (into its run, at its next step), Queue for After This Run, and
    /// Send Now (the step under way stopped for it), as 0.7 offered them.
    fn send_choices(&self, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity();
        let can_queue = self.can_queue();
        // (Kept in place while one's on its way, so Send doesn't move.)
        let trigger = Button::icon_only("send-choices", IconName::ChevronDown, "More ways to send").ghost().size(ButtonSize::Small).disabled(self.sending);
        div()
            .debug_selector(|| "send-choices".into())
            .child(PopoverMenu::new("send-choices-menu", trigger, move |_, cx| Self::send_menu(this.clone(), can_queue, cx)).with_handle(self.send_menu.clone()).align_right().above().centered())
            .into_any_element()
    }

    fn send_menu(this: Entity<Composer>, can_queue: bool, cx: &mut App) -> Entity<Menu> {
        let (steer, queue, now) = (this.clone(), this.clone(), this);
        cx.new(|cx| {
            Menu::new(cx)
                .width(px(400.))
                .entry(MenuEntry::new("Send").detail("At its next step").shortcut("↩").on_select(move |_, cx| steer.update(cx, |this, cx| this.send_as(SendAs::Steer, cx))))
                .entry(MenuEntry::new("Queue for After This Run").detail("Once this run ends").shortcut("⌘↩").disabled(!can_queue).on_select(move |_, cx| queue.update(cx, |this, cx| this.send_as(SendAs::Queue, cx))))
                .entry(MenuEntry::new("Send Now").detail("Stops its current step").on_select(move |_, cx| now.update(cx, |this, cx| this.send_as(SendAs::Now, cx))))
        })
    }

    /// Opens the menu beside Send (for shots).
    pub fn open_send_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let menu = Self::send_menu(cx.entity(), self.can_queue(), cx);
        self.send_menu.open(menu, window, cx);
        cx.notify();
    }
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.item_editor {
            return div().id("item-composer").debug_selector(|| "item-composer".into()).w_full().flex().flex_col().gap(space::S2)
                .child(self.field.clone())
                .children(self.note.as_ref().map(|note| div().child(note.text.clone())))
                .when(self.item_attaching, |editor| editor.on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.attach_named(paths.paths().to_vec(), cx))))
                .into_any_element();
        }
        let held = self.client.gate().is_held();
        if self.field_held != held {
            self.field_held = held;
            let placeholder: SharedString = if held {
                "Can’t send until Kuvo is updated.".into()
            } else {
                self.placeholder.clone()
            };
            self.field.update(cx, |field, cx| {
                field.set_placeholder(placeholder);
                field.set_disabled(held, cx);
            });
        }
        // (0.7's words while the agent works: what's sent goes into its task.)
        if !held {
            let wanted: SharedString = if self.running && matches!(self.target, Target::Session { .. }) { WHILE_RUNNING.into() } else { self.placeholder.clone() };
            if self.field.read(cx).placeholder_text() != wanted.as_ref() {
                self.field.update(cx, |field, _| field.set_placeholder(wanted));
            }
        }
        let theme = cx.theme().clone();
        let c = &theme.colors;
        let text = self.text(cx);
        let empty = text.trim().is_empty() && self.attached.is_empty();
        let (offers, _) = self.shown_offers(cx);
        self.slash_at = self.slash_at.min(offers.len().saturating_sub(1));
        // The keyword it starts with, as the box will read it (and as the sent message shows it).
        let keyword = typed::keyword(&text).map(|(keyword, _)| Chip::new("typed-keyword", format!("/{keyword}")).icon(IconName::Sparkle).tone(ChipTone::Accent));
        let group = matches!(self.target, Target::Group { .. });
        let stop = self.running && empty && matches!(self.target, Target::Session { .. } | Target::Group { .. });
        let send = if stop {
            Button::icon_only("stop", IconName::Stop, "Stop").style(kuvo_ui::ButtonStyle::Secondary).pill().on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
        } else {
            // While the agent works, 0.7's words: Return steers it in, ⌘Return waits for the run.
            let steers = self.running && !empty && matches!(self.target, Target::Session { .. });
            Button::icon_only("send", IconName::ArrowUp, if empty { "Type to send" } else { "Send" })
                .accent()
                .pill()
                .when(steers, |button| button.tooltip("Send: the agent reads it at its next step (Return). ⌘Return queues it for after this run."))
                .disabled(empty || self.sending || self.client.gate().is_held())
                .loading(self.sending)
                .on_click(cx.listener(|this, _, _, cx| this.send(cx)))
        };
        // The other ways to send while the agent works, beside Send.
        let choices = (self.running && !empty && matches!(self.target, Target::Session { .. })).then(|| self.send_choices(cx));
        let note = self.note.clone().map(|note| {
            div()
                .px(space::S2)
                .pt(space::S2)
                .text_ui(TypeSize::Caption)
                .text_color(if note.warning { c.warning } else { c.text_muted })
                .child(note.text)
        });
        // The list's keys while it shows, and while a mention's suggestions are on their way.
        let waiting = self.mention_pending && !group && mention_query(&text).is_some();
        let context = if offers.is_empty() && !waiting { CONTEXT.to_owned() } else { format!("{CONTEXT} {SLASH}") };
        let card = div()
            .relative()
            .flex()
            .flex_col()
            .gap(space::S2)
            .p(space::S3)
            .pl(space::S4)
            .rounded(radius::CARD)
            .bg(c.surface)
            .border_1()
            .border_color(c.border_strong)
            .shadow(theme.card_shadow())
            .when(!offers.is_empty(), |d| d.child(self.slash_list(&offers, cx)))
            .children(self.full_access_question(cx))
            .children(self.quote_cards(window, cx))
            .children(self.attachments(cx))
            .children(keyword.map(|chip| div().flex().child(chip)))
            .child(div().pt(space::S1).pr(space::S1).child(self.field.clone()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S1)
                    .ml(-space::S2)
                    .when(!group, |d| d.child(Button::icon_only("attach", IconName::Paperclip, "Attach Files").ghost().size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.pick_files(cx)))))
                    .when(group, |d| d.children(self.member_faces(cx)))
                    .children(self.mode_chip(cx))
                    .children(self.model_chip(cx))
                    .children(self.effort_chip(cx))
                    .children(self.worktree_chip(cx))
                    .child(div().flex_1())
                    .children(choices)
                    .child(send),
            );
        div()
            .id("composer")
            .key_context(context.as_str())
            .on_action(cx.listener(|this, _: &SendQueued, _, cx| this.send_as(SendAs::Queue, cx)))
            .on_action(cx.listener(|this, _: &ChooseModel, window, cx| {
                this.open_model_picker(window, cx);
            }))
            .on_action(cx.listener(|this, _: &SlashNext, _, cx| this.slash_step(1, cx)))
            .on_action(cx.listener(|this, _: &SlashPrevious, _, cx| this.slash_step(-1, cx)))
            .on_action(cx.listener(|this, _: &SlashPick, window, cx| this.slash_pick(this.slash_at, window, cx)))
            .on_action(cx.listener(|this, _: &SlashDismiss, _, cx| {
                this.slash_dismissed = Some(this.text(cx));
                cx.notify();
            }))
            .when(!group, |d| d.on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.attach_named(paths.paths().to_vec(), cx))))
            .w_full()
            .flex()
            .flex_col()
            .child(div().debug_selector(|| "composer-card".into()).child(card))
            .children(note)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A summary asked for says so in the box's words, and nothing where the box sent none.
    #[test]
    fn a_summary_asked_for_is_said_in_the_boxs_words() {
        let answer: Answer = serde_json::from_value(serde_json::json!({"keyword": "compact", "compact": true, "said": "Asked to summarize the conversation."})).unwrap();
        assert_eq!(said(&answer).map(|note| (note.text.to_string(), note.warning)), Some(("Asked to summarize the conversation.".to_owned(), false)));
        let silent: Answer = serde_json::from_value(serde_json::json!({"keyword": "compact", "compact": true})).unwrap();
        assert!(said(&silent).is_none());
    }

    #[test]
    fn executed_composer_actions_are_not_restored_as_unsupported() {
        for value in [serde_json::json!({"shell": "pwd"}), serde_json::json!({"compact": true})] {
            let answer: Answer = serde_json::from_value(value).unwrap();
            assert!(!is_clients_to_do(&answer));
            assert!(!answer.takes_files());
            assert!(said(&answer).is_none_or(|note| !note.warning));
        }
    }

    #[test]
    fn unsupported_composer_actions_stay_clients_to_do() {
        for value in [
            serde_json::json!({"toggleAutopilot": true}),
            serde_json::json!({"sideThread": true}),
            serde_json::json!({"showGoal": true}),
        ] {
            let answer: Answer = serde_json::from_value(value).unwrap();
            assert!(is_clients_to_do(&answer));
            assert!(!answer.takes_files());
            assert!(said(&answer).is_some_and(|note| note.warning));
        }
    }

    #[test]
    fn the_mention_being_typed_is_read_from_the_end() {
        assert_eq!(mention_query("look at @sche").map(|(w, at, q)| (w, at, q.to_owned())), Some((For::Files, 8, "sche".into())));
        assert!(mention_query("look at @schema.ts now").is_none(), "a file's word ended");
        assert!(mention_query("me@example").is_none(), "not a word's start");
        // An agent's name runs over spaces, to the line's end.
        assert_eq!(mention_query("ask &Rate lim").map(|(w, _, q)| (w, q.to_owned())), Some((For::Agents, "Rate lim".into())));
        assert!(mention_query("& then").is_none());
        assert_eq!(mention_query("@a.ts and &Mi").map(|(w, _, _)| w), Some(For::Agents), "the later one");
    }

    #[test]
    fn an_agent_picked_is_named_while_its_name_stays() {
        assert!(mentioned("ask &Mira about it", "&Mira"));
        assert!(!mentioned("ask &Miranda", "&Mira"));
        assert!(!mentioned("ask her", "&Mira"));
    }
}
