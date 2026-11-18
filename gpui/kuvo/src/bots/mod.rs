//! The Bots area's main view: a bot's page (who it is, what it's doing now, its recent sessions,
//! and its settings in Settings' row style), New Bot, and deleting one, asked first. Everything on
//! it is the box's: the list and what each bot is doing (`kuvo/bots`), its sessions (`kuvo/work`),
//! what New Bot offers (`kuvo/bots/templates`), the models offered. Every change goes to the
//! box's route (`POST kuvo/bots`, `PATCH` and `DELETE kuvo/bots/<id>`) and what it answers is what's
//! drawn after: the bot as it is now, an edit it holds for another computer, or why not, in its
//! words. The app decides nothing but what it draws.
//!
//! Drawn as Settings' pages are: the page's items worked out once each time what they're drawn from
//! changes, then a virtual list that lays out only what's on screen.

mod page;

pub use page::dim;

use std::collections::HashMap;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, ListAlignment, ListState, Subscription, Task, px};
use kuvo_box::BoxClient;
use kuvo_box::accounts::said;
use kuvo_box::areas::{Bot, Look};
use kuvo_box::areas::DeleteLine;
use kuvo_box::bots::{BotChange, BotsSaid, Elsewhere, Held, NewBot, Outcome, Personality, Refused, Templates, Work, WorkSession, WorkState};
use kuvo_box::models::effort_title;
use kuvo_box::models::Offered;
use kuvo_box::typed::ModelRef;
use kuvo_ui::{TextField, TextFieldEvent};

use crate::areas::{Reading, reading};
use crate::time;

/// How many of a bot's sessions its page lists before "Show All".
const RECENT: usize = 6;

/// What the main view shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    /// Nothing picked.
    Nothing,
    /// One of the box's bots, by its id.
    Bot(String),
    /// A bot that lives on another of your computers, as the box says that computer last told it
    /// (or, for one none lists, by what's held for it or was refused).
    Elsewhere(String),
    /// New Bot.
    New,
}

/// What the view asks of the window around it.
#[derive(Clone, Debug, PartialEq)]
pub enum BotsEvent {
    /// Open a bot's own conversation, with `draft` in its composer where the box gave one.
    Chat { bot: String, session: String, draft: Option<String> },
    /// Open one of its sessions.
    Session(String),
    /// Pick this row in the sidebar (none: nothing).
    Select(Option<String>),
    /// The box's bots changed: its list is to be read again.
    Changed,
}

/// What a control asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Chat,
    OpenSession(String),
    ShowAll,
    Rename,
    EditLook,
    EditPersonality,
    SetPersonality(Option<String>),
    SetModel(ModelRef),
    /// The effort its model runs at; none: the model's own.
    SetEffort(Option<String>),
    Delete,
    New,
    PickTemplate(Option<String>),
    FormLook,
    FormPersonality(Option<String>),
    FormModel(Option<ModelRef>),
    FormProject(Option<String>),
    Create,
    CancelNew,
}

/// A sheet over the page.
#[derive(Clone, Debug, PartialEq)]
pub enum Sheet {
    Personality(Personality),
    /// The look being picked, and whether it's the new bot's.
    Look { look: Look, new: bool },
    Delete,
}

/// A row's control, at its right.
#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    Text(String),
    Delete,
    Busy(String),
    MoveTo { bot: String, computer: String, source: Option<crate::moves::RemoteSource> },
    /// A bot's face beside a button.
    Face(Option<Look>, String, Act),
    /// What's chosen, and the choices: each one's name, a line under it, what it does, whether it's on.
    Menu { shown: String, choices: Vec<(String, String, Act, bool)>, personality: bool },
    /// A model, from the model picker: what's shown, the model now, and whether it's New Bot's
    /// (whose first choice leaves it to the box: what `unset` says).
    Model { shown: String, current: Option<ModelRef>, new: Option<(String, String)> },
    /// The new bot's name, typed in the row.
    NameField,
}

/// A settings row: its name, a line under it, and its control.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub key: String,
    pub title: String,
    pub detail: String,
    pub control: Control,
}

/// How a notice reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Waiting,
    Refused,
}

/// A session's line on its bot's page: its project and how it stands, with where "Waiting on
/// you" is in it (the only part coloured).
pub(crate) fn session_line(project: &str, state: WorkState) -> (String, Option<std::ops::Range<usize>>) {
    let mut line = project.to_owned();
    let Some(words) = state_words(state) else { return (line, None) };
    if !line.is_empty() {
        line.push_str(" · ");
    }
    let start = line.len();
    line.push_str(words);
    (line, (state == WorkState::Asking).then(|| start..start + words.len()))
}

/// One of the page list's items.
#[derive(Clone, Debug, PartialEq)]
enum Item {
    Title(String),
    /// The bot’s avatar and profile fields.
    Head(Box<Bot>),
    /// A bot that lives elsewhere: what's known of it here.
    ElsewhereHead { key: String, name: String, look: Option<Look>, line: String, editable: bool },
    Notice { key: String, tone: Tone, title: String, words: String },
    Group(String),
    Row { row: Box<Row>, first: bool, last: bool },
    Session { session: WorkSession, project: String, ago: String, first: bool, last: bool },
    More { count: usize },
    Note { key: String, words: String, busy: bool },
    /// New Bot's templates, and which is picked.
    Templates { picked: Option<String> },
    /// New Bot's foot: why it wasn't made, Cancel and Create.
    Foot { error: Option<String>, creating: bool },
    Empty { none: bool },
    End,
}

/// What New Bot is being filled in with. What's left unpicked is the box's to choose.
#[derive(Clone, Debug, PartialEq, Default)]
struct Form {
    template: Option<String>,
    look: Option<Look>,
    /// `Some(None)`: the model's own way, picked.
    personality: Option<Option<String>>,
    model: Option<ModelRef>,
    project: Option<String>,
    /// This New Bot's own id: a retry sends it again, so the bot is made once.
    request: String,
    creating: bool,
    error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProfileField {
    Name,
    Job,
    Instructions,
}

impl ProfileField {
    const ALL: [Self; 3] = [Self::Name, Self::Job, Self::Instructions];
}

struct FieldSave {
    text: String,
    generation: u64,
}

#[derive(Default)]
struct FieldDraft {
    saved: Option<String>,
    text: String,
    generation: u64,
    clean_generation: u64,
    queued: Option<FieldSave>,
    pending: Option<FieldSave>,
    error: Option<FieldError>,
}

struct FieldError {
    generation: u64,
    message: String,
}

type ProfileDraft = [FieldDraft; 3];

/// The Bots area's main view.
pub struct BotsView {
    client: BoxClient,
    moves: Option<Entity<crate::moves::MovesView>>,
    live: bool,
    said: Reading<BotsSaid>,
    shown: Shown,
    /// The sessions of the bot shown.
    work: Reading<Work>,
    all_sessions: bool,
    templates: Reading<Templates>,
    offered: Reading<Offered>,
    /// Rows with a change on its way, and what they say meanwhile.
    busy: Vec<(String, String)>,
    /// What the box last refused of a row's change, over the page until closed.
    problem: Option<String>,
    editor: Option<Sheet>,
    /// The editor’s change is on its way; and why the box turned the last one down.
    saving: bool,
    edit_error: Option<String>,
    /// Drafts outlive the shared text fields when the user switches bots.
    profiles: HashMap<String, ProfileDraft>,
    profile_shown: Option<String>,
    watching_profile_focus: bool,
    name: Entity<TextField>,
    line: Entity<TextField>,
    details: Entity<TextField>,
    personality_name: Entity<TextField>,
    personality_description: Entity<TextField>,
    new_name: Entity<TextField>,
    form: Form,
    /// What takes the keyboard as it's next drawn.
    wants_focus: Option<FocusHandle>,
    sheet_focus: FocusHandle,
    this_box: String,
    now: Option<f64>,
    items: Vec<Item>,
    loading_control: Option<String>,
    list: ListState,
    focus: FocusHandle,
    tasks: HashMap<String, Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<BotsEvent> for BotsView {}

impl Focusable for BotsView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl BotsView {
    fn move_source(&self, id: &str) -> Option<crate::moves::RemoteSource> {
        let Reading::Ready(said) = &self.said else { return None };
        crate::moves::RemoteSource::of(said.elsewhere.iter().find(|bot| bot.id == id)?)
    }

