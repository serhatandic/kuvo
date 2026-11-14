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
use kuvo_box::bots::{BotChange, BotsSaid, Held, NewBot, Outcome, Refused, Templates, Work, WorkSession, WorkState};
use kuvo_box::models::Offered;
use kuvo_box::typed::ModelRef;
use kuvo_ui::{TextField, TextFieldEvent};

use crate::areas::{Reading, doing, reading};
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
    /// A bot that lives on another computer, known here by what's held for it (or was refused).
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
    EditJob,
    EditLook,
    SetPersonality(Option<String>),
    SetModel(ModelRef),
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
    Name,
    Job,
    /// The look being picked, and whether it's the new bot's.
    Look { look: Look, new: bool },
    Delete,
}

/// A row's control, at its right.
#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    Text(String),
    Busy(String),
    Button(String, Act),
    /// A bot's face beside a button.
    Face(Option<Look>, String, Act),
    Danger(String, Act),
    /// What's chosen, and the choices: each one's name, a line under it, what it does, whether it's on.
    Menu { shown: String, choices: Vec<(String, String, Act, bool)> },
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

/// One of the page list's items.
#[derive(Clone, Debug, PartialEq)]
enum Item {
    Title(String),
    /// A bot's head: its face, name and job, what it's doing now, and Chat.
    Head { bot: Box<Bot>, doing: Option<String> },
    /// A bot that lives elsewhere: what's known of it here.
    ElsewhereHead { key: String, name: String, look: Option<Look>, line: String },
    Notice { key: String, tone: Tone, title: String, words: String },
    Group(String),
    Row { row: Box<Row>, first: bool, last: bool },
    Session { session: WorkSession, project: String, ago: String, first: bool, last: bool },
    More { count: usize, last: bool },
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

/// The Bots area's main view.
pub struct BotsView {
    client: BoxClient,
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
    sheet: Option<Sheet>,
    /// The sheet's change is on its way; and why the box turned the last one down.
    saving: bool,
    sheet_error: Option<String>,
    /// The sheets' fields, made once: a name, a job's line and its details; and New Bot's name.
    name: Entity<TextField>,
    line: Entity<TextField>,
    details: Entity<TextField>,
    new_name: Entity<TextField>,
    form: Form,
    /// What takes the keyboard as it's next drawn.
    wants_focus: Option<FocusHandle>,
    sheet_focus: FocusHandle,
    this_box: String,
    now: Option<f64>,
    items: Vec<Item>,
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
    pub fn new(client: BoxClient, live: bool, cx: &mut Context<Self>) -> BotsView {
        let name = cx.new(|cx| TextField::new(cx).placeholder("Name"));
        let line = cx.new(|cx| TextField::new(cx).placeholder("What it does, in a few words"));
        let details = cx.new(|cx| TextField::new(cx).multi_line(4, 10).placeholder("All of it: what to do, when, and what to tell you"));
        let new_name = cx.new(|cx| TextField::new(cx).placeholder("Name"));
        let subscriptions = vec![
            cx.subscribe(&name, |this: &mut BotsView, _, event: &TextFieldEvent, cx| this.sheet_key(event, cx)),
            cx.subscribe(&line, |this: &mut BotsView, _, event: &TextFieldEvent, cx| this.sheet_key(event, cx)),
            cx.subscribe(&details, |this: &mut BotsView, _, event: &TextFieldEvent, cx| {
                if matches!(event, TextFieldEvent::Cancel) {
                    this.close_sheet(cx);
                }
            }),
            cx.subscribe(&new_name, |this: &mut BotsView, _, event: &TextFieldEvent, cx| match event {
                TextFieldEvent::Submit(_) => this.act(Act::Create, cx),
                TextFieldEvent::Cancel => this.act(Act::CancelNew, cx),
                TextFieldEvent::Changed => {}
            }),
        ];
        let mut view = BotsView {
            client,
            live,
            said: Reading::NotYet,
            shown: Shown::Nothing,
            work: Reading::NotYet,
            all_sessions: false,
            templates: Reading::NotYet,
            offered: Reading::NotYet,
            busy: Vec::new(),
            problem: None,
            sheet: None,
            saving: false,
            sheet_error: None,
            name,
            line,
            details,
            new_name,
            form: Form::default(),
            wants_focus: None,
            sheet_focus: cx.focus_handle(),
            this_box: "This Mac".into(),
            now: None,
            items: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, px(400.)),
            focus: cx.focus_handle(),
            tasks: HashMap::new(),
            _subscriptions: subscriptions,
        };
        view.rebuild(cx);
        view
    }

