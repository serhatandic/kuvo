//! The Marketplace (#750): apps to connect, ready-made agents and skills, free, for the whole
//! setup. The box serves all of it (`kuvo/marketplace`, PROTOCOL.md "Marketplace"): the catalog,
//! its categories and counts, what's added and connected, each logo, and the words of every action
//! and form. This view draws what it says, by area (the sidebar: Apps, Agents, Skills, and the
//! area's categories) as a calm two-column catalog, and sends what you do from sheets over it:
//! Connect, Add an agent, a skill's preview, and the box's forms.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, Image, IntoElement, KeyDownEvent, Render,
    SharedString, Task, Window, canvas, div, hsla, img, prelude::*, px,
};
use kuvo_box::BoxClient;
use kuvo_box::areas::Look;
use kuvo_box::marketplace::{
    AddAgent, AgentDetails, AgentOptions, AppDetails, Connected, FormField, Market, MarketAction, MarketArea, MarketForm, MarketItem,
    MarketQuery, SignIn, SkillDetails, SkillFile, SkillPreview,
};
use kuvo_box::typed::ModelRef;
use kuvo_ui::menu::{Menu, MenuEntry, PopoverMenu, Tooltip};
use kuvo_ui::small::Switch;
use kuvo_ui::text_field::{TextField, TextFieldEvent};
use kuvo_ui::theme::{MONO_FONT, radius, space};
use kuvo_ui::{ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TextRole, TypeSize};

use crate::areas::{Reachability, Reading};

/// The Marketplace's three areas, as its sidebar lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Apps,
    Agents,
    Skills,
}

impl Section {
    pub const ALL: [Section; 3] = [Section::Apps, Section::Agents, Section::Skills];

    /// Its id in the box's answer.
    pub fn id(self) -> &'static str {
        match self {
            Section::Apps => "apps",
            Section::Agents => "agents",
            Section::Skills => "skills",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Section::Apps => "Apps",
            Section::Agents => "Agents",
            Section::Skills => "Skills",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Section::Apps => IconName::Grid,
            Section::Agents => IconName::Bot,
            Section::Skills => IconName::Sparkle,
        }
    }
}

/// A bot made from an agent on this window's computer: the workspace opens its conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BotMade {
    pub bot: String,
    pub session: String,
}

impl EventEmitter<BotMade> for MarketplaceView {}

#[derive(Clone)]
enum Logo {
    Reading,
    Ready(Arc<Image>),
    None,
}

/// What a form the box describes does when it's sent.
#[derive(Clone, Debug, PartialEq)]
enum FormFor {
    Custom,
    Create,
    Edit { id: String, was: Option<String> },
    Place,
    Preview,
}

struct FormSheet {
    form: MarketForm,
    fields: Vec<(FormField, Entity<TextField>)>,
    /// Each choice field's pick.
    picked: HashMap<String, String>,
    target: FormFor,
}

struct AppSheet {
    id: String,
    details: Reading<AppDetails>,
    /// The form connecting asks for (a token), as its details describe it.
    fields: Vec<(FormField, Entity<TextField>)>,
    sign_in: Option<SignIn>,
    callback: Entity<TextField>,
    /// The Add sheet it was opened from, to go back to once connected.
    then: Option<Box<Sheet>>,
}

struct AgentSheet {
    id: String,
    details: Reading<AgentDetails>,
    computer: Option<String>,
    options: Reading<AgentOptions>,
    model: Option<ModelRef>,
    /// The routines turned on (each starts off).
    on: HashSet<String>,
    name: Entity<TextField>,
    request: String,
}

struct PreviewSheet {
    preview: Reading<SkillPreview>,
    file: usize,
    /// The revision an update replaces.
    was: Option<String>,
}

struct SkillSheet {
    id: String,
    details: Reading<SkillDetails>,
    file: usize,
}

enum Sheet {
    App(AppSheet),
    Form(FormSheet),
    Agent(AgentSheet),
    Preview(PreviewSheet),
    Skill(SkillSheet),
}

/// The catalog's widest; two columns down to this width, one below it.
const MAX_W: f32 = 880.;
const TWO_COLUMNS: f32 = 620.;
/// How many of a category's items "All" shows before "See all".
const FIRST: usize = 6;

pub struct MarketplaceView {
    client: BoxClient,
    visible: bool,
    section: Section,
    category: Option<String>,
    origin: Option<String>,
    search: Entity<TextField>,
    words: String,
    market: Reading<Market>,
    reachability: Reachability,
    reading: Option<Task<()>>,
    typing: Option<Task<()>>,
    logos: HashMap<String, Logo>,
    sheet: Option<Sheet>,
    sheet_focus: FocusHandle,
    sheet_problem: Option<String>,
    working: bool,
    /// What went wrong with the last thing you did on the page, until the next.
    problem: Option<String>,
    /// The catalog's width as last drawn: one column or two.
    width: Rc<Cell<f32>>,
    /// A sign-in being followed until it finishes.
    following: Option<Task<()>>,
    /// A sheet's action asked first, in the box's words, over its sheet.
    confirming: Option<MarketAction>,
    confirm_focus: FocusHandle,
}