    pub fn set_moves(&mut self, moves: Entity<crate::moves::MovesView>, cx: &mut Context<Self>) {
        self._subscriptions.push(cx.observe(&moves, |_, _, cx| cx.notify()));
        self.moves = Some(moves);
    }

    pub fn new(client: BoxClient, live: bool, cx: &mut Context<Self>) -> BotsView {
        let name = cx.new(|cx| TextField::new(cx).bare().tab_stop().placeholder("Name"));
        let line = cx.new(|cx| TextField::new(cx).bare().tab_stop().placeholder("Describe what your bot does"));
        let details = cx.new(|cx| {
            TextField::new(cx)
                .bare()
                .tab_stop()
                .multi_line(2, 10)
                .submits_on_enter(false)
                .placeholder("What to do, when to do it, and what to report")
        });
        let personality_name = cx.new(|cx| TextField::new(cx).tab_stop().placeholder("Name"));
        let personality_description = cx.new(|cx| TextField::new(cx).tab_stop().multi_line(4, 10).placeholder("Description"));
        let new_name = cx.new(|cx| TextField::new(cx).placeholder("Name"));
        let subscriptions = vec![
            cx.subscribe(&name, |this: &mut BotsView, _, event: &TextFieldEvent, cx| this.profile_key(ProfileField::Name, event, cx)),
            cx.subscribe(&line, |this: &mut BotsView, _, event: &TextFieldEvent, cx| this.profile_key(ProfileField::Job, event, cx)),
            cx.subscribe(&details, |this: &mut BotsView, _, event: &TextFieldEvent, cx| this.profile_key(ProfileField::Instructions, event, cx)),
            cx.subscribe(&personality_name, |this: &mut BotsView, _, event: &TextFieldEvent, cx| this.editor_key(event, cx)),
            cx.subscribe(&personality_description, |this: &mut BotsView, _, event: &TextFieldEvent, cx| this.editor_key(event, cx)),
            cx.subscribe(&new_name, |this: &mut BotsView, _, event: &TextFieldEvent, cx| match event {
                TextFieldEvent::Submit(_) => this.act(Act::Create, cx),
                TextFieldEvent::Cancel => this.act(Act::CancelNew, cx),
                TextFieldEvent::Changed => {}
            }),
        ];
        let mut view = BotsView {
            client,
            moves: None,
            live,
            said: Reading::NotYet,
            shown: Shown::Nothing,
            work: Reading::NotYet,
            all_sessions: false,
            templates: Reading::NotYet,
            offered: Reading::NotYet,
            busy: Vec::new(),
            problem: None,
            editor: None,
            saving: false,
            edit_error: None,
            profiles: HashMap::new(),
            profile_shown: None,
            watching_profile_focus: false,
            name,
            line,
            details,
            personality_name,
            personality_description,
            new_name,
            form: Form::default(),
            wants_focus: None,
            sheet_focus: cx.focus_handle(),
            this_box: "This Mac".into(),
            now: None,
            items: Vec::new(),
            loading_control: None,
            list: ListState::new(0, ListAlignment::Top, px(400.)),
            focus: cx.focus_handle(),
            tasks: HashMap::new(),
            _subscriptions: subscriptions,
        };
        view.rebuild(cx);
        let sized = kuvo_ui::theme::remeasure_on_text_size(&view.list, cx);
        view._subscriptions.push(sized);
        view
    }

    pub fn shown(&self) -> &Shown {
        &self.shown
    }

    /// The sheet open, if one is.
    pub fn sheet(&self) -> Option<&Sheet> {
        self.editor.as_ref()
    }

    /// The field a name is typed in (Rename's), a job's line and its details, and New Bot's name.
    pub fn name_field(&self) -> &Entity<TextField> {
        &self.name
    }

    pub fn job_fields(&self) -> (&Entity<TextField>, &Entity<TextField>) {
        (&self.line, &self.details)
    }

    pub fn new_name_field(&self) -> &Entity<TextField> {
        &self.new_name
    }

    pub fn set_now(&mut self, now: f64) {
        self.now = Some(now);
    }

    pub fn set_this_box(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.this_box != name {
            self.this_box = name.to_owned();
            self.rebuild(cx);
        }
    }

    fn now(&self) -> f64 {
        self.now.unwrap_or_else(time::now_ms)
    }

    /// The box's bots, as the sidebar read them.
    pub fn set_said(&mut self, said: Reading<BotsSaid>, cx: &mut Context<Self>) {
        if self.said != said {
            self.said = said;
            // A bot that disappeared from the list can no longer be edited.
            if matches!(self.shown, Shown::Bot(_) | Shown::Elsewhere(_)) && self.subject().is_none() && self.editor.is_some() && !self.saving {
                self.editor = None;
                self.edit_error = None;
                self.wants_focus = Some(self.focus.clone());
            }
            self.rebuild(cx);
        }
    }

    /// Shows what the sidebar's row `key` names (none: nothing picked), reading what it's drawn from.
    pub fn show(&mut self, key: Option<&str>, cx: &mut Context<Self>) {
        let shown = match key.and_then(|key| key.split_once(':')) {
            Some(("bot", id)) => Shown::Bot(id.to_owned()),
            Some(("elsewhere", id)) => Shown::Elsewhere(id.to_owned()),
            _ if key == Some("new") => Shown::New,
            _ => Shown::Nothing,
        };
        if self.shown != shown {
            self.queue_profile(&ProfileField::ALL, false, cx);
            self.profile_shown = None;
            if let Some(editor) = &self.editor {
                self.disable_editor_fields(editor, false, cx);
                self.wants_focus = None;
            }
            self.shown = shown;
            self.problem = None;
            self.editor = None;
            self.edit_error = None;
            // (A change still on its way answers for its own bot, not for what's shown now.)
            self.saving = false;
            self.all_sessions = false;
            self.work = Reading::NotYet;
            self.list.scroll_to(gpui::ListOffset::default());
            // New Bot starts empty, unless one's still being made: that form stays, so a second
            // Create can't make another.
            if self.shown == Shown::New && !self.form.creating {
                self.start_form(cx);
            }
        }
        self.read(cx);
        self.rebuild(cx);
    }

    /// Reads what the page shown is drawn from (again, each time it's shown).
    pub fn read(&mut self, cx: &mut Context<Self>) {
        match self.shown.clone() {
            Shown::Bot(bot) => {
                let asked = bot.clone();
                self.run("read-work", move |client| client.bot_work(&asked), move |this, read, _| {
                    if this.shown == Shown::Bot(bot) {
                        this.work = reading(read, &this.work);
                    }
                }, cx);
                self.read_offers(cx);
            }
            // (A bot elsewhere: the looks a bot can have, for its Look sheet.)
            Shown::New | Shown::Elsewhere(_) => self.read_offers(cx),
            Shown::Nothing => {}
        }
    }

    /// What a bot can be here: its looks and personalities, and the models offered.
    fn read_offers(&mut self, cx: &mut Context<Self>) {
        self.run("read-templates", |client| client.bot_templates(), |this, read, _| this.templates = reading(read, &this.templates), cx);
        self.run("read-offered", |client| client.offered_models(None, None), |this, read, _| this.offered = reading(read, &this.offered), cx);
    }

    /// Runs `work` against the box, in the app off this thread, in tests and shots at once, and
    /// hands what it came to to `then`. A newer one under the same name replaces an older.
    fn run<T: Send + 'static>(&mut self, name: impl Into<String>, work: impl FnOnce(&BoxClient) -> T + Send + 'static, then: impl FnOnce(&mut BotsView, T, &mut Context<BotsView>) + 'static, cx: &mut Context<Self>) {
        let client = self.client.clone();
        if !self.live {
            let done = work(&client);
            then(self, done, cx);
            self.rebuild(cx);
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            let done = cx.background_executor().spawn(async move { work(&client) }).await;
            this.update(cx, |this, cx| {
                then(this, done, cx);
                this.rebuild(cx);
            })
            .ok();
        });
        self.tasks.insert(name.into(), task);
    }