    pub fn shown(&self) -> &Shown {
        &self.shown
    }

    /// The sheet open, if one is.
    pub fn sheet(&self) -> Option<&Sheet> {
        self.sheet.as_ref()
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
            self.shown = shown;
            self.problem = None;
            self.sheet = None;
            self.sheet_error = None;
            self.all_sessions = false;
            self.work = Reading::NotYet;
            self.list.scroll_to(gpui::ListOffset::default());
            if self.shown == Shown::New {
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
            Shown::New => self.read_offers(cx),
            Shown::Nothing | Shown::Elsewhere(_) => {}
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

    fn offered(&self) -> Option<&Offered> {
        match &self.offered {
            Reading::Ready(offered) => Some(offered),
            _ => None,
        }
    }

    fn busy(&self, key: &str) -> Option<&str> {
        self.busy.iter().find(|(busy, _)| busy == key).map(|(_, words)| words.as_str())
    }

    /// A model's name, as the box's list of models offered names it (its id where it lists none).
    fn model_name(&self, model: &ModelRef) -> String {
        self.offered().and_then(|offered| offered.models.iter().find(|each| each.is(model))).map(|each| each.name.clone()).unwrap_or_else(|| model.id.clone())
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
        let items = match self.shown.clone() {
            Shown::Nothing => self.nothing_items(),
            Shown::Bot(id) => self.bot_items(&id),
            Shown::Elsewhere(id) => self.elsewhere_items(&id),
            Shown::New => self.new_items(),
        };
        // Only the items that changed are laid out again; the rest keep their places and the
        // page keeps its scroll.
        if items != self.items {
            let start = self.items.iter().zip(&items).take_while(|(old, new)| old == new).count();
            let tail = self.items[start..].iter().rev().zip(items[start..].iter().rev()).take_while(|(old, new)| old == new).count();
            self.list.splice(start..self.items.len() - tail, items.len() - tail - start);
            self.items = items;
        }
        cx.notify();
    }

    fn nothing_items(&self) -> Vec<Item> {
        match &self.said {
            Reading::NotYet => vec![Item::Note { key: "bots-loading".into(), words: "Loading bots…".into(), busy: true }, Item::End],
            Reading::Failed(why) => vec![Item::Note { key: "bots-failed".into(), words: format!("Bots couldn’t be read: {why}"), busy: false }, Item::End],
            Reading::Ready(said) => vec![Item::Empty { none: said.bots.is_empty() }, Item::End],
        }
    }

    fn bot_items(&self, id: &str) -> Vec<Item> {
        let Some(bot) = self.bot() else {
            return match &self.said {
                Reading::NotYet => vec![Item::Note { key: "bots-loading".into(), words: "Loading bots…".into(), busy: true }, Item::End],
                Reading::Failed(why) => vec![Item::Note { key: "bots-failed".into(), words: format!("Bots couldn’t be read: {why}"), busy: false }, Item::End],
                // Gone from the box's list since it was picked (deleted or moved, from anywhere).
                Reading::Ready(_) => vec![Item::Note { key: "bot-gone".into(), words: format!("This bot isn’t on {} any more.", self.this_box), busy: false }, Item::End],
            };
        };
        let mut items = vec![Item::Head { bot: Box::new(bot.clone()), doing: doing(&bot.now, self.now()) }];
        items.extend(self.notices(id));

        items.push(Item::Group("Recent sessions".into()));
        match &self.work {
            Reading::NotYet => items.push(Item::Note { key: "work-loading".into(), words: "Loading its sessions…".into(), busy: true }),
            Reading::Failed(why) => items.push(Item::Note { key: "work-failed".into(), words: format!("Its sessions couldn’t be read: {why}"), busy: false }),
            Reading::Ready(work) if work.sessions.is_empty() => items.push(Item::Note { key: "work-none".into(), words: "It hasn’t started a session yet.".into(), busy: false }),
            Reading::Ready(work) => {
                let count = work.sessions.len();
                let listed = if self.all_sessions { count } else { count.min(RECENT) };
                let more = count > listed;
                for (ix, session) in work.sessions.iter().take(listed).enumerate() {
                    items.push(Item::Session {
                        session: session.clone(),
                        project: session.project.as_deref().map(|project| self.project_name(project)).unwrap_or_default(),
                        ago: session.started.map(|started| time::ago(started, self.now())).unwrap_or_default(),
                        first: ix == 0,
                        last: ix + 1 == listed && !more,
                    });
                }
                if more {
                    items.push(Item::More { count, last: true });
                }
            }
        }

        items.push(Item::Group("Settings".into()));
        let rows = self.setting_rows(bot);
        let count = rows.len();
        items.extend(rows.into_iter().enumerate().map(|(ix, row)| Item::Row { row: Box::new(row), first: ix == 0, last: ix + 1 == count }));

        items.push(Item::Group(String::new()));
        let delete = Row {
            key: "bot-delete".into(),
            title: "Delete Bot".into(),
            // BOX-NEEDED: what deleting a bot takes with it, in the box's words (its conversation,
            // wake-ups, keys and folder, and where the folder goes, differ by computer): e.g.
            // `deletes` on each bot in `GET kuvo/bots`. Until then nothing is claimed here.
            detail: format!("Removes it from {}.", self.this_box),
            control: match self.busy("bot-delete") {
                Some(words) => Control::Busy(words.to_owned()),
                None => Control::Danger("Delete…".into(), Act::Delete),
            },
        };
        items.push(Item::Row { row: Box::new(delete), first: true, last: true });
        items.push(Item::End);
        items
    }

    /// What's held for `bot` on another computer, and what one refused of it, as the box says them.
    fn notices(&self, bot: &str) -> Vec<Item> {
        let mut items = Vec::new();
        if let Some(held) = self.held(bot) {
            let words = if held.delete {
                "It’s deleted once that computer is back.".to_owned()
            } else {
                // (Each field by the box's own name for it: name, look, job, personality, model.)
                let fields = held.fields();
                match fields.as_slice() {
                    [] => "Your changes go to it once that computer is back.".to_owned(),
                    fields => format!("Your changes to its {} go to it once that computer is back.", listed(fields)),
                }
            };
            items.push(Item::Notice { key: "bot-held".into(), tone: Tone::Waiting, title: format!("Waiting for {}", held.computer), words });
        }
        for (ix, refused) in self.refused(bot).into_iter().enumerate() {
            // The box's own words for why, as they are.
            items.push(Item::Notice { key: format!("bot-refused-{ix}"), tone: Tone::Refused, title: format!("{} didn’t take a change", refused.computer), words: refused.message.clone() });
        }
        items
    }

    fn setting_rows(&self, bot: &Bot) -> Vec<Row> {
        let busy_or = |key: &str, control: Control| match self.busy(key) {
            Some(words) => Control::Busy(words.to_owned()),
            None => control,
        };
        let mut rows = vec![
            Row { key: "bot-name".into(), title: "Name".into(), detail: bot.name.clone(), control: Control::Button("Rename…".into(), Act::Rename) },
            Row { key: "bot-look".into(), title: "Look".into(), detail: "Its colour and shape, wherever it shows.".into(), control: Control::Face(Some(bot.look.clone()), "Change…".into(), Act::EditLook) },
            Row {
                key: "bot-job".into(),
                title: "Job".into(),
                detail: if bot.job.is_empty() { "None yet. Tell it in its chat, or write one here.".into() } else { bot.job.clone() },
                control: Control::Button("Edit…".into(), Act::EditJob),
            },
        ];
        // Its personality, among those the box offers (`kuvo/bots/templates`).
        let personality = match &self.templates {
            Reading::NotYet => (String::new(), Control::Busy("Loading…".into())),
            Reading::Failed(why) => (format!("The personalities couldn’t be read: {why}"), Control::Text(String::new())),
            Reading::Ready(templates) => {
                let has = bot.personality.as_ref().and_then(|id| templates.personalities.iter().find(|each| &each.id == id));
                // One the box doesn't list (any more) shows by its id: nothing is made up for it.
                let shown = has.map(|each| each.name.clone()).or_else(|| bot.personality.clone()).unwrap_or_else(|| OWN_WAY.to_owned());
                let mut choices = vec![(OWN_WAY.to_owned(), String::new(), Act::SetPersonality(None), bot.personality.is_none())];
                choices.extend(templates.personalities.iter().map(|each| (each.name.clone(), each.description.clone(), Act::SetPersonality(Some(each.id.clone())), bot.personality.as_ref() == Some(&each.id))));
                (has.map(|each| each.description.clone()).unwrap_or_default(), Control::Menu { shown, choices })
            }
        };
        rows.push(Row { key: "bot-personality".into(), title: "Personality".into(), detail: if personality.0.is_empty() { "How it talks.".into() } else { personality.0 }, control: busy_or("bot-personality", personality.1) });
        // Its model, among those the box offers.
        let model = match (&self.offered, &bot.model) {
            (Reading::NotYet, _) => Control::Busy("Loading…".into()),
            (Reading::Ready(offered), model) => Control::Menu {
                shown: model.as_ref().map(|model| self.model_name(model)).unwrap_or_else(|| "None".into()),
                choices: offered.models.iter().filter(|each| each.is_offered()).map(|each| (each.name.clone(), String::new(), Act::SetModel(each.reference()), model.as_ref().is_some_and(|model| each.is(model)))).collect(),
            },
            (Reading::Failed(_), model) => Control::Text(model.as_ref().map(|model| self.model_name(model)).unwrap_or_default()),
        };
        let model_line = match &self.offered {
            Reading::Failed(why) => format!("The models couldn’t be read: {why}"),
            _ => "What it thinks with.".into(),
        };
        rows.push(Row { key: "bot-model".into(), title: "Model".into(), detail: model_line, control: busy_or("bot-model", model) });
        // Where it lives and works: the box's facts, read only.
        rows.push(Row { key: "bot-lives".into(), title: "Lives on".into(), detail: String::new(), control: Control::Text(bot.machine.clone().unwrap_or_else(|| self.this_box.clone())) });
        rows.push(Row { key: "bot-works".into(), title: "Works in".into(), detail: String::new(), control: Control::Text(bot.project.as_deref().map(|project| self.project_name(project)).unwrap_or_else(|| "A folder of its own".into())) });
        rows
    }

    fn elsewhere_items(&self, id: &str) -> Vec<Item> {
        let (held, refused) = (self.held(id), self.refused(id));
        let Some(computer) = held.map(|held| held.computer.clone()).or_else(|| refused.first().map(|refused| refused.computer.clone())) else {
            // Nothing waits for it any more: it went through (or was let go).
            return vec![Item::Note { key: "elsewhere-done".into(), words: "Nothing is waiting for this bot any more.".into(), busy: false }, Item::End];
        };
        let mut items = vec![Item::ElsewhereHead {
            key: id.to_owned(),
            name: elsewhere_name(held, &computer),
            look: held.and_then(Held::look),
            line: format!("Lives on {computer}"),
        }];
        items.extend(self.notices(id));
        // BOX-NEEDED: the bots of your other computers as a list a client can draw and change
        // (name, look, job, where each lives), e.g. `GET kuvo/bots?across=1`, as `kuvo/computers`
        // lists the computers themselves. `kuvo/bots` lists only the bots of the box asked, so a bot
        // living elsewhere is known here only by what's held for it.
        items.push(Item::Note { key: "elsewhere-note".into(), words: format!("This window shows the bots that live on {}. This one’s page is on {computer}.", self.this_box), busy: false });
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
        let starting = templates.starting().map(|model| self.model_name(model));
        let model_shown = match (&self.form.model, &starting) {
            (Some(model), _) => self.model_name(model),
            (None, Some(starting)) => starting.clone(),
            (None, None) => "Picked for you".into(),
        };
        let mut models = vec![(starting.clone().unwrap_or_else(|| "Picked for you".into()), format!("What a new bot starts on, on {}", self.this_box), Act::FormModel(None), self.form.model.is_none())];
        if let Some(offered) = self.offered() {
            models.extend(offered.models.iter().filter(|each| each.is_offered()).map(|each| (each.name.clone(), String::new(), Act::FormModel(Some(each.reference())), self.form.model.as_ref().is_some_and(|model| each.is(model)))));
        }
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
                control: Control::Menu { shown: has.map(|each| each.name.clone()).or(personality).unwrap_or_else(|| OWN_WAY.to_owned()), choices },
            },
            Row { key: "new-model".into(), title: "Model".into(), detail: "What it thinks with.".into(), control: Control::Menu { shown: model_shown, choices: models } },
        ];
        if !templates.projects.is_empty() {
            let mut projects = vec![("A folder of its own".to_owned(), String::new(), Act::FormProject(None), self.form.project.is_none())];
            projects.extend(templates.projects.iter().map(|project| (self.project_name(&project.directory), String::new(), Act::FormProject(Some(project.directory.clone())), self.form.project.as_ref() == Some(&project.directory))));
            rows.push(Row {
                key: "new-project".into(),
                title: "Works in".into(),
                detail: "Its own folder, or one of your projects.".into(),
                control: Control::Menu { shown: self.form.project.as_deref().map(|project| self.project_name(project)).unwrap_or_else(|| "A folder of its own".into()), choices: projects },
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
        self.run(
            "create",
            move |client| client.make_bot(&new),
            |this, made, cx| {
                this.form.creating = false;
                match made {
                    Ok(made) => {
                        cx.emit(BotsEvent::Changed);
                        // On to its conversation: from a template its job waits in the composer,
                        // to edit and send; otherwise it writes first.
                        cx.emit(BotsEvent::Chat { bot: made.bot.id.clone(), session: made.bot.session.clone(), draft: made.first_message.filter(|draft| !draft.is_empty()) });
                    }
                    Err(error) => this.form.error = Some(said(&error)),
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

    /// What a control asked for.
    pub fn act(&mut self, act: Act, cx: &mut Context<Self>) {
        match act {
            Act::Chat => {
                if let Some(bot) = self.bot() {
                    cx.emit(BotsEvent::Chat { bot: bot.id.clone(), session: bot.session.clone(), draft: None });
                }
            }
            Act::OpenSession(session) => cx.emit(BotsEvent::Session(session)),
            Act::ShowAll => self.all_sessions = true,
            Act::Rename => {
                let Some(name) = self.bot().map(|bot| bot.name.clone()) else { return };
                self.name.update(cx, |field, cx| {
                    field.set_error(None, cx);
                    let end = name.len();
                    field.set_text(name, cx);
                    field.select_range(0..end, cx);
                });
                self.open_sheet(Sheet::Name, Some(self.name.focus_handle(cx)));
            }
            Act::EditJob => {
                let Some((line, details)) = self.bot().map(|bot| (bot.job.clone(), bot.job_details.clone())) else { return };
                self.line.update(cx, |field, cx| field.set_text(line, cx));
                self.details.update(cx, |field, cx| field.set_text(details, cx));
                self.open_sheet(Sheet::Job, Some(self.line.focus_handle(cx)));
            }
            Act::EditLook => {
                let Some(look) = self.bot().map(|bot| bot.look.clone()) else { return };
                self.open_sheet(Sheet::Look { look, new: false }, None);
            }
            Act::FormLook => {
                // From the look shown; with none, the first of each the box offers.
                let look = self.form_look().or_else(|| {
                    let looks = &self.templates()?.looks;
                    Some(Look { color: looks.colors.first()?.clone(), shape: looks.shapes.first()?.clone(), face: looks.faces.first()?.clone() })
                });
                let Some(look) = look else { return };
                self.open_sheet(Sheet::Look { look, new: true }, None);
            }
            Act::SetPersonality(personality) => self.change("bot-personality", BotChange { personality: Some(personality), ..BotChange::default() }, cx),
            Act::SetModel(model) => self.change("bot-model", BotChange { model: Some(model), ..BotChange::default() }, cx),
            Act::Delete => self.open_sheet(Sheet::Delete, None),
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

    fn open_sheet(&mut self, sheet: Sheet, focus: Option<FocusHandle>) {
        self.sheet = Some(sheet);
        self.sheet_error = None;
        self.saving = false;
        self.problem = None;
        self.wants_focus = Some(focus.unwrap_or_else(|| self.sheet_focus.clone()));
    }

    pub fn close_sheet(&mut self, cx: &mut Context<Self>) {
        // (One whose change is on its way stays until the box answers.)
        if self.saving {
            return;
        }
        self.sheet = None;
        self.sheet_error = None;
        self.wants_focus = Some(self.focus.clone());
        cx.notify();
    }

    /// Return or Escape in one of a sheet's fields.
    fn sheet_key(&mut self, event: &TextFieldEvent, cx: &mut Context<Self>) {
        match event {
            TextFieldEvent::Submit(_) => self.save_sheet(cx),
            TextFieldEvent::Cancel => self.close_sheet(cx),
            TextFieldEvent::Changed => {}
        }
    }

    /// Picks a colour or a shape in the look's sheet.
    pub fn pick_look(&mut self, color: Option<&str>, shape: Option<&str>, cx: &mut Context<Self>) {
        if let Some(Sheet::Look { look, .. }) = &mut self.sheet {
            if let Some(color) = color {
                look.color = color.to_owned();
            }
            if let Some(shape) = shape {
                look.shape = shape.to_owned();
            }
            cx.notify();
        }
    }

    /// The sheet's Save (or Delete): sent to the box, the sheet staying until it answers.
    pub fn save_sheet(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(sheet) = self.sheet.clone() else { return };
        let change = match sheet {
            // The new bot's look: kept for when it's made.
            Sheet::Look { look, new: true } => {
                self.form.look = Some(look);
                self.close_sheet(cx);
                self.rebuild(cx);
                return;
            }
            Sheet::Name => Some(BotChange { name: Some(self.name.read(cx).text().trim().to_owned()), ..BotChange::default() }),
            Sheet::Job => Some(BotChange { job: Some((self.line.read(cx).text().trim().to_owned(), self.details.read(cx).text().trim().to_owned())), ..BotChange::default() }),
            Sheet::Look { look, new: false } => Some(BotChange { look: Some(look), ..BotChange::default() }),
            Sheet::Delete => None,
        };
        let Some(bot) = self.bot().map(|bot| bot.id.clone()) else { return };
        self.saving = true;
        self.sheet_error = None;
        let id = bot.clone();
        self.run(
            "sheet",
            move |client| match &change {
                Some(change) => client.change_bot(&id, change),
                None => client.delete_bot(&id),
            },
            move |this, outcome, cx| {
                this.saving = false;
                let deleting = this.sheet == Some(Sheet::Delete);
                match outcome {
                    Ok(outcome) => {
                        this.sheet = None;
                        this.wants_focus = Some(this.focus.clone());
                        this.took(&bot, outcome, deleting, cx);
                    }
                    Err(error) => this.sheet_error = Some(said(&error)),
                }
            },
            cx,
        );
        self.rebuild(cx);
    }

    /// A row's own change (a menu's pick): busy while it goes, then the box's answer, or its
    /// refusal over the page.
    fn change(&mut self, key: &str, change: BotChange, cx: &mut Context<Self>) {
        let Some(bot) = self.bot().map(|bot| bot.id.clone()) else { return };
        self.problem = None;
        self.set_busy(key, "Saving…");
        let (key, id) = (key.to_owned(), bot.clone());
        self.run(
            key.clone(),
            move |client| client.change_bot(&id, &change),
            move |this, outcome, cx| {
                this.done(&key);
                match outcome {
                    Ok(outcome) => this.took(&bot, outcome, false, cx),
                    Err(error) => this.problem = Some(said(&error)),
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
                        now.now = listed.now.clone();
                        *listed = *now;
                    }
                }
                Outcome::Done(None) if deleted => {
                    said.bots.retain(|each| each.id != bot);
                    cx.emit(BotsEvent::Select(None));
                }
                Outcome::Done(None) => {}
                Outcome::Held(held) => {
                    said.held.retain(|each| each.bot != bot);
                    said.held.push(held);
                }
            }
        }
        // The sidebar's list, and whatever else changed with it, as the box says it now.
        cx.emit(BotsEvent::Changed);
    }
}

/// How a personality left to the model is named.
const OWN_WAY: &str = "The model’s own way";

/// "name", "name and look", "name, look and job".
fn listed<S: AsRef<str>>(words: &[S]) -> String {
    match words {
        [] => String::new(),
        [one] => one.as_ref().to_owned(),
        [most @ .., last] => format!("{} and {}", most.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(", "), last.as_ref()),
    }
}

/// What a bot that lives elsewhere is called here: the name held for it, where what's held names one.
pub fn elsewhere_name(held: Option<&Held>, computer: &str) -> String {
    // BOX-NEEDED: each `held` and `refused` entry in `GET kuvo/bots` should carry the bot's name
    // and look as the box last knew them (it has them from its peers' agents), so a bot waiting on
    // another computer shows as itself. Until then: the name being given, else a neutral one.
    held.and_then(Held::name).map(str::to_owned).unwrap_or_else(|| format!("A bot on {computer}"))
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
}