impl MarketplaceView {
    pub fn new(client: BoxClient, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextField::new(cx).bare().submits_on_enter(true).placeholder("Search"));
        cx.subscribe_in(&search, window, |this, field, event: &TextFieldEvent, window, cx| match event {
            TextFieldEvent::Changed => {
                let words = field.read(cx).text().to_owned();
                this.typed(words, cx);
            }
            TextFieldEvent::Submit(words) => {
                if this.section == Section::Skills && is_link(words) {
                    this.preview_from(words.trim().to_owned(), window, cx);
                }
            }
            TextFieldEvent::Cancel => {}
        })
        .detach();
        Self {
            client,
            visible: false,
            section: Section::Apps,
            category: None,
            origin: None,
            search,
            words: String::new(),
            market: Reading::NotYet,
            reachability: Reachability::Unanswered,
            reading: None,
            typing: None,
            logos: HashMap::new(),
            sheet: None,
            sheet_focus: cx.focus_handle(),
            sheet_problem: None,
            working: false,
            problem: None,
            width: Rc::new(Cell::new(MAX_W)),
            following: None,
            confirming: None,
            confirm_focus: cx.focus_handle(),
        }
    }

    pub(crate) fn reachability(&self) -> Reachability {
        self.reachability
    }

    /// Shown or not: read as it's shown (after the first time, what it said last shows at once).
    pub fn shown(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if visible {
            self.read_now(cx);
        }
    }

    pub fn read_again(&mut self, cx: &mut Context<Self>) {
        if self.visible {
            self.read_now(cx);
        }
    }

    /// The window's title: the area, beside "Marketplace".
    pub fn title(&self) -> (SharedString, Option<SharedString>) {
        (self.section.label().into(), Some("Marketplace".into()))
    }

    pub fn section(&self) -> Section {
        self.section
    }

    /// Opens an area (Agents, from New Bot), from the top.
    pub fn show_section(&mut self, section: Section, cx: &mut Context<Self>) {
        self.pick_section(section, cx);
    }

    fn query(&self) -> MarketQuery {
        let words = if self.section == Section::Skills && is_link(&self.words) { String::new() } else { self.words.clone() };
        MarketQuery { words, category: self.category.clone(), origin: self.origin.clone() }
    }

    fn read_now(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let query = self.query();
        let read = cx.background_executor().spawn(async move { client.marketplace(&query) });
        self.reading = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            this.update(cx, |this, cx| this.take(result, cx)).ok();
        }));
    }

    fn take(&mut self, result: kuvo_box::Result<Market>, cx: &mut Context<Self>) {
        match result {
            Ok(market) => {
                self.reachability = Reachability::Reached;
                if !market.search_placeholder.is_empty() && self.search.read(cx).placeholder_text() != market.search_placeholder {
                    let words = market.search_placeholder.clone();
                    self.search.update(cx, |field, _| field.set_placeholder(words));
                }
                self.market = Reading::Ready(market);
            }
            Err(error) => {
                self.reachability = Reachability::Unreachable;
                // (What it said last stays while it can't say more.)
                if !matches!(self.market, Reading::Ready(_)) {
                    self.market = Reading::Failed(kuvo_box::accounts::said(&error));
                }
            }
        }
        cx.notify();
    }

    fn typed(&mut self, words: String, cx: &mut Context<Self>) {
        if words == self.words {
            return;
        }
        self.words = words;
        self.typing = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            this.update(cx, |this, cx| {
                this.problem = None;
                this.read_now(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    fn pick_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if self.section == section && self.category.is_none() {
            return;
        }
        self.section = section;
        self.category = None;
        self.origin = None;
        self.problem = None;
        if self.visible {
            self.read_now(cx);
        }
        cx.notify();
    }

    fn pick_category(&mut self, category: Option<String>, cx: &mut Context<Self>) {
        if self.category == category {
            return;
        }
        self.category = category;
        self.read_now(cx);
        cx.notify();
    }

    fn pick_origin(&mut self, origin: Option<String>, cx: &mut Context<Self>) {
        if self.origin == origin {
            return;
        }
        self.origin = origin;
        self.read_now(cx);
        cx.notify();
    }

    fn area(&self) -> Option<&MarketArea> {
        match &self.market {
            Reading::Ready(market) => market.area(self.section.id()),
            _ => None,
        }
    }

    /// An app as the Apps area lists it (its logo, its state), wherever it's named.
    fn app_item(&self, id: &str) -> Option<&MarketItem> {
        match &self.market {
            Reading::Ready(market) => market.area("apps").and_then(|area| area.items.iter().find(|item| item.id == id)),
            _ => None,
        }
    }

    fn logo(&mut self, route: &str, cx: &mut Context<Self>) -> Logo {
        if let Some(logo) = self.logos.get(route) {
            return logo.clone();
        }
        self.logos.insert(route.to_owned(), Logo::Reading);
        let client = self.client.clone();
        let wanted = route.to_owned();
        let read = cx.background_executor().spawn(async move { client.marketplace_logo(&wanted).ok().and_then(|bytes| crate::pictures::small(bytes, 96)) });
        let route = route.to_owned();
        cx.spawn(async move |this, cx| {
            let image = read.await;
            this.update(cx, |this, cx| {
                this.logos.insert(route, image.map_or(Logo::None, Logo::Ready));
                cx.notify();
            })
            .ok();
        })
        .detach();
        Logo::Reading
    }

    /// Does `work` with the box, then `done` with what it answered.
    fn run<T: Send + 'static>(
        &mut self,
        work: impl FnOnce(BoxClient) -> kuvo_box::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, kuvo_box::Result<T>, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.working = true;
        self.sheet_problem = None;
        let client = self.client.clone();
        let task = cx.background_executor().spawn(async move { work(client) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.working = false;
                done(this, result, window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn close_sheet(&mut self, cx: &mut Context<Self>) {
        if let Some(Sheet::App(AppSheet { sign_in: Some(sign_in), id, .. })) = &self.sheet {
            let (client, id, attempt) = (self.client.clone(), id.clone(), sign_in.attempt.clone());
            cx.background_executor().spawn(async move { client.cancel_marketplace_sign_in(&id, &attempt) }).detach();
        }
        self.sheet = None;
        self.sheet_problem = None;
        self.following = None;
        self.confirming = None;
        self.working = false;
        cx.notify();
    }

    fn field(&self, field: &FormField, cx: &mut Context<Self>) -> Entity<TextField> {
        let placeholder = field.placeholder.clone().unwrap_or_default();
        let lines = field.kind.as_deref() == Some("lines");
        let masked = field.kind.as_deref() == Some("password");
        let (min, max) = if field.mono { (6, 14) } else { (3, 5) };
        let value = field.value.clone().unwrap_or_default();
        cx.new(|cx| {
            let mut text = TextField::new(cx).bare().placeholder(placeholder);
            if lines {
                text = text.multi_line(min, max);
            }
            if masked {
                text = text.masked();
            }
            if !value.is_empty() {
                text.set_text(value, cx);
            }
            text
        })
    }

    fn fields_of(&self, form: &MarketForm, cx: &mut Context<Self>) -> Vec<(FormField, Entity<TextField>)> {
        form.fields.iter().filter(|field| field.choices.is_empty()).map(|field| (field.clone(), self.field(field, cx))).collect()
    }

    fn open_form(&mut self, form: MarketForm, target: FormFor, window: &mut Window, cx: &mut Context<Self>) {
        let fields = self.fields_of(&form, cx);
        let picked = form.fields.iter().filter_map(|field| field.choices.first().map(|choice| (field.id.clone(), choice.id.clone()))).collect();
        let first = fields.first().map(|(_, field)| field.clone());
        self.sheet = Some(Sheet::Form(FormSheet { form, fields, picked, target }));
        self.sheet_problem = None;
        self.focus_sheet(first, window, cx);
    }

    fn focus_sheet(&self, field: Option<Entity<TextField>>, window: &mut Window, cx: &mut Context<Self>) {
        match field {
            Some(field) => window.focus(&field.read(cx).focus_handle(cx), cx),
            None => window.focus(&self.sheet_focus, cx),
        }
        cx.notify();
    }

    // ---- Apps ----

    fn open_app(&mut self, id: &str, then: Option<Box<Sheet>>, window: &mut Window, cx: &mut Context<Self>) {
        let callback = cx.new(|cx| TextField::new(cx).bare().placeholder("http://localhost:…"));
        self.sheet = Some(Sheet::App(AppSheet { id: id.to_owned(), details: Reading::NotYet, fields: Vec::new(), sign_in: None, callback, then }));
        self.sheet_problem = None;
        let wanted = id.to_owned();
        self.run(
            move |client| client.marketplace_app(&wanted),
            |this, read, window, cx| {
                let fields = match &read {
                    Ok(details) => details.form.as_ref().map(|form| this.fields_of(form, cx)).unwrap_or_default(),
                    Err(_) => Vec::new(),
                };
                let first = fields.first().map(|(_, field)| field.clone());
                if let Some(Sheet::App(sheet)) = &mut this.sheet {
                    sheet.details = match read {
                        Ok(details) => Reading::Ready(details),
                        Err(error) => Reading::Failed(kuvo_box::accounts::said(&error)),
                    };
                    sheet.fields = fields;
                }
                this.focus_sheet(first, window, cx);
            },
            window,
            cx,
        );
        window.focus(&self.sheet_focus, cx);
    }

    /// What the app sheet's button does: add it where it isn't yet, then connect (with the token
    /// typed, or a sign-in in the browser), or reconnect, disconnect or remove it.
    fn app_action(&mut self, action: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Sheet::App(sheet)) = &self.sheet else { return };
        let Reading::Ready(details) = &sheet.details else { return };
        let id = sheet.id.clone();
        let token = sheet.fields.iter().find(|(field, _)| field.id == "token").map(|(_, field)| field.read(cx).text().trim().to_owned());
        let installed = details.item.installed;
        let auth = details.auth.clone().unwrap_or_default();
        match action {
            "add" | "connect" => self.run(
                move |client| {
                    if !installed {
                        client.add_marketplace_app(&id)?;
                    }
                    if auth == "open" {
                        return Ok(Connected::Done { connection: Some("ready".into()) });
                    }
                    client.connect_marketplace_app(&id, token.as_deref().filter(|token| !token.is_empty()))
                },
                |this, connected, window, cx| match connected {
                    Ok(Connected::SignIn(sign_in)) => {
                        cx.open_url(&sign_in.opens);
                        this.follow_sign_in(sign_in, window, cx);
                        this.read_now(cx);
                    }
                    Ok(Connected::Done { connection }) if connection.as_deref() == Some("ready") => this.connected(window, cx),
                    Ok(Connected::Done { .. }) => {
                        this.read_now(cx);
                        if let Some(Sheet::App(sheet)) = &this.sheet {
                            let id = sheet.id.clone();
                            let then = None;
                            this.reopen_app(&id, then, window, cx);
                        }
                    }
                    Err(error) => this.sheet_problem = Some(kuvo_box::accounts::said(&error)),
                },
                window,
                cx,
            ),
            "disconnect" => self.run(move |client| client.disconnect_marketplace_app(&id), |this, done, window, cx| this.after_app(done, window, cx), window, cx),
            "remove" => self.run(
                move |client| client.remove_marketplace_app(&id),
                |this, done, _, cx| match done {
                    Ok(_) => {
                        this.close_sheet(cx);
                        this.read_now(cx);
                    }
                    Err(error) => this.sheet_problem = Some(kuvo_box::accounts::said(&error)),
                },
                window,
                cx,
            ),
            _ => {}
        }
    }

    fn reopen_app(&mut self, id: &str, then: Option<Box<Sheet>>, window: &mut Window, cx: &mut Context<Self>) {
        let then = then.or_else(|| match self.sheet.take() {
            Some(Sheet::App(sheet)) => sheet.then,
            other => {
                self.sheet = other;
                None
            }
        });
        self.open_app(id, then, window, cx);
    }

    fn after_app(&mut self, done: kuvo_box::Result<serde_json::Value>, window: &mut Window, cx: &mut Context<Self>) {
        match done {
            Ok(_) => {
                self.read_now(cx);
                if let Some(Sheet::App(sheet)) = &self.sheet {
                    let id = sheet.id.clone();
                    self.reopen_app(&id, None, window, cx);
                }
            }
            Err(error) => self.sheet_problem = Some(kuvo_box::accounts::said(&error)),
        }
    }

    /// Connected: back to the Add sheet it came from (its options read again), or closed.
    fn connected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.following = None;
        self.read_now(cx);
        let then = match self.sheet.take() {
            Some(Sheet::App(sheet)) => sheet.then,
            _ => None,
        };
        match then.map(|sheet| *sheet) {
            Some(Sheet::Agent(agent)) => {
                let computer = agent.computer.clone();
                self.sheet = Some(Sheet::Agent(agent));
                if let Some(computer) = computer {
                    self.read_options(computer, window, cx);
                }
            }
            _ => self.close_sheet(cx),
        }
        cx.notify();
    }

    /// Follows a sign-in in the browser until the box says it's done (or the sheet closes).
    fn follow_sign_in(&mut self, sign_in: SignIn, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Sheet::App(sheet)) = &mut self.sheet else { return };
        sheet.sign_in = Some(sign_in.clone());
        let id = sheet.id.clone();
        let client = self.client.clone();
        self.following = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(1500)).await;
                let (client, id, attempt) = (client.clone(), id.clone(), sign_in.attempt.clone());
                let state = cx.background_executor().spawn(async move { client.marketplace_sign_in(&id, &attempt) }).await;
                let finished = this
                    .update_in(cx, |this, window, cx| match state {
                        Ok(state) if state.state == "connected" => {
                            this.connected(window, cx);
                            true
                        }
                        Ok(_) => false,
                        Err(error) => {
                            this.sheet_problem = Some(kuvo_box::accounts::said(&error));
                            cx.notify();
                            true
                        }
                    })
                    .unwrap_or(true);
                if finished {
                    break;
                }
            }
        }));
    }

    /// Finishes a sign-in done in this Mac's browser for another computer, with the address it
    /// ended on.
    fn finish_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Sheet::App(sheet)) = &self.sheet else { return };
        let Some(sign_in) = &sheet.sign_in else { return };
        let (id, attempt) = (sheet.id.clone(), sign_in.attempt.clone());
        let address = sheet.callback.read(cx).text().trim().to_owned();
        self.run(
            move |client| client.finish_marketplace_sign_in(&id, &attempt, &address),
            |this, done, window, cx| match done {
                Ok(_) => this.connected(window, cx),
                Err(error) => this.sheet_problem = Some(kuvo_box::accounts::said(&error)),
            },
            window,
            cx,
        );
    }

    // ---- Agents ----

    fn open_agent(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = cx.new(|cx| TextField::new(cx).bare());
        self.sheet = Some(Sheet::Agent(AgentSheet {
            id: id.to_owned(),
            details: Reading::NotYet,
            computer: None,
            options: Reading::NotYet,
            model: None,
            on: HashSet::new(),
            name,
            request: new_request(),
        }));
        self.sheet_problem = None;
        let wanted = id.to_owned();
        self.run(
            move |client| client.marketplace_agent(&wanted),
            |this, read, window, cx| {
                let Some(Sheet::Agent(sheet)) = &mut this.sheet else { return };
                match read {
                    Ok(details) => {
                        let title = details.item.title.clone();
                        sheet.name.update(cx, |field, cx| field.set_text(title, cx));
                        let computer = details.computers.iter().find(|computer| computer.here).or(details.computers.first()).map(|computer| computer.key.clone());
                        sheet.details = Reading::Ready(details);
                        if let Some(computer) = computer {
                            this.read_options(computer, window, cx);
                        }
                    }
                    Err(error) => sheet.details = Reading::Failed(kuvo_box::accounts::said(&error)),
                }
            },
            window,
            cx,
        );
        window.focus(&self.sheet_focus, cx);
    }

    fn read_options(&mut self, computer: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Sheet::Agent(sheet)) = &mut self.sheet else { return };
        sheet.computer = Some(computer.clone());
        sheet.options = Reading::NotYet;
        let id = sheet.id.clone();
        self.run(
            move |client| client.agent_options(&id, &computer),
            |this, read, _, _| {
                let Some(Sheet::Agent(sheet)) = &mut this.sheet else { return };
                match read {
                    Ok(options) => {
                        // The model the box suggests, unless one's picked that's still offered.
                        let offered = |model: &ModelRef| options.offered.models.iter().any(|each| each.is_offered() && each.is(model));
                        if !sheet.model.as_ref().is_some_and(offered) {
                            sheet.model = options.offered.starting.clone().flatten().filter(offered).or_else(|| options.offered.models.iter().find(|model| model.is_offered()).map(|model| model.reference()));
                        }
                        sheet.on.retain(|id| options.routines.iter().any(|routine| &routine.id == id));
                        sheet.options = Reading::Ready(options);
                    }
                    Err(error) => sheet.options = Reading::Failed(kuvo_box::accounts::said(&error)),
                }
            },
            window,
            cx,
        );
    }

    fn add_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Sheet::Agent(sheet)) = &self.sheet else { return };
        let (Reading::Ready(details), Some(computer), Some(model)) = (&sheet.details, &sheet.computer, &sheet.model) else { return };
        let add = AddAgent {
            computer: computer.clone(),
            model: ModelRef { variant: None, ..model.clone() },
            version: details.item.version.clone().unwrap_or_default(),
            name: sheet.name.read(cx).text().trim().to_owned(),
            routines: details.item.routines.iter().filter(|routine| sheet.on.contains(&routine.id)).map(|routine| routine.id.clone()).collect(),
            request: sheet.request.clone(),
        };
        let here = details.computers.iter().any(|each| each.here && &each.key == computer);
        let id = sheet.id.clone();
        self.run(
            move |client| client.add_marketplace_agent(&id, &add),
            move |this, made, _, cx| match made {
                Ok(made) => {
                    this.close_sheet(cx);
                    this.read_now(cx);
                    if here {
                        cx.emit(BotMade { bot: made.bot.id.clone(), session: made.bot.session.clone() });
                    }
                }
                Err(error) => this.sheet_problem = Some(kuvo_box::accounts::said(&error)),
            },
            window,
            cx,
        );
    }

    // ---- Skills ----

    fn open_skill(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.sheet = Some(Sheet::Skill(SkillSheet { id: id.to_owned(), details: Reading::NotYet, file: 0 }));
        self.sheet_problem = None;
        let wanted = id.to_owned();
        self.run(
            move |client| client.marketplace_skill(&wanted),
            |this, read, _, _| {
                if let Some(Sheet::Skill(sheet)) = &mut this.sheet {
                    sheet.details = match read {
                        Ok(details) => Reading::Ready(details),
                        Err(error) => Reading::Failed(kuvo_box::accounts::said(&error)),
                    };
                }
            },
            window,
            cx,
        );
        window.focus(&self.sheet_focus, cx);
    }

    /// A skill's preview from a GitHub link or a folder, read by the box, shown before installing.
    fn preview_from(&mut self, from: String, window: &mut Window, cx: &mut Context<Self>) {
        self.sheet = Some(Sheet::Preview(PreviewSheet { preview: Reading::NotYet, file: 0, was: None }));
        self.sheet_problem = None;
        self.run(move |client| client.preview_skill(&from), |this, read, _, _| this.previewed(read, None), window, cx);
        window.focus(&self.sheet_focus, cx);
    }

    fn previewed(&mut self, read: kuvo_box::Result<SkillPreview>, was: Option<String>) {
        let preview = match read {
            Ok(preview) => Reading::Ready(preview),
            Err(error) => Reading::Failed(kuvo_box::accounts::said(&error)),
        };
        self.sheet = Some(Sheet::Preview(PreviewSheet { preview, file: 0, was }));
    }

    fn skill_action(&mut self, action: &MarketAction, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Sheet::Skill(sheet)) = &self.sheet else { return };
        let Reading::Ready(details) = &sheet.details else { return };
        let (id, revision) = (sheet.id.clone(), details.revision.clone());
        match action.id.as_str() {
            "add" => {
                let Some(available) = details.available.clone() else { return };
                self.run(move |client| client.add_catalog_skill(&id, &available, None), |this, done, _, cx| this.after_skill(done, cx), window, cx);
            }
            "remove" => self.run(move |client| client.remove_skill(&id, revision.as_deref()), |this, done, _, cx| this.after_skill(done, cx), window, cx),
            "edit" => {
                if let Some(form) = action.form.clone() {
                    self.open_form(form, FormFor::Edit { id, was: revision }, window, cx);
                }
            }
            "update-preview" => self.run(move |client| client.preview_skill_update(&id), move |this, read, _, _| this.previewed(read, revision), window, cx),
            _ => {}
        }
    }

    fn after_skill(&mut self, done: kuvo_box::Result<serde_json::Value>, cx: &mut Context<Self>) {
        match done {
            Ok(_) => {
                self.close_sheet(cx);
                self.read_now(cx);
            }
            Err(error) => self.sheet_problem = Some(kuvo_box::accounts::said(&error)),
        }
    }

    fn install_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Sheet::Preview(PreviewSheet { preview: Reading::Ready(preview), was, .. })) = &self.sheet else { return };
        let (id, was) = (preview.preview.clone(), was.clone());
        self.run(
            move |client| client.install_skill(&id, was.as_deref()),
            |this, done, _, cx| {
                if done.is_ok() {
                    // (What was pasted to find it has done its work.)
                    this.search.update(cx, |field, cx| field.set_text("", cx));
                    this.words.clear();
                }
                this.after_skill(done, cx);
            },
            window,
            cx,
        );
    }

    fn switch_skill(&mut self, id: String, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        // (Shown switched at once; what the box says next is what stays.)
        if let Reading::Ready(market) = &mut self.market
            && let Some(item) = market.areas.iter_mut().flat_map(|area| area.items.iter_mut()).find(|item| item.id == id && item.enabled.is_some())
        {
            item.enabled = Some(on);
        }
        self.run(
            move |client| client.switch_skill(&id, on),
            |this, done, _, cx| {
                if let Err(error) = done {
                    this.problem = Some(kuvo_box::accounts::said(&error));
                }
                this.read_now(cx);
            },
            window,
            cx,
        );
    }

    fn page_action(&mut self, work: impl FnOnce(BoxClient) -> kuvo_box::Result<serde_json::Value> + Send + 'static, window: &mut Window, cx: &mut Context<Self>) {
        self.problem = None;
        self.run(
            work,
            |this, done, _, cx| {
                if let Err(error) = done {
                    this.problem = Some(kuvo_box::accounts::said(&error));
                }
                this.read_now(cx);
            },
            window,
            cx,
        );
    }

    fn submit_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Sheet::Form(sheet)) = &self.sheet else { return };
        let mut values = serde_json::Map::new();
        for (field, text) in &sheet.fields {
            values.insert(field.id.clone(), serde_json::Value::String(text.read(cx).text().to_owned()));
        }
        for (id, picked) in &sheet.picked {
            values.insert(id.clone(), serde_json::Value::String(picked.clone()));
        }
        let text = |id: &str| values.get(id).and_then(|value| value.as_str()).unwrap_or_default().to_owned();
        match sheet.target.clone() {
            FormFor::Custom => self.run(move |client| client.add_custom_app(&values), |this, done, _, cx| this.after_skill(done, cx), window, cx),
            FormFor::Create => {
                let (name, description, instructions) = (text("name"), text("description"), text("instructions"));
                self.run(move |client| client.create_skill(&name, &description, &instructions), |this, done, _, cx| this.after_skill(done, cx), window, cx);
            }
            FormFor::Edit { id, was } => {
                let (description, instructions) = (text("description"), text("instructions"));
                self.run(move |client| client.edit_skill(&id, &description, &instructions, was.as_deref()), |this, done, _, cx| this.after_skill(done, cx), window, cx);
            }
            FormFor::Place => {
                let place = values.values().next().and_then(|value| value.as_str()).unwrap_or_default().to_owned();
                self.run(move |client| client.add_skill_place(&place), |this, done, _, cx| this.after_skill(done, cx), window, cx);
            }
            FormFor::Preview => {
                let from = values.values().next().and_then(|value| value.as_str()).unwrap_or_default().trim().to_owned();
                self.preview_from(from, window, cx);
            }
        }
    }

    /// What a row does when it's pressed: its sheet.
    fn open_item(&mut self, section: Section, item: &MarketItem, window: &mut Window, cx: &mut Context<Self>) {
        match section {
            Section::Apps => self.open_app(&item.id, None, window, cx),
            Section::Agents => self.open_agent(&item.id, window, cx),
            Section::Skills if item.origin.as_deref() != Some("found") || !item.actions.is_empty() => self.open_skill(&item.id, window, cx),
            Section::Skills => {}
        }
    }

    // ---- The sidebar ----

    /// The sidebar beside the rail while the Marketplace is open: its areas, then the area's
    /// categories (once the box has said them).
    pub fn sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let dark = cx.theme().appearance.is_dark();
        let row = |id: SharedString, icon: Option<IconName>, label: String, count: Option<usize>, selected: bool, cx: &mut Context<Self>| {
            div()
                .id(id.clone())
                .debug_selector(move || id.to_string())
                .h(px(32.))
                .px(px(8.))
                .rounded(radius::NAV)
                .cursor_pointer()
                .flex()
                .items_center()
                .gap(space::S2)
                .when(selected, |d| {
                    d.bg(if dark { hsla(0., 0., 1., 0.105) } else { c.selected_card })
                        .border_1()
                        .border_color(if dark { hsla(0., 0., 1., 0.14) } else { c.selected_edge })
                        .shadow(cx.theme().selected_shadow())
                })
                .when(!selected, |d| d.hover(|s| s.bg(c.hover)))
                .children(icon.map(|icon| div().size(px(20.)).flex_none().flex().items_center().justify_center().child(Icon::new(icon).size(IconSize::Nav).color(if selected { c.icon_on } else { c.icon }))))
                .child(div().flex_1().min_w_0().truncate().text_role(TextRole::RowLabel).when(selected, |d| d.medium().text_color(c.text)).when(!selected, |d| d.text_color(c.text_muted)).child(label))
                .children(count.map(|count| div().flex_none().text_role(TextRole::Meta).text_color(c.text_faint).child(count.to_string())))
        };
        let sections: Vec<AnyElement> = Section::ALL
            .into_iter()
            .map(|section| {
                let selected = self.section == section;
                row(format!("market-section-{}", section.id()).into(), Some(section.icon()), section.label().into(), None, selected, cx)
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_section(section, cx)))
                    .into_any_element()
            })
            .collect();
        let categories: Vec<(String, usize)> = self.area().map(|area| area.categories.iter().map(|category| (category.title.clone(), category.count)).collect()).unwrap_or_default();
        let total = self.area().map(|area| area.count);
        let mut picks: Vec<AnyElement> = Vec::new();
        if let Some(total) = total.filter(|_| !categories.is_empty()) {
            picks.push(row("market-category-all".into(), None, "All".into(), Some(total), self.category.is_none(), cx).on_click(cx.listener(|this, _, _, cx| this.pick_category(None, cx))).into_any_element());
            for (title, count) in categories {
                let selected = self.category.as_deref() == Some(title.as_str());
                let picked = title.clone();
                picks.push(
                    row(format!("market-category-{}", slug(&title)).into(), None, title, Some(count), selected, cx)
                        .on_click(cx.listener(move |this, _, _, cx| this.pick_category(Some(picked.clone()), cx)))
                        .into_any_element(),
                );
            }
        }
        div()
            .id("market-sidebar")
            .size_full()
            .px(px(10.))
            .py(space::S3)
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(div().pl(px(8.)).h(px(34.)).mb(space::S1).flex().items_center().text_role(TextRole::PaneTitle).text_color(c.text).child("Marketplace"))
            .children(sections)
            .when(!picks.is_empty(), |d| {
                d.child(div().h(px(1.)).mx(space::S2).my(space::S2).bg(c.border))
                    .child(div().px(px(8.)).pb(space::S1).text_role(TextRole::Meta).text_color(c.text_faint).child("Categories"))
                    .child(div().id("market-categories").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(2.)).children(picks))
            })
            .into_any_element()
    }

    // ---- The page ----

    fn header(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let (title, line, failed) = match (&self.market, self.area()) {
            (Reading::Ready(_), Some(area)) => (area.title.clone(), area.line.clone(), false),
            (Reading::Failed(why), _) => (self.section.label().to_owned(), why.clone(), true),
            _ => (self.section.label().to_owned(), "Loading…".to_owned(), false),
        };
        let filters = self.area().map(|area| area.filters.clone()).unwrap_or_default();
        let actions = self.area().map(|area| area.actions.clone()).unwrap_or_default();
        let ready = self.area().is_some();
        let top = div()
            .flex()
            .items_end()
            .justify_between()
            .gap(space::S4)
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(space::S1)
                    .child(div().debug_selector(|| "market-title".into()).text_ui(TypeSize::Title).medium().text_color(c.text).child(title))
                    .child(div().debug_selector(|| "market-line".into()).text_ui(TypeSize::Small).text_color(c.text_muted).child(line)),
            )
            .when(!filters.is_empty(), |d| d.child(self.filters(filters, cx)));
        let search = div()
            .debug_selector(|| "market-search".into())
            .flex_1()
            .min_w_0()
            .h(px(34.))
            .px(space::S3)
            .rounded(radius::NAV)
            .bg(c.sunken)
            .border_1()
            .border_color(c.border)
            .flex()
            .items_center()
            .gap(space::S2)
            .child(Icon::new(IconName::Search).size(IconSize::Nav).color(c.text_faint))
            .child(div().flex_1().min_w_0().child(self.search.clone()));
        let buttons: Vec<AnyElement> = actions
            .into_iter()
            .map(|action| {
                let id = action.id.clone();
                Button::new(SharedString::from(format!("market-action-{id}")), action.title.clone())
                    .icon(if id == "create" || id == "custom" { IconName::Plus } else { IconName::Link })
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        if let Some(form) = action.form.clone() {
                            let target = match action.id.as_str() {
                                "custom" => FormFor::Custom,
                                "create" => FormFor::Create,
                                _ => FormFor::Preview,
                            };
                            this.open_form(form, target, window, cx);
                        }
                    }))
                    .into_any_element()
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(space::S4)
            .child(top)
            .when(failed, |d| d.child(div().child(Button::new("market-try-again", "Try Again").size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.read_now(cx))))))
            .when(ready, |d| d.child(div().flex().items_center().gap(space::S2).child(search).children(buttons)))
            .into_any_element()
    }

    /// All, then the box's own filters (Kuvo, Yours), as a flat segmented control.
    fn filters(&self, filters: Vec<MarketAction>, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let dark = cx.theme().appearance.is_dark();
        let chip = |id: Option<String>, label: String, this: &Self, cx: &mut Context<Self>| {
            let on = this.origin == id;
            let key = id.clone().unwrap_or_else(|| "all".into());
            div()
                .id(SharedString::from(format!("market-filter-{key}")))
                .debug_selector(move || format!("market-filter-{key}"))
                .h(px(24.))
                .px(space::S2)
                .rounded(radius::SMALL)
                .cursor_pointer()
                .flex()
                .items_center()
                .text_ui(TypeSize::Caption)
                .when(on, |d| {
                    d.medium()
                        .text_color(c.text)
                        .bg(if dark { hsla(0., 0., 1., 0.11) } else { hsla(0., 0., 1., 0.96) })
                        .border_1()
                        .border_color(if dark { hsla(0., 0., 1., 0.14) } else { hsla(0., 0., 0., 0.08) })
                        .shadow(cx.theme().card_shadow())
                })
                .when(!on, |d| d.text_color(c.text_muted).hover(|s| s.text_color(c.text)))
                .on_click(cx.listener(move |this, _, _, cx| this.pick_origin(id.clone(), cx)))
                .child(label)
                .into_any_element()
        };
        let mut chips = vec![chip(None, "All".into(), self, cx)];
        for filter in filters {
            chips.push(chip(Some(filter.id.clone()), filter.title.clone(), self, cx));
        }
        div()
            .flex_none()
            .rounded(radius::NAV)
            .bg(if dark { hsla(0., 0., 1., 0.04) } else { hsla(0., 0., 0., 0.04) })
            .border_1()
            .border_color(if dark { hsla(0., 0., 1., 0.08) } else { hsla(0., 0., 0., 0.06) })
            .p(px(2.))
            .flex()
            .items_center()
            .gap(px(2.))
            .children(chips)
            .into_any_element()
    }

    fn tile(&mut self, section: Section, item: &MarketItem, size: f32, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        if section == Section::Agents {
            return crate::bot_face::still_bot_face(&item.id, &agent_look(&item.id), px(size));
        }
        let frame = div().size(px(size)).flex_none().rounded(radius::NAV).bg(c.sunken).border_1().border_color(c.border).overflow_hidden().flex().items_center().justify_center();
        if let Some(route) = item.logo.clone() {
            return match self.logo(&route, cx) {
                Logo::Ready(image) => {
                    let id = item.id.clone();
                    frame.bg(hsla(0., 0., 1., 1.)).child(div().debug_selector(move || format!("market-logo-{id}")).size(px(size * 0.66)).child(img(image).size_full().object_fit(gpui::ObjectFit::Contain))).into_any_element()
                }
                _ => frame.into_any_element(),
            };
        }
        let icon = match (section, item.origin.as_deref()) {
            (Section::Apps, _) => IconName::Globe,
            (_, Some("github")) => IconName::Branch,
            (_, Some("found")) => IconName::Folder,
            _ => IconName::Sparkle,
        };
        frame.child(Icon::new(icon).size(IconSize::Nav).color(c.icon_on)).into_any_element()
    }

    fn status(&self, status: &kuvo_box::marketplace::Status, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        div()
            .flex_none()
            .h(px(24.))
            .flex()
            .items_center()
            .gap(space::S1_5)
            .child(div().size(px(6.)).rounded(radius::PILL).bg(if status.good() { c.success } else { c.warning }))
            .child(div().text_ui(TypeSize::Caption).text_color(if status.good() { c.text_muted } else { c.warning }).child(status.words.clone()))
            .into_any_element()
    }

    fn row(&mut self, section: Section, item: &MarketItem, switch_title: &str, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let tile = self.tile(section, item, 34., cx);
        let id = item.id.clone();
        let trailing: AnyElement = if let Some(status) = &item.status {
            let key = format!("market-status-{id}");
            div().debug_selector(move || key).child(self.status(status, cx)).into_any_element()
        } else if let Some(on) = item.enabled {
            let (skill, why) = (item.id.clone(), item.switch_why.clone());
            let tip: SharedString = why.clone().unwrap_or_else(|| switch_title.to_owned()).into();
            div()
                .id(SharedString::from(format!("market-switch-{id}")))
                .debug_selector({
                    let id = id.clone();
                    move || format!("market-switch-{id}")
                })
                .flex_none()
                .tooltip(move |_, cx| Tooltip::new(tip.clone()).build(cx))
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(Switch::new(SharedString::from(format!("market-switch-control-{id}")), on).disabled(why.is_some()).on_change({
                    let view = cx.entity().downgrade();
                    move |on, window, cx| {
                        view.update(cx, |this, cx| this.switch_skill(skill.clone(), on, window, cx)).ok();
                    }
                }))
                .into_any_element()
        } else if let Some(action) = item.actions.first() {
            let item = item.clone();
            Button::new(SharedString::from(format!("market-button-{id}")), action.title.clone())
                .size(ButtonSize::Small)
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.open_item(section, &item, window, cx);
                }))
                .into_any_element()
        } else {
            div().into_any_element()
        };
        // ("Yours" beside a name only where it's among Kuvo's: its own group says so already.)
        let yours = section != Section::Agents && item.is_yours() && item.origin.as_deref() != Some("found") && !item.category.starts_with("Your");
        let detail = item.detail.clone().or_else(|| item.location.clone());
        let pressable = !(section == Section::Skills && item.origin.as_deref() == Some("found") && item.actions.is_empty());
        let clicked = item.clone();
        div()
            .id(SharedString::from(format!("market-item-{id}")))
            .debug_selector(move || format!("market-item-{id}"))
            .min_h(px(60.))
            .py(space::S2)
            .px(space::S1)
            .border_b_1()
            .border_color(c.border)
            .flex()
            .items_center()
            .gap(space::S3)
            .when(pressable, |d| {
                d.cursor_pointer().hover(|s| s.bg(c.hover)).on_click(cx.listener(move |this, _, window, cx| this.open_item(section, &clicked, window, cx)))
            })
            .child(tile)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space::S1_5)
                            .min_w_0()
                            .child(div().min_w_0().truncate().text_role(TextRole::RowTitle).text_color(c.text).child(item.title.clone()))
                            .when(yours, |d| d.child(div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_faint).child("· Yours"))),
                    )
                    .child(div().truncate().text_role(TextRole::RowDetail).text_color(c.text_muted).child(item.summary.clone()))
                    .children(detail.map(|detail| div().truncate().text_ui(TypeSize::Caption).text_color(c.text_faint).child(detail))),
            )
            .child(trailing)
            .into_any_element()
    }

    /// Rows two to a line, each half the width (one to a line in a narrow window).
    fn rows(&mut self, section: Section, items: &[MarketItem], switch_title: &str, cx: &mut Context<Self>) -> AnyElement {
        let two = self.width.get() >= TWO_COLUMNS;
        let rows: Vec<AnyElement> = items.iter().map(|item| self.row(section, item, switch_title, cx)).collect();
        if !two {
            return div().flex().flex_col().children(rows).into_any_element();
        }
        let mut lines = Vec::new();
        let mut rows = rows.into_iter();
        while let Some(left) = rows.next() {
            let right = rows.next();
            lines.push(div().flex().items_stretch().gap(space::S6).child(div().flex_1().min_w_0().child(left)).child(div().flex_1().min_w_0().children(right)).into_any_element());
        }
        div().flex().flex_col().children(lines).into_any_element()
    }

    fn group(&mut self, title: String, count: Option<usize>, more: bool, body: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let picked = title.clone();
        let key = slug(&title);
        div()
            .debug_selector(move || format!("market-group-{key}"))
            .flex()
            .flex_col()
            .gap(space::S1)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pb(space::S1)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space::S2)
                            .child(div().text_ui(TypeSize::Body).medium().text_color(c.text).child(title))
                            .children(count.map(|count| div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(count.to_string()))),
                    )
                    .when(more, |d| {
                        d.child(
                            div()
                                .id(SharedString::from(format!("market-see-all-{}", slug(&picked))))
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .gap(px(2.))
                                .text_ui(TypeSize::Caption)
                                .text_color(c.text_muted)
                                .hover(|s| s.text_color(c.text))
                                .on_click(cx.listener(move |this, _, _, cx| this.pick_category(Some(picked.clone()), cx)))
                                .child("See all")
                                .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).color(c.text_faint)),
                        )
                    }),
            )
            .child(body)
            .into_any_element()
    }

    fn catalog(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(area) = self.area().cloned() else { return Vec::new() };
        let c = cx.theme().colors.clone();
        let section = self.section;
        let switch_title = area.here.as_ref().map(|here| here.switch_title.clone()).unwrap_or_default();
        let mut out = Vec::new();
        if area.items.is_empty() {
            out.push(div().debug_selector(|| "market-empty".into()).py(space::S6).text_ui(TypeSize::Small).text_color(c.text_muted).child(area.empty.clone()).into_any_element());
        } else if let Some(category) = self.category.clone() {
            let body = self.rows(section, &area.items, &switch_title, cx);
            out.push(self.group(category, None, false, body, cx));
        } else {
            let searching = !self.words.trim().is_empty();
            // (In the box's order.)
            for category in &area.categories {
                let items: Vec<MarketItem> = area.items.iter().filter(|item| item.category == category.title).cloned().collect();
                if items.is_empty() {
                    continue;
                }
                let more = !searching && items.len() > FIRST;
                let shown = if more { &items[..FIRST] } else { &items[..] };
                let body = self.rows(section, shown, &switch_title, cx);
                out.push(self.group(category.title.clone(), Some(items.len()), more, body, cx));
            }
        }
        // This computer's own skills: Recently Deleted and Other Places.
        let yours = self.category.as_deref().is_none_or(|category| category == "Your skills");
        if section == Section::Skills && yours && self.words.trim().is_empty() && let Some(here) = &area.here {
            if let Some(problem) = &here.problem {
                out.push(div().text_ui(TypeSize::Caption).text_color(c.text_muted).child(problem.clone()).into_any_element());
            }
            if let Some(deleted) = here.deleted.clone().filter(|deleted| !deleted.items.is_empty()) {
                let rows: Vec<AnyElement> = deleted
                    .items
                    .iter()
                    .map(|item| {
                        let id = item.id.clone();
                        self.plain_row(
                            format!("market-deleted-{}", slug(&item.id)),
                            item.title.clone(),
                            Some(item.summary.clone()),
                            Button::new(SharedString::from(format!("market-restore-{}", slug(&item.id))), deleted.action.title.clone())
                                .size(ButtonSize::Small)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let id = id.clone();
                                    this.page_action(move |client| client.restore_skill(&id), window, cx);
                                }))
                                .into_any_element(),
                            cx,
                        )
                    })
                    .collect();
                let body = div().flex().flex_col().gap(space::S1).child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(deleted.note.clone())).children(rows).into_any_element();
                out.push(self.group(deleted.title.clone(), None, false, body, cx));
            }
            if let Some(places) = here.places.clone() {
                let rows: Vec<AnyElement> = places
                    .items
                    .iter()
                    .map(|place| {
                        let index: usize = place.id.parse().unwrap_or(0);
                        self.plain_row(
                            format!("market-place-{index}"),
                            place.title.clone(),
                            None,
                            Button::new(SharedString::from(format!("market-place-remove-{index}")), places.remove.title.clone())
                                .size(ButtonSize::Small)
                                .on_click(cx.listener(move |this, _, window, cx| this.page_action(move |client| client.remove_skill_place(index), window, cx)))
                                .into_any_element(),
                            cx,
                        )
                    })
                    .collect();
                let add = places.add.clone();
                let body = div()
                    .flex()
                    .flex_col()
                    .gap(space::S1)
                    .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(places.note.clone()))
                    .children(rows)
                    .child(
                        div().pt(space::S2).child(
                            Button::new("market-place-add", add.title.clone()).size(ButtonSize::Small).icon(IconName::Plus).on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(form) = add.form.clone() {
                                    this.open_form(form, FormFor::Place, window, cx);
                                }
                            })),
                        ),
                    )
                    .into_any_element();
                out.push(self.group(places.title.clone(), None, false, body, cx));
            }
        }
        out
    }

    fn plain_row(&self, key: String, title: String, line: Option<String>, trailing: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        div()
            .debug_selector(move || key.clone())
            .py(space::S2)
            .px(space::S1)
            .border_b_1()
            .border_color(c.border)
            .flex()
            .items_center()
            .gap(space::S3)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(div().truncate().text_ui(TypeSize::Small).medium().text_color(c.text).child(title))
                    .children(line.filter(|line| !line.is_empty()).map(|line| div().truncate().text_role(TextRole::RowDetail).text_color(c.text_muted).child(line))),
            )
            .child(trailing)
            .into_any_element()
    }

    // ---- Sheets ----

    /// A sheet's card: its title and close, what's in it, and its foot (what went wrong, on the
    /// left; its buttons on the right).
    fn frame(&self, key: &'static str, width: f32, title: String, body: Vec<AnyElement>, buttons: Vec<AnyElement>, note: Option<String>, cx: &mut Context<Self>) -> AnyElement {
        self.frame_with(key, width, title, body, Vec::new(), buttons, note, cx)
    }

    /// The same, with buttons on the left of its foot too (what undoes: Disconnect, Remove).
    #[allow(clippy::too_many_arguments)]
    fn frame_with(&self, key: &'static str, width: f32, title: String, body: Vec<AnyElement>, left: Vec<AnyElement>, buttons: Vec<AnyElement>, note: Option<String>, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let problem = self.sheet_problem.clone();
        div()
            .id(key)
            .debug_selector(move || key.into())
            .track_focus(&self.sheet_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.close_sheet(cx);
                }
            }))
            .w(px(width))
            .max_w_full()
            .max_h(px(680.))
            .rounded(radius::CARD)
            .bg(c.elevated)
            .border_1()
            .border_color(c.border)
            .shadow(cx.theme().elevated_shadow())
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .px(space::S5)
                    .py(space::S4)
                    .border_b_1()
                    .border_color(c.border)
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_ui(TypeSize::Body).medium().text_color(c.text).child(title))
                    .child(Button::icon_only("market-sheet-close", IconName::Close, "Close").ghost().size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.close_sheet(cx)))),
            )
            .child(div().id("market-sheet-body").flex_1().min_h_0().overflow_y_scroll().px(space::S5).py(space::S4).flex().flex_col().gap(space::S4).children(body))
            .child(
                div()
                    .flex_none()
                    .px(space::S5)
                    .py(space::S3)
                    .border_t_1()
                    .border_color(c.border)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(space::S3)
                    .children((!left.is_empty()).then(|| div().flex_none().flex().items_center().gap(space::S2).children(left)))
                    .child(
                        div()
                            .debug_selector(|| "market-sheet-note".into())
                            .flex_1()
                            .min_w_0()
                            .text_ui(TypeSize::Caption)
                            .text_color(if problem.is_some() { c.danger } else { c.text_muted })
                            .children(problem.or(note)),
                    )
                    .child(div().flex_none().flex().items_center().gap(space::S2).children(buttons)),
            )
            .into_any_element()
    }

    fn cancel(&self, cx: &mut Context<Self>) -> AnyElement {
        Button::new("market-sheet-cancel", "Cancel").on_click(cx.listener(|this, _, _, cx| this.close_sheet(cx))).into_any_element()
    }

    fn label(&self, words: String, cx: &mut Context<Self>) -> AnyElement {
        div().text_ui(TypeSize::Caption).medium().text_color(cx.theme().colors.text_muted).child(words).into_any_element()
    }

    fn input(&self, key: String, field: &Entity<TextField>, mono: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        div()
            .debug_selector(move || key.clone())
            .min_h(px(34.))
            .px(space::S3)
            .py(px(7.))
            .rounded(radius::NAV)
            .bg(c.surface)
            .border_1()
            .border_color(c.border_strong)
            .text_ui(TypeSize::Small)
            .when(mono, |d| d.font_family(MONO_FONT))
            .child(field.clone())
            .into_any_element()
    }

    fn form_field(&self, field: &FormField, text: &Entity<TextField>, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        div()
            .flex()
            .flex_col()
            .gap(space::S1)
            .child(self.label(field.title.clone(), cx))
            .child(self.input(format!("market-field-{}", field.id), text, field.mono, cx))
            .children(field.line.clone().map(|line| div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(line)))
            .into_any_element()
    }

    fn heading(&self, tile: AnyElement, title: String, summary: String, extra: Option<String>, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        div()
            .flex()
            .items_center()
            .gap(space::S3)
            .child(tile)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().text_role(TextRole::RowTitle).text_color(c.text).child(title))
                    .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(summary))
                    .children(extra.map(|extra| div().truncate().text_ui(TypeSize::Caption).text_color(c.text_faint).child(extra))),
            )
            .into_any_element()
    }

    fn waiting(&self, words: &str, cx: &mut Context<Self>) -> AnyElement {
        let _ = cx;
        div().py(space::S4).flex().justify_center().child(kuvo_ui::small::LoadingLabel::new("market-sheet-loading", words.to_owned())).into_any_element()
    }

    fn form_sheet(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(Sheet::Form(sheet)) = &self.sheet else { return None };
        let c = cx.theme().colors.clone();
        let dark = cx.theme().appearance.is_dark();
        let mut body = Vec::new();
        for field in &sheet.form.fields {
            if field.choices.is_empty() {
                if let Some((_, text)) = sheet.fields.iter().find(|(each, _)| each.id == field.id) {
                    body.push(self.form_field(field, text, cx));
                }
                continue;
            }
            let picked = sheet.picked.get(&field.id).cloned();
            let choices: Vec<AnyElement> = field
                .choices
                .iter()
                .map(|choice| {
                    let on = picked.as_deref() == Some(choice.id.as_str());
                    let (field_id, choice_id) = (field.id.clone(), choice.id.clone());
                    div()
                        .id(SharedString::from(format!("market-choice-{}", choice.id)))
                        .debug_selector({
                            let id = choice.id.clone();
                            move || format!("market-choice-{id}")
                        })
                        .h(px(28.))
                        .px(space::S2)
                        .rounded(radius::SMALL)
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .text_ui(TypeSize::Caption)
                        .when(on, |d| d.medium().text_color(c.text).bg(if dark { hsla(0., 0., 1., 0.11) } else { hsla(0., 0., 1., 0.96) }).border_1().border_color(if dark { hsla(0., 0., 1., 0.14) } else { hsla(0., 0., 0., 0.08) }))
                        .when(!on, |d| d.text_color(c.text_muted))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(Sheet::Form(sheet)) = &mut this.sheet {
                                sheet.picked.insert(field_id.clone(), choice_id.clone());
                                cx.notify();
                            }
                        }))
                        .child(choice.title.clone())
                        .into_any_element()
                })
                .collect();
            body.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::S1)
                    .child(self.label(field.title.clone(), cx))
                    .child(div().flex().flex_wrap().gap(px(2.)).p(px(2.)).rounded(radius::NAV).bg(if dark { hsla(0., 0., 1., 0.04) } else { hsla(0., 0., 0., 0.04) }).children(choices))
                    .into_any_element(),
            );
        }
        let (title, submit) = (sheet.form.title.clone(), sheet.form.submit.clone());
        let width = if sheet.fields.iter().any(|(field, _)| field.mono) { 640. } else { 520. };
        let buttons = vec![
            self.cancel(cx),
            Button::new("market-sheet-submit", submit).primary().loading(self.working).disabled(self.working).on_click(cx.listener(|this, _, window, cx| this.submit_form(window, cx))).into_any_element(),
        ];
        Some(self.frame("market-form-sheet", width, title, body, buttons, None, cx))
    }

    fn app_sheet(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(Sheet::App(sheet)) = &self.sheet else { return None };
        let id = sheet.id.clone();
        let details = match &sheet.details {
            Reading::Ready(details) => Some(details.clone()),
            _ => None,
        };
        let failed = match &sheet.details {
            Reading::Failed(why) => Some(why.clone()),
            _ => None,
        };
        let fields = sheet.fields.clone();
        let sign_in = sheet.sign_in.clone();
        let callback = sheet.callback.clone();
        let c = cx.theme().colors.clone();
        let listed = self.app_item(&id).cloned().or_else(|| details.as_ref().map(|details| details.item.clone()));
        let mut body = Vec::new();
        if let Some(item) = &listed {
            let tile = self.tile(Section::Apps, item, 44., cx);
            body.push(self.heading(tile, item.title.clone(), item.summary.clone(), details.as_ref().and_then(|details| details.url.clone()), cx));
        }
        let mut buttons = vec![self.cancel(cx)];
        let mut left = Vec::new();
        match (&details, &failed) {
            (_, Some(why)) => body.push(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(why.clone()).into_any_element()),
            (None, _) => body.push(self.waiting("Loading…", cx)),
            (Some(details), _) => {
                if let Some(status) = &details.item.status {
                    body.push(self.status(status, cx));
                }
                if let Some(sign_in) = &sign_in {
                    body.push(
                        div()
                            .debug_selector(|| "market-signing-in".into())
                            .flex()
                            .items_center()
                            .gap(space::S2)
                            .child(kuvo_ui::small::spinner("market-sign-in-spinner", IconSize::XSmall, c.text_muted))
                            .child(div().text_ui(TypeSize::Small).text_color(c.text).child(sign_in.title.clone()))
                            .into_any_element(),
                    );
                    // (A sign-in for another computer finishes with the address the browser here ends on.)
                    if !self.client.here() {
                        body.push(
                            div()
                                .flex()
                                .flex_col()
                                .gap(space::S1)
                                .child(self.label(sign_in.callback_label.clone(), cx))
                                .child(self.input("market-field-callback".into(), &callback, true, cx))
                                .into_any_element(),
                        );
                        buttons.push(Button::new("market-finish-sign-in", sign_in.submit.clone()).primary().loading(self.working).on_click(cx.listener(|this, _, window, cx| this.finish_sign_in(window, cx))).into_any_element());
                    }
                } else {
                    let ready = details.item.connection.as_deref() == Some("ready");
                    if !ready {
                        for (field, text) in &fields {
                            body.push(self.form_field(field, text, cx));
                        }
                        if let Some(help) = details.form.as_ref().and_then(|form| form.help.clone()) {
                            let host = help.split('/').nth(2).unwrap_or(&help).to_owned();
                            body.push(
                                div()
                                    .id("market-token-help")
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .gap(space::S1)
                                    .text_ui(TypeSize::Caption)
                                    .text_color(c.text_muted)
                                    .hover(|s| s.text_color(c.text))
                                    .on_click(move |_, _, cx| cx.open_url(&help))
                                    .child(Icon::new(IconName::External).size(IconSize::XSmall).color(c.text_faint))
                                    .child(host)
                                    .into_any_element(),
                            );
                        }
                    }
                    let actions = details.item.actions.clone();
                    let primary = actions.iter().position(|action| action.id == "add" || action.id == "connect");
                    for (ix, action) in actions.into_iter().enumerate() {
                        let id = action.id.clone();
                        let mut button = Button::new(SharedString::from(format!("market-app-{id}")), action.title.clone()).disabled(!action.enabled || self.working);
                        let primary = Some(ix) == primary;
                        if primary {
                            button = button.primary().loading(self.working);
                        } else if action.destructive() {
                            button = button.destructive();
                        }
                        let button = button.on_click(cx.listener(move |this, _, window, cx| {
                            if action.confirm.is_some() {
                                this.ask_first(action.clone(), window, cx);
                            } else {
                                this.app_action(&id, window, cx);
                            }
                        })).into_any_element();
                        if primary { buttons.push(button) } else { left.push(button) }
                    }
                }
            }
        }
        let title = listed.map(|item| item.title).unwrap_or_default();
        let width = if left.is_empty() { 520. } else { 640. };
        Some(self.frame_with("market-app-sheet", width, title, body, left, buttons, None, cx))
    }

    fn agent_sheet(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(Sheet::Agent(sheet)) = &self.sheet else { return None };
        let c = cx.theme().colors.clone();
        let (details, options) = (match &sheet.details {
            Reading::Ready(details) => Ok(details.clone()),
            Reading::Failed(why) => Err(Some(why.clone())),
            Reading::NotYet => Err(None),
        }, match &sheet.options {
            Reading::Ready(options) => Ok(options.clone()),
            Reading::Failed(why) => Err(Some(why.clone())),
            Reading::NotYet => Err(None),
        });
        let (computer, model, on, name) = (sheet.computer.clone(), sheet.model.clone(), sheet.on.clone(), sheet.name.clone());
        let mut body = Vec::new();
        let mut buttons = vec![self.cancel(cx)];
        let details = match details {
            Ok(details) => details,
            Err(why) => {
                body.push(match why {
                    Some(why) => div().text_ui(TypeSize::Small).text_color(c.text_muted).child(why).into_any_element(),
                    None => self.waiting("Loading…", cx),
                });
                return Some(self.frame("market-agent-sheet", 600., "Add Agent".into(), body, buttons, None, cx));
            }
        };
        let labels = details.labels.clone();
        let face = crate::bot_face::still_bot_face(&details.item.id, &agent_look(&details.item.id), px(44.));
        body.push(self.heading(face, details.item.title.clone(), details.item.summary.clone(), None, cx));
        body.push(div().flex().flex_col().gap(space::S1).child(self.label(labels.name.clone().unwrap_or_default(), cx)).child(self.input("market-field-name".into(), &name, false, cx)).into_any_element());
        // Runs on: the setup's computers, those away shown but not picked.
        let computers = details.computers.clone();
        let shown = computers.iter().find(|each| Some(&each.key) == computer.as_ref()).map(|each| each.name.clone()).unwrap_or_default();
        let view = cx.entity().downgrade();
        let computer_menu = PopoverMenu::new(
            "market-computer-menu",
            picker("market-computer", Some(Icon::new(IconName::Computer).size(IconSize::Nav).color(c.icon).into_any_element()), shown, (computers.len() > 1).then(|| format!("{} computers", computers.len())), cx),
            move |_, cx| {
                let (view, computers) = (view.clone(), computers.clone());
                cx.new(|cx| {
                    let mut menu = Menu::new(cx).width(px(280.));
                    for each in computers {
                        let (view, key) = (view.clone(), each.key.clone());
                        menu = menu.entry(MenuEntry::new(each.name.clone()).icon(IconName::Computer).disabled(!each.available).on_select(move |window, cx| {
                            view.update(cx, |this, cx| this.read_options(key.clone(), window, cx)).ok();
                        }));
                    }
                    menu
                })
            },
        )
        .fill();
        body.push(div().flex().flex_col().gap(space::S1).child(self.label(labels.computer.clone().unwrap_or_default(), cx)).child(computer_menu).into_any_element());
        match &options {
            Err(Some(why)) => body.push(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(why.clone()).into_any_element()),
            Err(None) => body.push(self.waiting("Loading…", cx)),
            Ok(options) => {
                let offered = options.offered.clone();
                let suggested = offered.starting.clone().flatten();
                let picked = model.as_ref().and_then(|model| offered.models.iter().find(|each| each.is(model))).map(|each| format!("{} · {}", offered.provider_name(&each.provider_id), each.name)).unwrap_or_default();
                let view = cx.entity().downgrade();
                let model_menu = PopoverMenu::new(
                    "market-model-menu",
                    picker("market-model", Some(Icon::new(IconName::Sparkle).size(IconSize::Nav).color(c.icon).into_any_element()), picked, None, cx),
                    move |_, cx| {
                        let (view, offered, suggested, current) = (view.clone(), offered.clone(), suggested.clone(), model.clone());
                        cx.new(|cx| {
                            let mut menu = Menu::new(cx).width(px(340.));
                            for each in offered.models.iter().filter(|each| each.is_offered()) {
                                let reference = each.reference();
                                let mut entry = MenuEntry::new(format!("{} · {}", offered.provider_name(&each.provider_id), each.name)).checked(current.as_ref().is_some_and(|model| each.is(model)));
                                if suggested.as_ref().is_some_and(|model| each.is(model)) {
                                    entry = entry.detail("Suggested");
                                }
                                let view = view.clone();
                                menu = menu.entry(entry.on_select(move |_, cx| {
                                    view.update(cx, |this, cx| {
                                        if let Some(Sheet::Agent(sheet)) = &mut this.sheet {
                                            sheet.model = Some(reference.clone());
                                            cx.notify();
                                        }
                                    })
                                    .ok();
                                }));
                            }
                            menu
                        })
                    },
                )
                .fill();
                body.push(div().flex().flex_col().gap(space::S1).child(self.label(labels.model.clone().unwrap_or_default(), cx)).child(model_menu).into_any_element());
                // Needed apps: connected, or what's still to do.
                if !details.item.apps.is_empty() {
                    let mut rows = Vec::new();
                    for app in &details.item.apps {
                        let item = self.app_item(app).cloned().unwrap_or_else(|| MarketItem { id: app.clone(), title: app.clone(), ..MarketItem::default() });
                        let tile = self.tile(Section::Apps, &item, 28., cx);
                        let missing = options.missing.iter().find(|each| &each.id == app).cloned();
                        let trailing: AnyElement = match missing {
                            Some(missing) => {
                                let app = app.clone();
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(space::S2)
                                    .child(self.status(&kuvo_box::marketplace::Status { words: missing.words.clone(), tone: "warning".into() }, cx))
                                    .when(missing.connect, |d| {
                                        d.child(Button::new(SharedString::from(format!("market-connect-{app}")), "Connect").size(ButtonSize::Small).on_click(cx.listener(move |this, _, window, cx| {
                                            let back = this.sheet.take().map(Box::new);
                                            this.open_app(&app, back, window, cx);
                                        })))
                                    })
                                    .into_any_element()
                            }
                            None => match &item.status {
                                Some(status) => self.status(status, cx),
                                None => div().into_any_element(),
                            },
                        };
                        rows.push(div().debug_selector({
                            let app = app.clone();
                            move || format!("market-needs-{app}")
                        }).h(px(40.)).flex().items_center().gap(space::S3).child(tile).child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Small).medium().text_color(c.text).child(item.title.clone())).child(trailing).into_any_element());
                    }
                    body.push(div().flex().flex_col().gap(space::S1).child(self.label(labels.apps.clone().unwrap_or_default(), cx)).children(rows).into_any_element());
                }
                // Routines: each off until turned on.
                if !options.routines.is_empty() {
                    let mut rows = Vec::new();
                    for routine in &options.routines {
                        let id = routine.id.clone();
                        let view = cx.entity().downgrade();
                        rows.push(
                            div()
                                .debug_selector({
                                    let id = id.clone();
                                    move || format!("market-routine-{id}")
                                })
                                .py(space::S1)
                                .flex()
                                .items_center()
                                .gap(space::S3)
                                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(1.)).child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child(routine.title.clone())).child(div().text_ui(TypeSize::Caption).text_color(c.text_muted).child(routine.note.clone())))
                                .child(Switch::new(SharedString::from(format!("market-routine-switch-{id}")), on.contains(&id)).on_change(move |turned, _, cx| {
                                    view.update(cx, |this, cx| {
                                        if let Some(Sheet::Agent(sheet)) = &mut this.sheet {
                                            if turned {
                                                sheet.on.insert(id.clone());
                                            } else {
                                                sheet.on.remove(&id);
                                            }
                                            cx.notify();
                                        }
                                    })
                                    .ok();
                                }))
                                .into_any_element(),
                        );
                    }
                    body.push(
                        div()
                            .flex()
                            .flex_col()
                            .gap(space::S1)
                            .child(self.label(labels.routines.clone().unwrap_or_default(), cx))
                            .children(rows)
                            .when(on.is_empty(), |d| d.children(labels.off.clone().map(|off| div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(off))))
                            .into_any_element(),
                    );
                }
            }
        }
        if let Some(skills) = labels.skills.clone() {
            body.push(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(skills).into_any_element());
        }
        let blocked = options.as_ref().ok().and_then(|options| options.blocked.clone());
        let ready = options.is_ok() && blocked.is_none() && self.sheet_model_set();
        let add = details.item.actions.first().map(|action| action.title.clone()).unwrap_or_default();
        buttons.push(Button::new("market-add-agent", add).primary().loading(self.working).disabled(!ready || self.working).on_click(cx.listener(|this, _, window, cx| this.add_agent(window, cx))).into_any_element());
        let title = details.item.actions.first().map(|action| action.title.clone()).unwrap_or_else(|| details.item.title.clone());
        Some(self.frame("market-agent-sheet", 600., title, body, buttons, blocked, cx))
    }

    fn sheet_model_set(&self) -> bool {
        matches!(&self.sheet, Some(Sheet::Agent(sheet)) if sheet.model.is_some())
    }

    /// A skill's files, one picked, its text shown as it will be installed.
    fn files(&self, files: &[SkillFile], picked: usize, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let names: Vec<AnyElement> = files
            .iter()
            .enumerate()
            .map(|(ix, file)| {
                let on = ix == picked;
                div()
                    .id(("market-file", ix))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap(space::S1)
                    .px(space::S1_5)
                    .h(px(24.))
                    .rounded(radius::SMALL)
                    .when(on, |d| d.bg(c.selected))
                    .hover(|s| s.bg(c.hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        match &mut this.sheet {
                            Some(Sheet::Preview(sheet)) => sheet.file = ix,
                            Some(Sheet::Skill(sheet)) => sheet.file = ix,
                            _ => {}
                        }
                        cx.notify();
                    }))
                    .child(Icon::new(IconName::File).size(IconSize::XSmall).color(c.text_faint))
                    .child(div().text_ui(TypeSize::Caption).text_color(if on { c.text } else { c.text_muted }).child(file.path.clone()))
                    .into_any_element()
            })
            .collect();
        let text = files.get(picked).and_then(SkillFile::text).unwrap_or_default();
        div()
            .debug_selector(|| "market-skill-files".into())
            .flex()
            .flex_col()
            .gap(space::S2)
            .child(div().flex().flex_wrap().gap(space::S1).children(names))
            .child(
                div()
                    .id("market-file-text")
                    .debug_selector(|| "market-file-text".into())
                    .max_h(px(260.))
                    .overflow_y_scroll()
                    .p(space::S3)
                    .rounded(radius::NAV)
                    .bg(c.sunken)
                    .border_1()
                    .border_color(c.border)
                    .font_family(MONO_FONT)
                    .text_size(px(12.))
                    .text_color(c.text)
                    .whitespace_normal()
                    .children(text.lines().map(|line| div().min_h(px(16.)).child(line.to_owned())).collect::<Vec<_>>()),
            )
            .into_any_element()
    }

    fn preview_sheet(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(Sheet::Preview(sheet)) = &self.sheet else { return None };
        let c = cx.theme().colors.clone();
        let (preview, file) = (match &sheet.preview {
            Reading::Ready(preview) => Ok(preview.clone()),
            Reading::Failed(why) => Err(Some(why.clone())),
            Reading::NotYet => Err(None),
        }, sheet.file);
        let mut body = Vec::new();
        let mut buttons = vec![self.cancel(cx)];
        let title = match preview {
            Ok(preview) => {
                let name = preview.skill.name.clone().unwrap_or_else(|| preview.skill.id.clone());
                let source = preview.skill.source.as_ref().map(|source| format!("{} · {}", source.repository, source.commit.chars().take(7).collect::<String>()));
                let item = MarketItem { id: preview.skill.id.clone(), origin: Some(if source.is_some() { "github".into() } else { "folder".into() }), ..MarketItem::default() };
                let tile = self.tile(Section::Skills, &item, 44., cx);
                body.push(self.heading(tile, name.clone(), preview.skill.description.clone().unwrap_or_default(), source, cx));
                body.push(self.files(&preview.files, file, cx));
                buttons.push(Button::new("market-install-skill", preview.action.title.clone()).primary().loading(self.working).disabled(self.working).on_click(cx.listener(|this, _, window, cx| this.install_preview(window, cx))).into_any_element());
                name
            }
            Err(Some(why)) => {
                body.push(div().debug_selector(|| "market-preview-failed".into()).text_ui(TypeSize::Small).text_color(c.text_muted).child(why).into_any_element());
                String::new()
            }
            Err(None) => {
                body.push(self.waiting("Reading the skill…", cx));
                String::new()
            }
        };
        Some(self.frame("market-preview-sheet", 600., title, body, buttons, None, cx))
    }

    fn skill_sheet(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(Sheet::Skill(sheet)) = &self.sheet else { return None };
        let c = cx.theme().colors.clone();
        let (details, file) = (match &sheet.details {
            Reading::Ready(details) => Ok(details.clone()),
            Reading::Failed(why) => Err(Some(why.clone())),
            Reading::NotYet => Err(None),
        }, sheet.file);
        let mut body = Vec::new();
        let mut buttons = vec![self.cancel(cx)];
        let title = match details {
            Ok(details) => {
                let source = details.item.source.as_ref().and_then(|source| source.get("repository")).and_then(|repository| repository.as_str()).map(str::to_owned);
                let tile = self.tile(Section::Skills, &details.item, 44., cx);
                body.push(self.heading(tile, details.item.title.clone(), details.item.summary.clone(), source, cx));
                body.push(self.files(&details.files, file, cx));
                for action in details.item.actions.clone() {
                    let id = action.id.clone();
                    let mut button = Button::new(SharedString::from(format!("market-skill-{id}")), action.title.clone()).disabled(self.working);
                    button = match id.as_str() {
                        "add" => button.primary().loading(self.working),
                        _ if action.destructive() => button.destructive(),
                        _ => button,
                    };
                    buttons.push(button.on_click(cx.listener(move |this, _, window, cx| {
                        if action.confirm.is_some() {
                            this.ask_first(action.clone(), window, cx);
                        } else {
                            this.skill_action(&action, window, cx);
                        }
                    })).into_any_element());
                }
                details.item.title.clone()
            }
            Err(Some(why)) => {
                body.push(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(why).into_any_element());
                String::new()
            }
            Err(None) => {
                body.push(self.waiting("Loading…", cx));
                String::new()
            }
        };
        Some(self.frame("market-skill-sheet", 600., title, body, buttons, None, cx))
    }

    /// Asks first, over the sheet, in the box's words.
    fn ask_first(&mut self, action: MarketAction, window: &mut Window, cx: &mut Context<Self>) {
        self.confirming = Some(action);
        window.focus(&self.confirm_focus, cx);
        cx.notify();
    }

    /// Asked and answered: what the action does, on its sheet.
    fn confirmed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(action) = self.confirming.take() else { return };
        match &self.sheet {
            Some(Sheet::App(_)) => self.app_action(&action.id, window, cx),
            Some(Sheet::Skill(_)) => self.skill_action(&action, window, cx),
            _ => {}
        }
        cx.notify();
    }

    fn sheet_view(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(confirm) = self.confirming.as_ref().and_then(|action| action.confirm.clone()) {
            let card = crate::settings::page::confirm_card(
                crate::settings::page::ConfirmIds { card: "market-confirm", cancel: "market-confirm-cancel", go: "market-confirm-go" },
                &self.confirm_focus,
                (confirm.title, confirm.body, confirm.button),
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.confirming = None;
                    cx.notify();
                }),
                cx.listener(|this, event: &KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        this.confirming = None;
                        cx.notify();
                    }
                }),
                cx.listener(|this, _: &ClickEvent, window, cx| this.confirmed(window, cx)),
                cx,
            );
            return Some(crate::settings::page::sheet(card, cx));
        }
        let card = match &self.sheet {
            Some(Sheet::App(_)) => self.app_sheet(cx),
            Some(Sheet::Form(_)) => self.form_sheet(cx),
            Some(Sheet::Agent(_)) => self.agent_sheet(cx),
            Some(Sheet::Preview(_)) => self.preview_sheet(cx),
            Some(Sheet::Skill(_)) => self.skill_sheet(cx),
            None => None,
        }?;
        Some(crate::settings::page::sheet(card, cx))
    }

    #[cfg(feature = "stand-in")]
    /// A shot's state (`market-<state>`): the area, a category, a sheet open.
    pub fn sample(&mut self, slug: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.section = match slug {
            s if s.starts_with("market-agent") || s == "market-add-agent" => Section::Agents,
            s if s.starts_with("market-skill") || s == "market-preview" || s == "market-yours" || s == "market-new-skill" => Section::Skills,
            _ => Section::Apps,
        };
        self.category = match slug {
            "market-category" => Some("Code".into()),
            "market-yours" => Some("Your skills".into()),
            _ => None,
        };
        if slug == "market-loading" {
            self.visible = true;
            self.market = Reading::NotYet;
            cx.notify();
            return;
        }
        self.visible = true;
        self.read_now(cx);
        match slug {
            "market-connect" => self.open_app("github", None, window, cx),
            "market-connect-oauth" => self.open_app("linear", None, window, cx),
            "market-add-agent" => self.open_agent("bug-triager", window, cx),
            "market-preview" => self.preview_from("https://github.com/acme/agent-skills/tree/main/pr-checklist".into(), window, cx),
            "market-custom" | "market-new-skill" => {
                // (Opened once the box has said its form.)
            }
            _ => {}
        }
    }

    #[cfg(feature = "stand-in")]
    /// A shot's form, once the box has said it.
    pub fn open_sample_form(&mut self, slug: &str, window: &mut Window, cx: &mut Context<Self>) {
        let wanted = match slug {
            "market-custom" => "custom",
            "market-new-skill" => "create",
            _ => return,
        };
        let Some(action) = self.area().and_then(|area| area.action(wanted)).cloned() else { return };
        if let Some(form) = action.form {
            self.open_form(form, if wanted == "custom" { FormFor::Custom } else { FormFor::Create }, window, cx);
        }
    }

    /// Typed into the search field, as typing does.
    pub fn type_search(&mut self, words: &str, cx: &mut Context<Self>) {
        self.search.update(cx, |field, cx| field.set_text(words.to_owned(), cx));
    }
}