    // MARK: What's shown

    fn bots(&self) -> Option<&BotsSaid> {
        match &self.said {
            Reading::Ready(said) => Some(said),
            _ => None,
        }
    }

    /// The bot shown, as the box lists it.
    pub fn bot(&self) -> Option<&Bot> {
        let Shown::Bot(id) = &self.shown else { return None };
        self.bots()?.bots.iter().find(|bot| &bot.id == id)
    }

    /// The bot shown that lives on another computer, as it's drawn.
    fn there(&self, id: &str) -> Option<There> {
        self.bots()?.elsewhere.iter().find(|bot| bot.id == id).map(There::of)
    }

    /// The bot shown that lives on another computer, where one of your computers lists it now.
    fn listed_there(&self, id: &str) -> Option<&Elsewhere> {
        self.bots()?.elsewhere.iter().find(|bot| bot.id == id && !bot.unlisted)
    }

    /// The creature the page's faces are: the bot shown (the same as its other copies), else New
    /// Bot's own.
    fn face_key(&self) -> String {
        self.subject().map(|(id, ..)| id).unwrap_or_else(|| "new-bot".to_owned())
    }

    /// The bot a sheet is about: the one shown, here or on another computer, where the box lists
    /// it. Its id, its name and its look.
    fn subject(&self) -> Option<(String, String, Option<Look>)> {
        match &self.shown {
            Shown::Bot(_) => self.bot().map(|bot| (bot.id.clone(), bot.name.clone(), Some(bot.look.clone()))),
            Shown::Elsewhere(id) => self.listed_there(id).map(|bot| (bot.id.clone(), There::of(bot).name, bot.look.clone())),
            Shown::Nothing | Shown::New => None,
        }
    }

    /// What deleting the bot shown takes with it, in the box's words: what its confirm shows.
    pub fn deletes(&self) -> Vec<DeleteLine> {
        match &self.shown {
            Shown::Bot(_) => self.bot().map(|bot| bot.deletes.clone()),
            Shown::Elsewhere(id) => self.listed_there(id).map(|bot| bot.deletes.clone()),
            Shown::Nothing | Shown::New => None,
        }
        .unwrap_or_default()
    }

    fn held(&self, bot: &str) -> Option<&Held> {
        self.bots()?.held.iter().find(|held| held.bot == bot)
    }

    fn refused(&self, bot: &str) -> Vec<&Refused> {
        self.bots().map(|said| said.refused.iter().filter(|refused| refused.bot == bot).collect()).unwrap_or_default()
    }

    fn templates(&self) -> Option<&Templates> {
        match &self.templates {
            Reading::Ready(templates) => Some(templates),
            _ => None,
        }
    }

    /// The models offered, as the box last said them (the model picker's).
    pub(crate) fn offered_models(&self) -> Option<&Offered> {
        self.offered()
    }

    pub(crate) fn client(&self) -> &BoxClient {
        &self.client
    }

    /// Your stars as a picker changed them, kept for the next one opened.
    pub(crate) fn keep_starred(&mut self, starred: Vec<kuvo_box::models::Starred>) {
        if let Reading::Ready(offered) = &mut self.offered {
            offered.starred = Some(starred);
        }
    }

    fn offered(&self) -> Option<&Offered> {
        match &self.offered {
            Reading::Ready(offered) => Some(offered),
            _ => None,
        }
    }

    /// What `bot`'s row `key` says while its change is on its way.
    fn busy(&self, bot: &str, key: &str) -> Option<&str> {
        let key = format!("{bot}/{key}");
        self.busy.iter().find(|(busy, _)| *busy == key).map(|(_, words)| words.as_str())
    }

    /// A model’s name, as the list of models offered names it.
    fn model_name(&self, model: &ModelRef) -> Option<String> {
        self.offered()?.models.iter().find(|each| each.is(model)).map(|each| each.name.clone())
    }

    /// A project's name, as New Bot's list names it (else its folder's).
    fn project_name(&self, directory: &str) -> String {
        self.templates()
            .and_then(|templates| templates.projects.iter().find(|project| project.directory == directory))
            .map(|project| project.name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| directory.trim_end_matches('/').rsplit('/').next().unwrap_or(directory).to_owned())
    }

    /// Works the page's items out again, from what it's drawn from now.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.sync_profile(cx);
        let items = match self.shown.clone() {
            Shown::Nothing => self.nothing_items(),
            Shown::Bot(id) => self.bot_items(&id),
            Shown::Elsewhere(id) => self.elsewhere_items(&id),
            Shown::New => self.new_items(),
        };
        self.loading_control = if items.iter().any(|item| matches!(item, Item::Note { busy: true, .. })) {
            None
        } else {
            items.iter().find_map(|item| match item {
                Item::Row { row, .. } if matches!(&row.control, Control::Busy(words) if words.starts_with("Loading")) => Some(row.key.clone()),
                _ => None,
            })
        };
        // Only the items that changed are laid out again; the rest keep their places and the
        // page keeps its scroll.
        if items != self.items {
            let start = self.items.iter().zip(&items).take_while(|(old, new)| old == new).count();
            let tail = self.items[start..].iter().rev().zip(items[start..].iter().rev()).take_while(|(old, new)| old == new).count();
            self.list.splice(start..self.items.len() - tail, items.len() - tail - start);
            self.items = items;
        }
        let ready: Vec<_> = self.profiles.iter()
            .filter(|(id, fields)| !self.change_pending(id) && fields.iter().any(|field| field.queued.is_some()))
            .map(|(id, _)| id.clone()).collect();
        if !ready.is_empty() {
            let view = cx.weak_entity();
            cx.defer(move |cx| {
                view.update(cx, |this, cx| {
                    for id in ready {
                        this.drain_profile(&id, cx);
                    }
                }).ok();
            });
        }
        cx.notify();
    }

    fn nothing_items(&self) -> Vec<Item> {
        match &self.said {
            Reading::NotYet => vec![Item::Note { key: "bots-loading".into(), words: "Loading bots…".into(), busy: true }, Item::End],
            Reading::Failed(_) => vec![Item::Note { key: "bots-failed".into(), words: "Couldn’t load bots. Reopen Bots to try again.".into(), busy: false }, Item::End],
            Reading::Ready(said) => vec![Item::Empty { none: said.bots.is_empty() }, Item::End],
        }
    }

    fn bot_items(&self, id: &str) -> Vec<Item> {
        let Some(bot) = self.bot() else {
            return match &self.said {
                Reading::NotYet => vec![Item::Note { key: "bots-loading".into(), words: "Loading bots…".into(), busy: true }, Item::End],
                Reading::Failed(_) => vec![Item::Note { key: "bots-failed".into(), words: "Couldn’t load bots. Reopen Bots to try again.".into(), busy: false }, Item::End],
                // Gone from the box's list since it was picked (deleted or moved, from anywhere).
                Reading::Ready(_) => vec![Item::Note { key: "bot-gone".into(), words: format!("This bot isn’t on {} any more.", self.this_box), busy: false }, Item::End],
            };
        };
        let mut items = vec![Item::Head(Box::new(bot.clone()))];
        items.extend(self.notices(id));

        items.push(Item::Group("Brain".into()));
        let rows = self.setting_rows(bot);
        let count = rows.len();
        items.extend(rows.into_iter().enumerate().map(|(ix, row)| Item::Row { row: Box::new(row), first: ix == 0, last: ix + 1 == count }));

        items.push(Item::Group("Where".into()));
        items.push(Item::Row { row: Box::new(Self::computer_row(id, bot.machine.as_deref().filter(|name| !name.trim().is_empty()).unwrap_or(&self.this_box), None, true)), first: true, last: false });
        let folder = bot.project.as_deref().map(|project| self.project_name(project)).unwrap_or_else(|| "A folder of its own".into());
        let works_in = Row { key: "bot-works".into(), title: "Works in".into(), detail: String::new(), control: Control::Text(folder) };
        items.push(Item::Row { row: Box::new(works_in), first: false, last: true });

        items.push(Item::Group("Recent sessions".into()));
        match &self.work {
            Reading::NotYet => items.push(Item::Note { key: "work-loading".into(), words: "Loading its sessions…".into(), busy: true }),
            Reading::Failed(_) => items.push(Item::Note { key: "work-failed".into(), words: "Couldn’t load sessions. Reopen this page to try again.".into(), busy: false }),
            Reading::Ready(work) if work.sessions.is_empty() => items.push(Item::Note { key: "work-none".into(), words: "It hasn’t started a session yet.".into(), busy: false }),
            Reading::Ready(work) => {
                let count = work.sessions.len();
                let listed = if self.all_sessions { count } else { count.min(RECENT) };
                let more = count > listed;
                for (index, session) in work.sessions.iter().take(listed).enumerate() {
                    items.push(Item::Session {
                        session: session.clone(),
                        project: session.project.as_deref().map(|project| self.project_name(project)).unwrap_or_default(),
                        ago: session.started.map(|started| time::ago(started, self.now())).unwrap_or_default(),
                        first: index == 0,
                        last: index + 1 == listed && !more,
                    });
                }
                if more {
                    items.push(Item::More { count });
                }
            }
        }

        items.push(Item::Row { row: Box::new(delete_row()), first: true, last: true });
        items.push(Item::End);
        items
    }

    /// What's held for `bot` on another computer, and what one refused of it, as the box says them.
    fn notices(&self, bot: &str) -> Vec<Item> {
        let mut items = Vec::new();
        if let Some(held) = self.held(bot) {
            // Why it waits though its computer answers, where the box says so, as it says it.
            let words = if let Some(waits) = &held.waits {
                waits.clone()
            } else if held.delete {
                "It’s deleted once that computer is back.".to_owned()
            } else {
                // (Each field by the box's own name for it: name, look, job, personality, model.)
                let fields = held.fields();
                match fields.as_slice() {
                    [] => "Your changes go to it once that computer is back.".to_owned(),
                    fields => format!("Your changes to its {} go to it once that computer is back.", listed(fields)),
                }
            };
            items.push(Item::Notice { key: "bot-held".into(), tone: Tone::Waiting, title: format!("Waiting for {}", computer_name(&held.computer)), words });
        }
        for (ix, refused) in self.refused(bot).into_iter().enumerate() {
            // The box's own words for why, as they are.
            items.push(Item::Notice { key: format!("bot-refused-{ix}"), tone: Tone::Refused, title: format!("{} didn’t take a change", capitalized(computer_name(&refused.computer))), words: refused.message.clone() });
        }
        items
    }

    fn computer_row(bot: &str, computer: &str, source: Option<crate::moves::RemoteSource>, can_move: bool) -> Row {
        let control = if can_move {
            Control::MoveTo { bot: bot.to_owned(), computer: computer.to_owned(), source }
        } else {
            Control::Text(computer.to_owned())
        };
        Row { key: "bot-lives".into(), title: "Computer".into(), detail: String::new(), control }
    }

    fn personality_row(&self, bot: &Bot) -> Row {
        let (name, description) = match (&self.templates, &bot.personality) {
            (_, None) => (OWN_WAY.into(), OWN_WAY_DESCRIPTION.into()),
            (Reading::NotYet, Some(_)) => ("Loading personality…".into(), String::new()),
            (Reading::Failed(_), Some(_)) => ("Personality unavailable".into(), String::new()),
            (Reading::Ready(templates), Some(id)) => templates.personalities.iter()
                .find(|each| &each.id == id)
                .map(|each| (each.name.clone(), each.description.clone()))
                .unwrap_or_else(|| ("Personality unavailable".into(), "Choose another personality.".into())),
        };
        let description = match self.templates {
            Reading::Failed(_) => "Couldn’t load personalities. Reopen this page to try again.".into(),
            _ => description,
        };
        let control = if self.is_busy("bot-personality") {
            Control::Busy("Saving…".into())
        } else if let Some(templates) = self.templates().filter(|_| self.can_open_editor()) {
            let mut choices = vec![(OWN_WAY.into(), OWN_WAY_DESCRIPTION.into(), Act::SetPersonality(None), bot.personality.is_none())];
            choices.extend(templates.personalities.iter().map(|choice| {
                let selected = bot.personality.as_ref() == Some(&choice.id);
                (choice.name.clone(), choice.description.clone(), Act::SetPersonality(Some(choice.id.clone())), selected)
            }));
            if self.editable_personality().is_some() {
                choices.push(("Edit personality…".into(), String::new(), Act::EditPersonality, false));
            }
            Control::Menu { shown: name, choices, personality: true }
        } else {
            Control::Text(name)
        };
        Row { key: "bot-personality".into(), title: "Personality".into(), detail: description, control }
    }

    fn setting_rows(&self, bot: &Bot) -> Vec<Row> {
        let busy_or = |key: &str, control: Control| match self.busy(&bot.id, key) {
            Some(words) => Control::Busy(words.to_owned()),
            None => control,
        };
        let locked = self.editor.is_some() || self.change_pending(&bot.id);
        let model_label = if let Some(model) = &bot.model {
            let name = self.model_name(model).unwrap_or_else(|| "Model unavailable".into());
            match model.variant.as_deref() {
                Some(effort) => format!("{name} · {}", effort_title(effort)),
                None if self.offered().is_some_and(|catalog| catalog.models.iter().any(|offered| offered.is(model) && !offered.efforts.is_empty())) => format!("{name} · Default"),
                None => name,
            }
        } else {
            "None".into()
        };
        // Its model, among those the box offers.
        let model = match (&self.offered, &bot.model) {
            (Reading::NotYet, _) => Control::Busy("Loading…".into()),
            (Reading::Ready(_), _) if locked => Control::Text(model_label),
            (Reading::Ready(_), model) => Control::Model { shown: model_label, current: model.clone(), new: None },
            (Reading::Failed(_), _) => Control::Text(String::new()),
        };
        let model_line = match &self.offered {
            Reading::Failed(_) => "Couldn’t load models. Reopen this page to try again.".into(),
            _ => String::new(),
        };
        vec![
            Row { key: "bot-model".into(), title: "Model".into(), detail: model_line, control: busy_or("bot-model", model) },
            self.personality_row(bot),
        ]
    }

    fn elsewhere_items(&self, id: &str) -> Vec<Item> {
        let Some(there) = self.there(id) else {
            return match &self.said {
                Reading::NotYet => vec![Item::Note { key: "bots-loading".into(), words: "Loading bots…".into(), busy: true }, Item::End],
                Reading::Failed(_) => vec![Item::Note { key: "bots-failed".into(), words: "Couldn’t load bots. Reopen Bots to try again.".into(), busy: false }, Item::End],
                // No computer lists it now, and nothing waits for it: deleted, or it went through.
                Reading::Ready(_) => vec![Item::Note { key: "elsewhere-done".into(), words: "None of your computers lists this bot now.".into(), busy: false }, Item::End],
            };
        };
        let computer = there.computer.clone();
        let mut items = vec![Item::ElsewhereHead {
            key: id.to_owned(),
            name: there.name.clone(),
            look: there.look.clone(),
            editable: there.listed,
            line: if there.job.is_empty() { format!("Lives on {computer}") } else { format!("{} · Lives on {computer}", there.job) },
        }];
        items.extend(self.notices(id));
        if there.listed {
            let source = self.move_source(id);
            let can_move = source.is_some();
            items.push(Item::Group("Where".into()));
            items.push(Item::Row { row: Box::new(Self::computer_row(id, &computer, source, can_move)), first: true, last: true });
            items.push(Item::Group("Manage bot".into()));
            items.push(Item::Row { row: Box::new(delete_row()), first: true, last: true });
            // What this box can't say of it: said plainly, with where it's said.
            items.push(Item::Note { key: "elsewhere-note".into(), words: format!("{} shows what {computer} last told it. Its job, personality, model and sessions are on {computer}.", capitalized(&self.this_box)), busy: false });
        } else {
            items.push(Item::Note { key: "elsewhere-note".into(), words: format!("{computer} hasn’t told {} about this bot.", self.this_box), busy: false });
        }
        items.push(Item::End);
        items
    }

    // MARK: New Bot

    fn start_form(&mut self, cx: &mut Context<Self>) {
        self.form = Form { request: new_request(), ..Form::default() };
        self.new_name.update(cx, |field, cx| field.set_text("", cx));
        self.wants_focus = Some(self.new_name.focus_handle(cx));
    }

    /// The look New Bot shows: the one picked, else the template's; none is the box's to pick.
    fn form_look(&self) -> Option<Look> {
        self.form.look.clone().or_else(|| {
            let template = self.form.template.as_ref()?;
            self.templates()?.templates.iter().find(|each| &each.id == template)?.look.clone()
        })
    }

    /// The personality New Bot shows: the one picked, else the template's.
    fn form_personality(&self) -> Option<String> {
        match &self.form.personality {
            Some(picked) => picked.clone(),
            None => {
                let template = self.form.template.as_ref()?;
                self.templates()?.templates.iter().find(|each| &each.id == template)?.personality.clone()
            }
        }
    }

    fn new_items(&self) -> Vec<Item> {
        let mut items = vec![Item::Title("New Bot".into())];
        let templates = match &self.templates {
            Reading::NotYet => {
                items.push(Item::Note { key: "new-loading".into(), words: "Loading what a bot can be here…".into(), busy: true });
                items.push(Item::End);
                return items;
            }
            Reading::Failed(why) => {
                items.push(Item::Note { key: "new-failed".into(), words: format!("New Bot couldn’t be read: {why}"), busy: false });
                items.push(Item::End);
                return items;
            }
            Reading::Ready(templates) => templates,
        };
        if !templates.templates.is_empty() {
            items.push(Item::Group("Start from".into()));
            items.push(Item::Templates { picked: self.form.template.clone() });
        }
        items.push(Item::Group("Bot".into()));
        let personality = self.form_personality();
        let has = personality.as_ref().and_then(|id| templates.personalities.iter().find(|each| &each.id == id));
        let mut choices = vec![(OWN_WAY.to_owned(), String::new(), Act::FormPersonality(None), personality.is_none())];
        choices.extend(templates.personalities.iter().map(|each| (each.name.clone(), each.description.clone(), Act::FormPersonality(Some(each.id.clone())), personality.as_ref() == Some(&each.id))));
        // The model it starts on is the box's (`startingModel`) unless another's picked.
        let starting = templates.starting().map(|model| self.model_name(model).unwrap_or_else(|| model.id.clone()));
        let model_shown = match (&self.form.model, &starting) {
            (Some(model), _) => self.model_name(model).unwrap_or_else(|| model.id.clone()),
            (None, Some(starting)) => starting.clone(),
            (None, None) => "Picked for you".into(),
        };
        let unset = (starting.clone().unwrap_or_else(|| "Picked for you".into()), format!("What a new bot starts on, on {}", self.this_box));
        let mut rows = vec![
            Row { key: "new-name".into(), title: "Name".into(), detail: "What it’s called, here and in group chats.".into(), control: Control::NameField },
            Row {
                key: "new-look".into(),
                title: "Look".into(),
                detail: if self.form_look().is_some() { "Its colour and shape, wherever it shows.".into() } else { "Picked for it when it’s made, unless you choose.".into() },
                control: Control::Face(self.form_look(), "Choose…".into(), Act::FormLook),
            },
            Row {
                key: "new-personality".into(),
                title: "Personality".into(),
                detail: has.map(|each| each.description.clone()).filter(|line| !line.is_empty()).unwrap_or_else(|| "How it talks.".into()),
                control: Control::Menu { shown: has.map(|each| each.name.clone()).or(personality).unwrap_or_else(|| OWN_WAY.to_owned()), choices, personality: false },
            },
            Row { key: "new-model".into(), title: "Model".into(), detail: "What it thinks with.".into(), control: Control::Model { shown: model_shown, current: self.form.model.clone(), new: Some(unset) } },
        ];
        if !templates.projects.is_empty() {
            let mut projects = vec![("A folder of its own".to_owned(), String::new(), Act::FormProject(None), self.form.project.is_none())];
            projects.extend(templates.projects.iter().map(|project| (self.project_name(&project.directory), String::new(), Act::FormProject(Some(project.directory.clone())), self.form.project.as_ref() == Some(&project.directory))));
            rows.push(Row {
                key: "new-project".into(),
                title: "Works in".into(),
                // (A template made for a project says so; whether one's needed is the box's to say.)
                detail: if self.form.template.as_ref().is_some_and(|id| templates.templates.iter().any(|each| &each.id == id && each.wants_project)) {
                    "This one is made to work in one of your projects.".into()
                } else {
                    "Its own folder, or one of your projects.".into()
                },
                control: Control::Menu { shown: self.form.project.as_deref().map(|project| self.project_name(project)).unwrap_or_else(|| "A folder of its own".into()), choices: projects, personality: false },
            });
        }
        let count = rows.len();
        items.extend(rows.into_iter().enumerate().map(|(ix, row)| Item::Row { row: Box::new(row), first: ix == 0, last: ix + 1 == count }));
        items.push(Item::Foot { error: self.form.error.clone(), creating: self.form.creating });
        items.push(Item::End);
        items
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        if self.form.creating {
            return;
        }
        // Everything is sent as written: what a bot may be called is the box's to say.
        let new = NewBot {
            name: self.new_name.read(cx).text().trim().to_owned(),
            look: self.form.look.clone(),
            personality: self.form.personality.clone().flatten(),
            model: self.form.model.clone(),
            project: self.form.project.clone(),
            template: self.form.template.clone(),
            request: self.form.request.clone(),
        };
        self.form.creating = true;
        self.form.error = None;
        let request = new.request.clone();
        self.run(
            "create",
            move |client| client.make_bot(&new),
            move |this, made, cx| {
                // (Only this New Bot's own answer is its form's.)
                let its_form = this.form.request == request;
                if its_form {
                    this.form.creating = false;
                }
                match made {
                    Ok(made) => {
                        cx.emit(BotsEvent::Changed);
                        if its_form {
                            // Made: the next New Bot starts empty, with an id of its own.
                            this.form = Form::default();
                        }
                        // On to its conversation (from a template its job waits in the composer,
                        // to edit and send; otherwise it writes first), unless you've gone
                        // elsewhere meanwhile: then it's in the list, and nothing moves.
                        if its_form && this.shown == Shown::New {
                            cx.emit(BotsEvent::Chat { bot: made.bot.id.clone(), session: made.bot.session.clone(), draft: made.first_message.filter(|draft| !draft.is_empty()) });
                        }
                    }
                    Err(error) if its_form => this.form.error = Some(said(&error)),
                    Err(_) => {}
                }
            },
            cx,
        );
    }

    // MARK: Changes

    fn set_busy(&mut self, key: &str, words: &str) {
        self.busy.retain(|(busy, _)| busy != key);
        self.busy.push((key.to_owned(), words.to_owned()));
    }

    fn done(&mut self, key: &str) {
        self.busy.retain(|(busy, _)| busy != key);
    }

    /// The first look the box offers, for a bot it gave none.
    fn first_look(&self) -> Option<Look> {
        let looks = &self.templates()?.looks;
        Some(Look { color: looks.colors.first()?.clone(), shape: looks.shapes.first()?.clone(), face: looks.faces.first()?.clone() })
    }

    /// Whether a change is on its way for this row of the bot shown.
    fn is_busy(&self, key: &str) -> bool {
        self.bot().is_some_and(|bot| self.busy(&bot.id, key).is_some())
    }

    /// What a control asked for.
    pub fn act(&mut self, act: Act, cx: &mut Context<Self>) {
        if self.client.gate().is_held() && matches!(act, Act::New | Act::Create) {
            return;
        }
        if matches!(act, Act::EditLook | Act::EditPersonality | Act::Delete)
            && !self.can_open_editor()
        {
            return;
        }
        // Replies carry the whole bot. Let one settings change finish before sending another.
        if matches!(act, Act::SetPersonality(_) | Act::SetModel(_) | Act::SetEffort(_)) {
            if self.editor.is_some() || self.bot().is_some_and(|bot| self.change_pending(&bot.id)) {
                return;
            }
        }
        match act {
            Act::Chat => {
                if let Some(bot) = self.bot() {
                    cx.emit(BotsEvent::Chat { bot: bot.id.clone(), session: bot.session.clone(), draft: None });
                }
            }
            Act::OpenSession(session) => cx.emit(BotsEvent::Session(session)),
            Act::ShowAll => self.all_sessions = true,
            Act::Rename => {
                self.wants_focus = Some(self.name.focus_handle(cx));
            }
            Act::EditLook => {
                // From the look it has; a bot elsewhere whose computer said none, the box's first.
                let Some(look) = self.subject().and_then(|(_, _, look)| look.or_else(|| self.first_look())) else { return };
                self.open_editor(Sheet::Look { look, new: false }, None);
            }
            Act::EditPersonality => {
                let Some(personality) = self.editable_personality().cloned() else { return };
                self.personality_name.update(cx, |field, cx| field.set_text(personality.name.clone(), cx));
                self.personality_description.update(cx, |field, cx| field.set_text(personality.description.clone(), cx));
                self.open_editor(Sheet::Personality(personality), Some(self.personality_name.focus_handle(cx)));
            }
            Act::FormLook => {
                // From the look shown; with none, the first of each the box offers.
                let look = self.form_look().or_else(|| self.first_look());
                let Some(look) = look else { return };
                self.open_editor(Sheet::Look { look, new: true }, None);
            }
            Act::SetEffort(effort) => self.change("bot-effort", BotChange { effort: Some(effort), ..BotChange::default() }, cx),
            Act::SetPersonality(personality) => {
                self.change("bot-personality", BotChange { personality: Some(personality), ..BotChange::default() }, cx);
            }
            Act::SetModel(model) => self.change("bot-model", BotChange { model: Some(model), ..BotChange::default() }, cx),
            Act::Delete => self.open_editor(Sheet::Delete, None),
            Act::New => cx.emit(BotsEvent::Select(Some("new".into()))),
            Act::PickTemplate(template) => {
                // A template's name goes in while the name is still a template's (or empty).
                let templates = self.templates().map(|templates| templates.templates.clone()).unwrap_or_default();
                let typed = self.new_name.read(cx).text().trim().to_owned();
                if typed.is_empty() || templates.iter().any(|each| each.name == typed) {
                    let name = template.as_ref().and_then(|id| templates.iter().find(|each| &each.id == id)).map(|each| each.name.clone()).unwrap_or_default();
                    self.new_name.update(cx, |field, cx| field.set_text(name, cx));
                }
                // Its look and personality show as the template's again.
                self.form.template = template;
                // (The keyboard goes back to its name, to change it.)
                self.wants_focus = Some(self.new_name.focus_handle(cx));
                self.form.look = None;
                self.form.personality = None;
            }
            Act::FormPersonality(personality) => self.form.personality = Some(personality),
            Act::FormModel(model) => self.form.model = model,
            Act::FormProject(project) => self.form.project = project,
            Act::Create => self.create(cx),
            Act::CancelNew => cx.emit(BotsEvent::Select(None)),
        }
        self.rebuild(cx);
    }

    fn editable_personality(&self) -> Option<&Personality> {
        let id = self.bot()?.personality.as_ref()?;
        self.templates()?.personalities.iter().find(|choice| choice.custom && &choice.id == id)
    }

    fn can_open_editor(&self) -> bool {
        self.editor.is_none() && self.subject().is_some_and(|(id, ..)| !self.change_pending(&id))
    }

    fn change_pending(&self, id: &str) -> bool {
        self.busy.iter().any(|(key, _)| key.split_once('/').is_some_and(|(bot, _)| bot == id))
    }

    fn open_editor(&mut self, editor: Sheet, focus: Option<FocusHandle>) {
        self.editor = Some(editor);
        self.edit_error = None;
        self.saving = false;
        self.problem = None;
        self.wants_focus = Some(focus.unwrap_or_else(|| self.sheet_focus.clone()));
    }

    pub fn close_editor(&mut self, cx: &mut Context<Self>) {
        self.finish_editor(true, cx);
    }

    fn finish_editor(&mut self, restore_focus: bool, cx: &mut Context<Self>) {
        // An editor whose change is on its way stays until the response.
        if self.saving {
            return;
        }
        self.editor = None;
        self.edit_error = None;
        self.wants_focus = restore_focus.then(|| self.focus.clone());
        self.rebuild(cx);
    }

    fn profile_field(&self, field: ProfileField) -> &Entity<TextField> {
        match field {
            ProfileField::Name => &self.name,
            ProfileField::Job => &self.line,
            ProfileField::Instructions => &self.details,
        }
    }

    fn capture_profile(&mut self, cx: &App) {
        let Some(id) = &self.profile_shown else { return };
        let texts = ProfileField::ALL.map(|field| self.profile_field(field).read(cx).text().to_owned());
        let draft = self.profiles.entry(id.clone()).or_default();
        for (field, text) in draft.iter_mut().zip(texts) {
            if text != field.text {
                field.text = text;
                field.generation += 1;
            }
        }
    }

    fn sync_profile(&mut self, cx: &mut Context<Self>) {
        self.capture_profile(cx);
        let Some((id, name, _)) = self.subject() else {
            self.profile_shown = None;
            return;
        };
        let values = if let Some(bot) = self.bot() {
            [Some(name), Some(bot.job.clone()), Some(bot.job_details.clone())]
        } else {
            let name = self.held(&id).and_then(|held| held.name()).unwrap_or(&name).to_owned();
            [Some(name), None, None]
        };
        let draft = self.profiles.entry(id.clone()).or_default();
        for (field, saved) in draft.iter_mut().zip(values) {
            // A refresh can change the baseline but only replaces a field without a draft.
            if field.pending.is_none() && field.queued.is_none() && field.error.is_none()
                && field.generation == field.clean_generation
            {
                field.text = saved.as_deref().unwrap_or_default().to_owned();
            }
            field.saved = saved;
        }
        self.profile_shown = Some(id);
        self.draw_profile_fields(cx);
    }

    fn draw_profile_fields(&self, cx: &mut Context<Self>) {
        let Some(draft) = self.profile_shown.as_ref().and_then(|id| self.profiles.get(id)) else { return };
        for (field, state) in ProfileField::ALL.into_iter().zip(draft) {
            let input = self.profile_field(field);
            if input.read(cx).text() != state.text {
                input.update(cx, |input, cx| input.set_text(state.text.clone(), cx));
            }
        }
    }

    fn profile_errors(&self) -> impl Iterator<Item = &str> {
        self.profile_shown.as_ref().and_then(|id| self.profiles.get(id)).into_iter().flatten()
            .filter_map(|field| field.error.as_ref().map(|error| error.message.as_str()))
    }

    fn profile_key(&mut self, field: ProfileField, event: &TextFieldEvent, cx: &mut Context<Self>) {
        match event {
            TextFieldEvent::Changed => self.capture_profile(cx),
            TextFieldEvent::Submit(_) => self.queue_profile(&[field], true, cx),
            TextFieldEvent::Cancel => {
                self.capture_profile(cx);
                let Some(draft) = self.profile_shown.as_ref().and_then(|id| self.profiles.get_mut(id)) else { return };
                let draft = &mut draft[field as usize];
                draft.generation += 1;
                draft.text = if let Some(pending) = &mut draft.pending {
                    pending.generation = draft.generation;
                    pending.text.clone()
                } else {
                    draft.clean_generation = draft.generation;
                    draft.saved.clone().unwrap_or_default()
                };
                draft.queued = None;
                draft.error = None;
                self.draw_profile_fields(cx);
                cx.notify();
            }
        }
    }

    fn save_profile(&mut self, cx: &mut Context<Self>) {
        self.queue_profile(&ProfileField::ALL, true, cx);
    }

    fn queue_profile(&mut self, fields: &[ProfileField], retry_failed: bool, cx: &mut Context<Self>) {
        if self.sheet().is_some() {
            return;
        }
        self.capture_profile(cx);
        let Some(id) = self.profile_shown.clone() else { return };
        let Some(draft) = self.profiles.get_mut(&id) else { return };
        for &field in fields {
            let state = &mut draft[field as usize];
            let Some(saved) = &state.saved else { continue };
            if !retry_failed && state.error.as_ref().is_some_and(|error| error.generation == state.generation) {
                continue;
            }
            let text = state.text.trim();
            let baseline = state.pending.as_ref().map_or(saved.as_str(), |save| save.text.as_str());
            if text == baseline {
                state.queued = None;
                if state.pending.is_none() {
                    state.text = saved.clone();
                    state.clean_generation = state.generation;
                    state.error = None;
                }
            } else if matches!(field, ProfileField::Name) && text.is_empty() {
                state.text = baseline.to_owned();
                state.generation += 1;
                state.queued = None;
                state.error = Some(FieldError { generation: state.generation, message: "A bot needs a name.".into() });
            } else {
                state.queued = Some(FieldSave { text: text.to_owned(), generation: state.generation });
            }
        }
        self.draw_profile_fields(cx);
        self.drain_profile(&id, cx);
        cx.notify();
    }

    fn drain_profile(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.change_pending(id) {
            return;
        }
        let Some(draft) = self.profiles.get_mut(id) else { return };
        // Save one field at a time so a refusal leaves other fields free to save.
        let Some((field, state, save)) = ProfileField::ALL.into_iter().zip(draft).find_map(|(field, state)| {
            let save = state.queued.take()?;
            if state.saved.as_deref() == Some(&save.text) {
                if state.generation == save.generation {
                    state.clean_generation = state.generation;
                }
                return None;
            }
            Some((field, state, save))
        }) else { return };
        let mut change = BotChange::default();
        match field {
            ProfileField::Name => change.name = Some(save.text.clone()),
            ProfileField::Job => change.job_line = Some(save.text.clone()),
            ProfileField::Instructions => change.job_details = Some(save.text.clone()),
        }
        state.error = None;
        state.pending = Some(save);
        let id = id.to_owned();
        let key = format!("{id}/profile");
        self.set_busy(&key, "Saving…");
        let target = id.clone();
        self.run(key.clone(), move |client| client.change_bot(&target, &change), move |this, outcome, cx| {
            this.done(&key);
            this.capture_profile(cx);
            if let Some(state) = this.profiles.get_mut(&id).map(|draft| &mut draft[field as usize])
                && let Some(sent) = state.pending.take()
            {
                match &outcome {
                    Ok(answer) => {
                        let saved = match answer {
                            Outcome::Done(Some(bot)) => match field {
                                ProfileField::Name => bot.name.clone(),
                                ProfileField::Job => bot.job.clone(),
                                ProfileField::Instructions => bot.job_details.clone(),
                            },
                            _ => sent.text,
                        };
                        if state.generation == sent.generation {
                            state.text = saved.clone();
                            state.clean_generation = state.generation;
                        }
                        state.saved = Some(saved);
                    }
                    Err(error) => {
                        state.error = Some(FieldError {
                            generation: sent.generation,
                            message: edit_problem(error, "Couldn’t save the change.", "Check the fields and try again."),
                        });
                    }
                }
            }
            if this.profile_shown.as_deref() == Some(&id) {
                this.draw_profile_fields(cx);
            }
            if let Ok(answer) = outcome {
                this.took(&id, answer, false, cx);
            }
        }, cx);
    }

    /// Return or Escape in an inline field.
    fn editor_key(&mut self, event: &TextFieldEvent, cx: &mut Context<Self>) {
        match event {
            TextFieldEvent::Submit(_) => self.save_editor(cx),
            TextFieldEvent::Cancel => self.close_editor(cx),
            TextFieldEvent::Changed => {}
        }
    }

    /// Picks a colour or a shape in the look's sheet.
    pub fn pick_look(&mut self, color: Option<&str>, shape: Option<&str>, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        if let Some(Sheet::Look { look, .. }) = &mut self.editor {
            if let Some(color) = color {
                look.color = color.to_owned();
            }
            if let Some(shape) = shape {
                look.shape = shape.to_owned();
            }
            cx.notify();
        }
    }

    /// Send Save or Delete and keep the editor open until the request completes.
    pub fn save_editor(&mut self, cx: &mut Context<Self>) {
        if self.sheet().is_some() {
            self.submit_editor(true, cx);
        } else {
            self.save_profile(cx);
        }
    }

    fn submit_editor(&mut self, restore_focus: bool, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(editor) = self.editor.clone() else { return };
        if let Sheet::Personality(personality) = &editor {
            self.save_personality(personality.clone(), cx);
            return;
        }
        let change = match &editor {
            // The new bot's look: kept for when it's made.
            Sheet::Look { look, new: true } => {
                self.form.look = Some(look.clone());
                self.close_editor(cx);
                return;
            }
            Sheet::Look { look, new: false } => Some(BotChange { look: Some(look.clone()), ..BotChange::default() }),
            Sheet::Delete => None,
            Sheet::Personality(_) => unreachable!("saved separately"),
        };
        let Some((bot, _, _)) = self.subject() else { return };
        self.saving = true;
        self.edit_error = None;
        self.disable_editor_fields(&editor, true, cx);
        let key = format!("{bot}/editor");
        self.set_busy(&key, "Saving…");
        let (id, deleting, page) = (bot.clone(), change.is_none(), self.shown.clone());
        self.run(
            key.clone(),
            move |client| match &change {
                Some(change) => client.change_bot(&id, change),
                None => client.delete_bot(&id),
            },
            move |this, outcome, cx| {
                this.done(&key);
                let its_editor = this.shown == page && this.editor.as_ref() == Some(&editor);
                if its_editor {
                    this.saving = false;
                    this.disable_editor_fields(&editor, false, cx);
                }
                match outcome {
                    Ok(outcome) => {
                        if its_editor {
                            this.editor = None;
                            this.wants_focus = restore_focus.then(|| this.focus.clone());
                        }
                        this.took(&bot, outcome, deleting, cx);
                    }
                    Err(error) if its_editor => {
                        let retry = match editor {
                            Sheet::Look { .. } => "Choose an avatar and try again.",
                            _ => "Try again.",
                        };
                        this.edit_error = Some(edit_problem(&error, if deleting { "Couldn’t delete the bot." } else { "Couldn’t save the change." }, retry));
                    }
                    // You've gone elsewhere meanwhile: nothing changed, and nothing is said over
                    // another bot's page.
                    Err(_) => {}
                }
            },
            cx,
        );
        self.rebuild(cx);
    }

    fn save_personality(&mut self, personality: Personality, cx: &mut Context<Self>) {
        let Some(bot) = self.bot().map(|bot| bot.id.clone()) else { return };
        let name = self.personality_name.read(cx).text().trim().to_owned();
        let description = self.personality_description.read(cx).text().trim().to_owned();
        let editor = Sheet::Personality(personality.clone());
        let page = self.shown.clone();
        let key = format!("{bot}/editor");
        self.saving = true;
        self.edit_error = None;
        self.disable_editor_fields(&editor, true, cx);
        self.set_busy(&key, "Saving…");
        self.run(
            key.clone(),
            move |client| client.change_personality(&personality.id, &name, &description),
            move |this, result, cx| {
                this.done(&key);
                let active = this.shown == page && this.editor.as_ref() == Some(&editor);
                if active {
                    this.saving = false;
                    this.disable_editor_fields(&editor, false, cx);
                }
                match result {
                    Ok(changed) => {
                        if active {
                            this.close_editor(cx);
                        }
                        if let Reading::Ready(templates) = &mut this.templates {
                            if let Some(choice) = templates.personalities.iter_mut().find(|choice| choice.id == changed.id) {
                                *choice = changed;
                            }
                        }
                        // Refresh any catalogue read begun while the change was pending.
                        this.read_offers(cx);
                    }
                    Err(error) if active => {
                        this.edit_error = Some(edit_problem(&error, "Couldn’t save the personality.", "Check the name and description, then try again."));
                    }
                    Err(_) => {}
                }
            },
            cx,
        );
        self.rebuild(cx);
    }

    fn disable_editor_fields(&self, editor: &Sheet, disabled: bool, cx: &mut Context<Self>) {
        let fields = match editor {
            Sheet::Personality(_) => &[&self.personality_name, &self.personality_description][..],
            _ => return,
        };
        for field in fields {
            field.update(cx, |field, cx| field.set_disabled(disabled, cx));
        }
    }

    /// A row's own change (a menu's pick): busy while it goes, then the box's answer, or its
    /// refusal over the page.
    fn change(&mut self, key: &str, change: BotChange, cx: &mut Context<Self>) {
        let Some(bot) = self.bot().map(|bot| bot.id.clone()) else { return };
        self.problem = None;
        // (Busy and its answer are this bot's row's, whatever shows by then.)
        let (key, id) = (format!("{bot}/{key}"), bot.clone());
        self.set_busy(&key, "Saving…");
        self.run(
            key.clone(),
            move |client| client.change_bot(&id, &change),
            move |this, outcome, cx| {
                this.done(&key);
                match outcome {
                    Ok(outcome) => this.took(&bot, outcome, false, cx),
                    Err(error) if this.shown == Shown::Bot(bot) => {
                        this.problem = Some(edit_problem(&error, "Couldn’t save the change.", "Choose an option and try again."));
                    }
                    Err(_) => {}
                }
            },
            cx,
        );
    }

    /// What the box answered a change or a delete with, drawn: the bot as it is now, or what it
    /// holds for the computer the bot lives on.
    fn took(&mut self, bot: &str, outcome: Outcome, deleted: bool, cx: &mut Context<Self>) {
        if let Reading::Ready(said) = &mut self.said {
            match outcome {
                Outcome::Done(Some(mut now)) => {
                    if let Some(listed) = said.bots.iter_mut().find(|each| each.id == bot) {
                        // (A change answers without what it's doing: that stays as last said.)
                        if now.now == Default::default() {
                            now.now = listed.now.clone();
                        }
                        *listed = *now;
                    }
                }
                Outcome::Done(None) if deleted => {
                    said.bots.retain(|each| each.id != bot);
                    if self.shown == Shown::Bot(bot.to_owned()) {
                        cx.emit(BotsEvent::Select(None));
                    }
                }
                Outcome::Done(None) => {}
                Outcome::Held(held) => {
                    said.held.retain(|each| each.bot != bot);
                    said.held.push(*held);
                }
            }
        }
        // The sidebar's list, and whatever else changed with it, as the box says it now.
        cx.emit(BotsEvent::Changed);
    }
}