impl Render for MarketplaceView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        let header = self.header(cx);
        let problem = self.problem.clone();
        let catalog = self.catalog(cx);
        let sheet = self.sheet_view(cx);
        let width = self.width.clone();
        let entity = cx.entity().downgrade();
        div()
            .debug_selector(|| "marketplace".into())
            .relative()
            .size_full()
            .min_w_0()
            .bg(c.surface)
            .child(
                div()
                    .id("market-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .child(
                        div().w_full().flex().justify_center().px(px(36.)).pt(space::S6).pb(px(48.)).child(
                            div()
                                .relative()
                                .w_full()
                                .max_w(px(MAX_W))
                                .flex()
                                .flex_col()
                                .gap(space::S6)
                                .child(header)
                                .children(problem.map(|problem| {
                                    div()
                                        .debug_selector(|| "market-problem".into())
                                        .flex()
                                        .items_center()
                                        .gap(space::S1_5)
                                        .text_ui(TypeSize::Caption)
                                        .text_color(c.text)
                                        .child(Icon::new(IconName::Warning).size(IconSize::XSmall).color(c.text_muted))
                                        .child(problem)
                                }))
                                .children(catalog)
                                // The catalog's width: one column or two (drawn again when it changes).
                                .child(
                                    canvas(
                                        move |bounds, _, cx| {
                                            let inner = f32::from(bounds.size.width);
                                            if (width.get() - inner).abs() > 0.5 {
                                                width.set(inner);
                                                entity.update(cx, |_, cx| cx.notify()).ok();
                                            }
                                        },
                                        |_, _, _, _| {},
                                    )
                                    .absolute()
                                    .size_full(),
                                ),
                        ),
                    ),
            )
            .children(sheet)
    }
}