/// How a personality left to the model is named.
const OWN_WAY: &str = "The model’s own way";
const OWN_WAY_DESCRIPTION: &str = "Uses the model’s default style.";

fn delete_row() -> Row {
    Row { key: "bot-delete".into(), title: "Delete Bot".into(), detail: String::new(), control: Control::Delete }
}

fn edit_problem(error: &kuvo_box::Error, problem: &str, retry: &str) -> String {
    match error {
        kuvo_box::Error::Refused(refusal) if matches!(refusal.message.as_str(),
            "Shorten Job to 200 characters or fewer." | "Shorten Instructions to 8,000 characters or fewer."
        ) => refusal.message.clone(),
        kuvo_box::Error::Refused(refusal) if refusal.message == "A bot needs a name." => {
            format!("{} Enter a name and try again.", refusal.message)
        }
        kuvo_box::Error::Refused(refusal) if refusal.message == "It’s moving to another computer. Try again once it’s there." => {
            refusal.message.clone()
        }
        kuvo_box::Error::Refused(refusal) if refusal.older_box => format!("{problem} Update Kuvo and try again."),
        kuvo_box::Error::NeedsUpdating { .. } => format!("{problem} Update Kuvo and try again."),
        kuvo_box::Error::NotRunning(_) | kuvo_box::Error::Unreachable(_) => {
            format!("{problem} Check that the computer is online, then try again.")
        }
        _ => format!("{problem} {retry}"),
    }
}

/// "name", "name and look", "name, look and job".
fn listed<S: AsRef<str>>(words: &[S]) -> String {
    match words {
        [] => String::new(),
        [one] => one.as_ref().to_owned(),
        [most @ .., last] => format!("{} and {}", most.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(", "), last.as_ref()),
    }
}

/// A bot that lives on another of your computers, as it's drawn: all of it the box's
/// (`elsewhere` in `kuvo/bots`), named where the box had no name for it.
#[derive(Clone, Debug, PartialEq)]
pub struct There {
    pub id: String,
    pub name: String,
    pub look: Option<Look>,
    /// What it does, in a few words, as its computer last said (empty: none said).
    pub job: String,
    /// The computer it lives on, by the name the box gave it.
    pub computer: String,
    /// One of your computers lists it now, so it can be changed from here.
    pub listed: bool,
    /// The box holds a change or a delete for it.
    pub waiting: bool,
    /// Its computer refused a change, lately.
    pub refused: bool,
}

impl There {
    pub fn of(bot: &Elsewhere) -> There {
        let computer = computer_name(&bot.computer).to_owned();
        There {
            id: bot.id.clone(),
            // (One the box has no name for: PROTOCOL.md, `bots.elsewhere`.)
            name: bot.name.clone().filter(|name| !name.is_empty()).unwrap_or_else(|| format!("A bot on {computer}")),
            look: bot.look.clone(),
            job: bot.job.clone(),
            computer,
            listed: !bot.unlisted,
            waiting: bot.waiting,
            refused: bot.refused,
        }
    }
}