/// Whether search words are a link to a skill on GitHub (previewed, not searched for).
fn is_link(words: &str) -> bool {
    let words = words.trim();
    words.starts_with("https://github.com/") || words.starts_with("github.com/")
}

fn slug(words: &str) -> String {
    words.to_lowercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|part| !part.is_empty()).collect::<Vec<_>>().join("-")
}

/// An agent's face: steady for its id, from Kuvo's own looks.
fn agent_look(id: &str) -> Look {
    const COLORS: [&str; 8] = ["indigo", "coral", "sky", "violet", "mint", "amber", "pink", "lime"];
    const SHAPES: [&str; 6] = ["squircle", "round", "pebble", "gem", "bloom", "dome"];
    let sum = id.bytes().fold(7u32, |sum, byte| sum.wrapping_mul(31).wrapping_add(byte as u32));
    Look { color: COLORS[(sum % 8) as usize].into(), shape: SHAPES[((sum / 8) % 6) as usize].into(), face: "calm".into() }
}

fn new_request() -> String {
    let id: String = BoxClient::new_id().chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')).collect();
    let mut id = format!("market-{id}");
    while id.len() < 22 {
        id.push('0');
    }
    id.chars().take(43).collect()
}

/// A picker's face: what's picked, what's beside it, and the menu's chevron.
fn picker(key: &'static str, leading: Option<AnyElement>, value: String, trailing: Option<String>, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .id(key)
        .debug_selector(move || key.into())
        .w_full()
        .h(px(34.))
        .px(space::S3)
        .rounded(radius::NAV)
        .bg(c.surface)
        .border_1()
        .border_color(c.border_strong)
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(space::S2)
        .children(leading)
        .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text).child(value))
        .children(trailing.map(|trailing| div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_faint).child(trailing)))
        .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(c.text_faint))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_github_link_is_previewed_and_other_words_searched() {
        assert!(is_link("https://github.com/acme/skills/tree/main/pr"));
        assert!(is_link(" github.com/acme/skills"));
        assert!(!is_link("github"));
    }

    #[test]
    fn an_add_request_is_as_long_as_the_box_takes() {
        let id = new_request();
        assert!((22..=43).contains(&id.len()), "{id}");
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')));
    }
}