/// A computer by the name the box gave it; where it gave none, a neutral one.
pub fn computer_name(name: &str) -> &str {
    if name.trim().is_empty() { "another computer" } else { name }
}

fn capitalized(words: &str) -> String {
    let mut chars = words.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// This New Bot's own id (8 to 64 of `A-Za-z0-9_-`), sent again with a retry.
fn new_request() -> String {
    BoxClient::new_id().chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')).take(64).collect()
}

/// How one of a bot's sessions stands, in a word or two, from the box's state (none for a state a
/// newer box names: nothing is claimed of it).
pub fn state_words(state: WorkState) -> Option<&'static str> {
    Some(match state {
        WorkState::Working => "At work",
        WorkState::Asking => "Waiting on you",
        WorkState::Finished => "Finished",
        WorkState::Failed => "Failed",
        WorkState::Stopped => "Stopped",
        WorkState::Idle => "Idle",
        WorkState::Other => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whats_held_is_listed_in_words() {
        assert_eq!(listed::<&str>(&[]), "");
        assert_eq!(listed(&["name"]), "name");
        assert_eq!(listed(&["name", "look"]), "name and look");
        assert_eq!(listed(&["name", "look", "job"]), "name, look and job");
        let request = new_request();
        assert!((8..=64).contains(&request.len()) && request.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'), "{request}");
    }

    /// A session waiting on you colours only "Waiting on you" on its bot's page, never its project
    /// (a project named like the words included); others colour nothing.
    #[test]
    fn only_waiting_on_you_is_coloured_in_a_session_line() {
        let coloured = |project: &str, state| {
            let (line, range) = super::session_line(project, state);
            (line.clone(), range.map(|range| line[range].to_owned()))
        };
        assert_eq!(coloured("Website", WorkState::Asking), ("Website · Waiting on you".into(), Some("Waiting on you".into())));
        let (line, range) = super::session_line("Waiting on you", WorkState::Asking);
        assert_eq!(range, Some("Waiting on you · ".len()..line.len()), "the state, not the project of the same name");
        assert_eq!(coloured("", WorkState::Asking), ("Waiting on you".into(), Some("Waiting on you".into())));
        assert_eq!(coloured("Website", WorkState::Working), ("Website · At work".into(), None));
        assert_eq!(coloured("Website", WorkState::Other), ("Website".into(), None));
    }
}
