//! The bars over a conversation's composer, for work going on that the conversation doesn't show
//! as it happens (as the Mac's `WorkBar`s): a bot's sessions at work ("5 sessions at work ·
//! answered 1 ask · 2.2M tokens today", with All), the subagents at work under it, and the commands
//! it left running ("Running in the background", each with how long it has run, Output and Stop).
//! Each is a line that folds what's under it; each row opens its conversation. All of it is what
//! the box says (`kuvo/work?bot=`, `kuvo/work/background/<session>`), read again as it says it
//! changed; nothing's worked out here. The card goes when nothing runs.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Anchor, AnyElement, App, AppContext as _, Context, EventEmitter, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Task, Window, anchored, deferred, div, point, prelude::FluentBuilder as _, px,
};
use kuvo_box::background::{Background, Command, Output};
use kuvo_box::bots::{Work, WorkChanged, WorkSession, WorkState};
use kuvo_box::follow::Followed;
use kuvo_box::models::effort_title;
use kuvo_box::{BoxClient, Follow, Timing};
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TypeSize,
    menu::{Menu, MenuEntry, PopoverMenu},
    small::spinner,
    theme::{radius, space},
};

use crate::entries::COLUMN;

/// How many rows show under a bar before it scrolls.
const ROWS: usize = 4;
const ROW: f32 = 30.;
/// How often Output reads what a command printed while it shows.
const OUTPUT_AGAIN: Duration = Duration::from_millis(1500);

/// One of the bars.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bar {
    Work,
    Subagents,
    Background,
}

/// Opens a session's conversation (the workspace's, handed in).
pub type Opener = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// What a command's Output shows: the command, and what was read of it last.
struct Shown {
    command: Command,
    read: Option<Output>,
}

pub struct WorkBars {
    client: BoxClient,
    session: String,
    live: bool,
    /// The bot this conversation is, whose sessions show in a bar of their own.
    bot: Option<String>,
    background: Background,
    work: Option<Work>,
    /// Models' names, by their ids, as the composer's box offers them.
    names: HashMap<String, String>,
    folded: HashSet<Bar>,
    output: Option<Shown>,
    /// The command whose Output a press elsewhere just put away, for that same press: its own
    /// Output button doesn't open it again.
    dismissed: Option<String>,
    stopping: HashSet<String>,
    failure: Option<String>,
    /// The latest read of each, so an older one coming back late is left.
    background_reads: u64,
    work_reads: u64,
    opener: Option<Opener>,
    /// The moment it's seen at, where one's set (tests and shots); else the clock's.
    now: Option<f64>,
    _background_follow: Option<Follow<Followed<Background>>>,
    _work_follow: Option<Follow<Followed<WorkChanged>>>,
    /// When the bot's work was last said to change, as last seen.
    work_seen: Option<f64>,
    ticking: Option<Task<()>>,
    reading_output: Option<Task<()>>,
    tasks: Vec<Task<()>>,
}

impl EventEmitter<()> for WorkBars {}

impl WorkBars {
    /// The bars of `session`'s conversation. `live`: follow its box (the app); else it shows what
    /// it's read with [`WorkBars::read_at_once`] (tests and shots).
    pub fn new(client: BoxClient, session: String, live: bool, cx: &mut Context<Self>) -> WorkBars {
        let mut bars = WorkBars {
            client: client.clone(),
            session,
            live,
            bot: None,
            background: Background::default(),
            work: None,
            names: HashMap::new(),
            folded: HashSet::new(),
            output: None,
            dismissed: None,
            stopping: HashSet::new(),
            failure: None,
            background_reads: 0,
            work_reads: 0,
            opener: None,
            now: None,
            _background_follow: None,
            _work_follow: None,
            work_seen: None,
            ticking: None,
            reading_output: None,
            tasks: Vec::new(),
        };
        if live {
            // What runs for it, now and as the box says it changes (one past the counts' limit too).
            let follow = client.follow_session_background(&bars.session, Timing::default());
            let updates = follow.updates().clone();
            bars._background_follow = Some(follow);
            bars.tasks.push(cx.spawn(async move |this, cx| {
                while let Ok(update) = updates.recv().await {
                    let Followed::Now(background) = update else { continue };
                    if this.update(cx, |this, cx| this.take_background(background, cx)).is_err() {
                        break;
                    }
                }
            }));
        }
        bars
    }

    fn spawn<T: Send + 'static>(&mut self, cx: &mut Context<Self>, work: impl FnOnce() -> T + Send + 'static, then: impl FnOnce(&mut WorkBars, T, &mut Context<Self>) + 'static) {
        let task = cx.background_executor().spawn(async move { work() });
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| then(this, result, cx)).ok();
        }));
    }

    /// The moment it's seen at (tests and shots, as the workspace's).
    pub fn set_now(&mut self, now: Option<f64>) {
        self.now = now;
    }

    /// How a row opens its conversation.
    pub fn set_opener(&mut self, opener: Opener) {
        self.opener = Some(opener);
    }

    /// Models' names as the box offers them, for a bot's sessions' rows.
    pub fn set_names(&mut self, names: HashMap<String, String>, cx: &mut Context<Self>) {
        if self.names != names {
            self.names = names;
            cx.notify();
        }
    }

    /// The bot this conversation is (none: not a bot's): its sessions' bar follows its work.
    pub fn set_bot(&mut self, bot: Option<String>, cx: &mut Context<Self>) {
        if self.bot == bot {
            return;
        }
        self.bot = bot.clone();
        self.work = None;
        self._work_follow = None;
        self.work_seen = None;
        cx.notify();
        let Some(bot) = bot.filter(|_| self.live) else { return };
        let follow = self.client.follow_work_changes(Timing::default());
        let updates = follow.updates().clone();
        self._work_follow = Some(follow);
        self.tasks.push(cx.spawn(async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                let Followed::Now(changes) = update else { continue };
                let went = this.update(cx, |this, cx| {
                    let at = changes.changed.get(&bot).copied();
                    // (The first line too: what changed while it wasn't followed.)
                    if this.work.is_none() || at != this.work_seen {
                        this.work_seen = at;
                        this.read_work(cx);
                    }
                });
                if went.is_err() {
                    break;
                }
            }
        }));
    }

    /// What it asks changed (a subagent waiting on you, or not): what runs is read again.
    pub fn asks_changed(&mut self, cx: &mut Context<Self>) {
        if self.live && !self.background.subagents.is_empty() {
            self.read_background(cx);
        }
    }

    fn read_background(&mut self, cx: &mut Context<Self>) {
        self.background_reads += 1;
        let read = self.background_reads;
        let (client, session) = (self.client.clone(), self.session.clone());
        self.spawn(cx, move || client.session_background(&session), move |this, background, cx| {
            if this.background_reads == read
                && let Ok(background) = background
            {
                this.take_background(background, cx);
            }
        });
    }

    fn read_work(&mut self, cx: &mut Context<Self>) {
        let Some(bot) = self.bot.clone() else { return };
        self.work_reads += 1;
        let read = self.work_reads;
        let client = self.client.clone();
        self.spawn(cx, move || client.bot_work(&bot), move |this, work, cx| {
            if this.work_reads == read
                && let Ok(work) = work
            {
                this.work = Some(work);
                cx.notify();
            }
        });
    }

    /// What runs for it besides its turn, as its box said.
    pub fn take_background(&mut self, background: Background, cx: &mut Context<Self>) {
        // (One stopping that's gone isn't stopping any more.)
        self.stopping.retain(|id| background.commands.iter().any(|command| &command.id == id) || background.subagents.iter().any(|subagent| &subagent.session == id));
        // A command gone takes its Output with it (its row's gone), and stops reading it.
        if self.output.as_ref().is_some_and(|shown| !background.commands.iter().any(|command| command.id == shown.command.id)) {
            self.output = None;
            self.reading_output = None;
        }
        self.background = background;
        // Its time goes on while commands run.
        if self.live && !self.background.commands.is_empty() && self.ticking.is_none() {
            self.ticking = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let going = this.update(cx, |this, cx| {
                        cx.notify();
                        !this.background.commands.is_empty()
                    });
                    if !matches!(going, Ok(true)) {
                        break;
                    }
                }
                this.update(cx, |this, _| this.ticking = None).ok();
            }));
        }
        cx.notify();
    }

    /// What its box says now, read at once on this thread (tests and shots).
    #[cfg(feature = "stand-in")]
    pub fn read_at_once(&mut self, cx: &mut Context<Self>) {
        if let Ok(background) = self.client.session_background(&self.session) {
            self.take_background(background, cx);
        }
        if let Some(bot) = &self.bot {
            self.work = self.client.bot_work(bot).ok();
        }
        if let Some(shown) = self.output.as_mut() {
            shown.read = Some(self.client.command_output(&shown.command));
        }
        cx.notify();
    }

    /// What a bot's session's row says of it beside its title.
    pub fn about_of(&self, session: &str) -> Option<String> {
        self.work.as_ref()?.sessions.iter().find(|each| each.session == session).map(|each| self.about(each))
    }

    pub fn background(&self) -> &Background {
        &self.background
    }

    /// The bot's sessions at work now, as its box last said.
    pub fn at_work(&self) -> Vec<&WorkSession> {
        self.work.as_ref().map(|work| work.sessions.iter().filter(|session| session.is_at_work()).collect()).unwrap_or_default()
    }

    /// Whether the bot's bar shows: sessions at work, or any started today (for All).
    fn shows_work(&self) -> bool {
        self.bot.is_some() && self.work.as_ref().is_some_and(|work| work.sessions.iter().any(WorkSession::is_at_work) || work.today.started > 0)
    }

    /// Whether anything shows: else the card goes.
    pub fn is_empty(&self) -> bool {
        !self.shows_work() && self.background.commands.is_empty() && self.background.subagents.is_empty()
    }

    pub fn is_folded(&self, bar: Bar) -> bool {
        self.folded.contains(&bar)
    }

    /// Folds what's under a bar into its line, or shows it again.
    pub fn toggle(&mut self, bar: Bar, cx: &mut Context<Self>) {
        if !self.folded.remove(&bar) {
            self.folded.insert(bar);
        }
        cx.notify();
    }

    /// Opens a session's conversation, to watch it and talk to it.
    pub fn open(&self, session: &str, window: &mut Window, cx: &mut App) {
        if let Some(opener) = self.opener.clone() {
            opener(session, window, cx);
        }
    }

    /// Stops a command it left running: its agent hears that it ended, with what it printed.
    pub fn stop_command(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(command) = self.background.commands.iter().find(|command| command.id == id).cloned() else { return };
        self.stopping.insert(id.to_owned());
        self.failure = None;
        cx.notify();
        let client = self.client.clone();
        self.spawn(cx, move || client.stop_command(&command), move |this, result, cx| {
            if let Err(error) = result {
                this.failure = Some(format!("It didn’t stop: {}", kuvo_box::accounts::said(&error)));
            }
            this.after_stop(cx);
        });
    }

    /// Stops a session at work (a subagent, or one of the bot's): what waits for it is held.
    pub fn stop_session(&mut self, session: &str, cx: &mut Context<Self>) {
        self.stopping.insert(session.to_owned());
        self.failure = None;
        cx.notify();
        let (client, id) = (self.client.clone(), session.to_owned());
        self.spawn(cx, move || client.stop_session(&id, &[]), move |this, result, cx| {
            if let Err(error) = result {
                this.failure = Some(format!("It didn’t stop: {}", kuvo_box::accounts::said(&error)));
            }
            this.after_stop(cx);
        });
    }

    /// Every session of the bot's work stopped; each keeps what it did.
    pub fn stop_all(&mut self, cx: &mut Context<Self>) {
        let Some(bot) = self.bot.clone() else { return };
        self.failure = None;
        let client = self.client.clone();
        self.spawn(cx, move || client.stop_bot_work(&bot), move |this, result, cx| {
            if let Err(error) = result {
                this.failure = Some(format!("They didn’t stop: {}", kuvo_box::accounts::said(&error)));
            }
            this.after_stop(cx);
        });
    }

    fn after_stop(&mut self, cx: &mut Context<Self>) {
        if self.live {
            self.read_background(cx);
            self.read_work(cx);
        } else {
            #[cfg(feature = "stand-in")]
            self.read_at_once(cx);
        }
        cx.notify();
    }

    /// Shows what a command has printed, read again every moment while it shows until it ends;
    /// again, it's put away.
    pub fn toggle_output(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.output.as_ref().is_some_and(|shown| shown.command.id == id) {
            self.output = None;
            self.reading_output = None;
            cx.notify();
            return;
        }
        let Some(command) = self.background.commands.iter().find(|command| command.id == id).cloned() else { return };
        self.output = Some(Shown { command: command.clone(), read: None });
        cx.notify();
        if !self.live {
            #[cfg(feature = "stand-in")]
            self.read_at_once(cx);
            return;
        }
        let client = self.client.clone();
        self.reading_output = Some(cx.spawn(async move |this, cx| {
            loop {
                let (client, asked) = (client.clone(), command.clone());
                let read = cx.background_executor().spawn(async move { client.command_output(&asked) }).await;
                let ended = read == Output::Ended;
                let shown = this.update(cx, |this, cx| {
                    let Some(shown) = this.output.as_mut().filter(|shown| shown.command.id == command.id) else { return false };
                    // (What was read stays while a read fails, and once it ended.)
                    match (&shown.read, &read) {
                        (Some(Output::Printed(_)), Output::Unread | Output::Ended) if read == Output::Unread => {}
                        _ => shown.read = Some(read),
                    }
                    cx.notify();
                    true
                });
                if !matches!(shown, Ok(true)) || ended {
                    break;
                }
                cx.background_executor().timer(OUTPUT_AGAIN).await;
            }
        }));
    }

    /// Output's words for what's been read.
    pub fn output_words(&self) -> Option<(String, bool)> {
        let shown = self.output.as_ref()?;
        Some(match &shown.read {
            Some(Output::Printed(text)) if !text.is_empty() => (text.clone(), true),
            Some(Output::Printed(_)) => ("Nothing printed yet.".into(), false),
            Some(Output::Ended) => ("This command has ended. What it printed is no longer kept.".into(), false),
            Some(Output::Unread) => ("Couldn’t read what it printed. Its computer may be away; this tries again on its own.".into(), false),
            None => ("Reading what it printed…".into(), false),
        })
    }
}

/// The bar's line over a bot's composer: "3 sessions at work", or, with none at work, how many it
/// started today (KuvoClient's `WorkWords.barTitle`).
pub fn work_title(at_work: usize, today: u32) -> String {
    match at_work {
        1 => "A session is at work".into(),
        n if n > 1 => format!("{n} sessions at work"),
        _ if today == 1 => "1 session today, none at work".into(),
        _ => format!("{today} sessions today, none at work"),
    }
}

/// What it did today besides: the asks it answered, the tokens its sessions used.
pub fn work_progress(work: &Work) -> Option<String> {
    let mut parts = Vec::new();
    match work.today.answered {
        0 => {}
        1 => parts.push("answered 1 ask".to_owned()),
        n => parts.push(format!("answered {n} asks")),
    }
    if work.today.tokens > 0 {
        parts.push(format!("{} tokens today", tokens(work.today.tokens)));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// A count of tokens, short: "412k", "1.2M".
pub fn tokens(count: u64) -> String {
    match count {
        0..1000 => count.to_string(),
        1000..1_000_000 => format!("{}k", (count as f64 / 1000.).round() as u64),
        _ => format!("{:.1}M", count as f64 / 1_000_000.),
    }
}

/// What a session asks, as its bot's work says it: "Asks to run npx expo install".
pub fn asking(what: &str) -> String {
    for (said, says) in [("Wants to run a command: ", "Asks to run "), ("Wants to edit files: ", "Asks to edit "), ("Wants to ", "Asks to ")] {
        if let Some(rest) = what.strip_prefix(said) {
            return format!("{says}{rest}");
        }
    }
    what.to_owned()
}

/// How long it has run, as a timer says it: "0:42", "12:03", "1:02:03".
pub fn elapsed(ms: f64) -> String {
    let seconds = (ms / 1000.).max(0.) as u64;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 { format!("{hours}:{minutes:02}:{seconds:02}") } else { format!("{minutes}:{seconds:02}") }
}

impl WorkBars {
    /// One of a bot's sessions in a few words: its model, effort and mode, the account it runs on,
    /// and its project (KuvoClient's `WorkWords.about`).
    fn about(&self, session: &WorkSession) -> String {
        let model = session.model.as_ref().map(|model| self.names.get(&model.id).cloned().unwrap_or_else(|| model.id.clone()));
        let effort = session.model.as_ref().and_then(|model| model.variant.as_deref()).map(effort_title);
        let plan = (session.mode.as_deref() == Some("plan")).then(|| "Plan".to_owned());
        let account = session.account.as_ref().map(|account| account.title.clone().unwrap_or_else(|| account.name.clone()));
        let project = session.project.as_ref().map(|project| project.rsplit('/').next().unwrap_or(project).to_owned());
        [model, effort, plan, account, project].into_iter().flatten().collect::<Vec<_>>().join(" · ")
    }

    /// A bar's line: its mark, what it says, how far along, and (folded, with one under it) what
    /// that is; clicking it folds what's under it or shows it again; its own button at its end.
    fn line(&self, bar: Bar, mark: AnyElement, title: String, progress: Option<String>, detail: Option<String>, accessory: Option<AnyElement>, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let folded = self.is_folded(bar);
        let name = match bar {
            Bar::Work => "work",
            Bar::Subagents => "subagents",
            Bar::Background => "background",
        };
        div()
            .flex()
            .items_center()
            .h(px(34.))
            .pr(space::S1)
            .child(
                div()
                    .id(SharedString::from(format!("bar-{name}")))
                    .debug_selector(move || format!("bar-{name}"))
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .pl(space::S3)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(bar, cx)))
                    .child(div().flex_none().w(px(16.)).flex().justify_center().child(mark))
                    .child(div().flex_none().text_ui(TypeSize::Small).medium().text_color(c.text).child(title))
                    .children(progress.map(|progress| div().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text_muted).child(progress)))
                    .children(detail.filter(|_| folded).map(|detail| div().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text_muted).child(detail))),
            )
            .children(accessory.map(|accessory| div().flex().items_center().child(accessory)))
            .child(
                div().debug_selector(move || format!("fold-{name}")).child(
                    Button::icon_only(SharedString::from(format!("fold-{name}")), if folded { IconName::ChevronUp } else { IconName::ChevronDown }, if folded { "Show them" } else { "Hide them: this line stays while they work" })
                        .size(ButtonSize::Small)
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle(bar, cx))),
                ),
            )
            .into_any_element()
    }

    /// The rows under a line, as many as fit before they scroll.
    fn rows(&self, bar: Bar, rows: Vec<AnyElement>) -> Option<AnyElement> {
        if self.is_folded(bar) || rows.is_empty() {
            return None;
        }
        let shown = rows.len().min(ROWS) as f32;
        Some(div().id(SharedString::from(format!("rows-{bar:?}"))).h(px(ROW * shown)).overflow_y_scroll().pb(space::S1).children(rows).into_any_element())
    }

    /// A row: its title, then a quieter line that gives way at its end (orange while it waits on
    /// you), then its buttons, shown under the pointer.
    fn row(&self, key: &str, title: String, about: String, waits: bool, hover_only: bool, buttons: Vec<AnyElement>, cx: &App) -> AnyElement {
        let c = cx.theme().colors.clone();
        let group = SharedString::from(format!("bar-row-{key}"));
        let selector = format!("bar-row-{key}");
        div()
            .id(group.clone())
            .debug_selector(move || selector)
            .group(group.clone())
            .flex()
            .items_center()
            .gap(space::S2)
            .h(px(ROW))
            .pl(px(40.))
            .pr(space::S1)
            .child(div().flex_none().max_w(px(320.)).truncate().text_ui(TypeSize::Small).text_color(c.text).child(title))
            .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Caption).text_color(if waits { c.warning } else { c.text_muted }).child(about))
            .child(div().flex().flex_none().gap(space::S0_5).when(hover_only, |d| d.opacity(0.).group_hover(group, |s| s.opacity(1.))).children(buttons))
            .into_any_element()
    }

    fn button(&self, id: String, label: &'static str, icon: Option<IconName>, disabled: bool, tip: &'static str, on: impl Fn(&mut WorkBars, &mut Window, &mut Context<WorkBars>) + 'static, cx: &mut Context<Self>) -> AnyElement {
        let selector = id.clone();
        let mut button = Button::new(SharedString::from(id), label).ghost().size(ButtonSize::Small).disabled(disabled).tooltip(tip).on_click(cx.listener(move |this, _, window, cx| on(this, window, cx)));
        if let Some(icon) = icon {
            button = button.icon(icon);
        }
        div().debug_selector(move || selector).child(button).into_any_element()
    }

    fn work_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.shows_work() {
            return None;
        }
        let work = self.work.as_ref()?;
        let c = cx.theme().colors.clone();
        let at_work = self.at_work();
        let mark: AnyElement = if at_work.is_empty() { Icon::new(IconName::Check).size(IconSize::Nav).color(c.accent).into_any_element() } else { spinner("work-spinner", IconSize::Nav, c.text_muted).into_any_element() };
        let detail = (at_work.len() == 1).then(|| at_work[0].title.clone().unwrap_or_default());
        let rows: Vec<AnyElement> = at_work
            .iter()
            .map(|session| {
                let waits = session.state == WorkState::Asking;
                let about = if waits { session.asks.as_deref().map(asking).unwrap_or_else(|| "Waiting for an answer".into()) } else { self.about(session) };
                let (open, stop) = (session.session.clone(), session.session.clone());
                let buttons = vec![
                    self.button(format!("bar-open-{}", session.session), "Open", Some(IconName::ArrowRight), false, "Open its conversation, to watch it and talk to it", move |this, window, cx| this.open(&open, window, cx), cx),
                    self.button(format!("bar-stop-{}", session.session), "Stop", None, self.stopping.contains(&session.session), "Stop it: the bot hears it was stopped", move |this, _, cx| this.stop_session(&stop, cx), cx),
                ];
                // (Its Open and Stop under the pointer, as on the Mac.)
                self.row(&session.session, session.title.clone().unwrap_or_else(|| "A session".into()), about, waits, true, buttons, cx)
            })
            .collect();
        let all = self.all_menu(work, cx);
        Some(div().flex().flex_col().child(self.line(Bar::Work, mark, work_title(at_work.len(), work.today.started), work_progress(work), detail, Some(all), cx)).children(self.rows(Bar::Work, rows)).into_any_element())
    }

    /// All of the bot's work: its sessions at work and done, each opening its conversation, and
    /// Stop All while any works.
    fn all_menu(&self, work: &Work, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        let sessions = work.sessions.clone();
        let today = format!("Today: {} started{}", work.today.started, if work.today.tokens > 0 { format!(" · {} tokens", tokens(work.today.tokens)) } else { String::new() });
        let trigger = div().debug_selector(|| "work-all".into()).child(Button::new("work-all", "All").ghost().size(ButtonSize::Small).tooltip("All of its work today"));
        PopoverMenu::new("work-all-menu", trigger, move |_, cx| {
            let (sessions, this, today) = (sessions.clone(), this.clone(), today.clone());
            cx.new(|cx| {
                let mut menu = Menu::new(cx).width(px(360.)).header(today);
                let entry = |session: &WorkSession, this: gpui::WeakEntity<WorkBars>| {
                    let id = session.session.clone();
                    let detail = match session.state {
                        WorkState::Working => "Working",
                        WorkState::Asking => "Waiting for you",
                        WorkState::Finished => "Done",
                        WorkState::Failed => "Failed",
                        WorkState::Stopped => "Stopped",
                        WorkState::Idle | WorkState::Other => "",
                    };
                    MenuEntry::new(session.title.clone().unwrap_or_else(|| "A session".into())).detail(detail).on_select(move |window, cx| {
                        this.update(cx, |this, cx| this.open(&id, window, cx)).ok();
                    })
                };
                let at_work: Vec<&WorkSession> = sessions.iter().filter(|session| session.is_at_work()).collect();
                if !at_work.is_empty() {
                    menu = menu.separator();
                    for session in &at_work {
                        menu = menu.entry(entry(session, this.clone()));
                    }
                }
                let done: Vec<&WorkSession> = sessions.iter().filter(|session| !session.is_at_work()).take(12).collect();
                if !done.is_empty() {
                    menu = menu.separator();
                    for session in done {
                        menu = menu.entry(entry(session, this.clone()));
                    }
                }
                if sessions.is_empty() {
                    menu = menu.entry(MenuEntry::new("It hasn’t started any sessions yet.").disabled(true));
                }
                if !at_work.is_empty() {
                    let this = this.clone();
                    menu = menu.separator().entry(MenuEntry::new("Stop All").destructive().on_select(move |_, cx| {
                        this.update(cx, |this, cx| this.stop_all(cx)).ok();
                    }));
                }
                menu
            })
        })
        .align_right()
        .into_any_element()
    }

    fn subagents_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let subagents = &self.background.subagents;
        if subagents.is_empty() {
            return None;
        }
        let c = cx.theme().colors.clone();
        let title = if subagents.len() == 1 { "A subagent is at work".to_owned() } else { format!("{} subagents at work", subagents.len()) };
        let name = |title: &Option<String>| title.clone().filter(|title| !title.is_empty()).unwrap_or_else(|| "A subagent’s task".into());
        let detail = (subagents.len() == 1).then(|| name(&subagents[0].title));
        let rows: Vec<AnyElement> = subagents
            .iter()
            .map(|subagent| {
                let (open, stop) = (subagent.session.clone(), subagent.session.clone());
                let buttons = vec![
                    self.button(format!("bar-open-{}", subagent.session), "Open", Some(IconName::ArrowRight), false, "Open its conversation, to watch it and talk to it", move |this, window, cx| this.open(&open, window, cx), cx),
                    self.button(format!("bar-stop-{}", subagent.session), "Stop", None, self.stopping.contains(&subagent.session), "Stop it: the agent hears it was stopped, and carries on", move |this, _, cx| this.stop_session(&stop, cx), cx),
                ];
                self.row(&subagent.session, name(&subagent.title), if subagent.asking { "Waiting for you".into() } else { "Working…".into() }, subagent.asking, false, buttons, cx)
            })
            .collect();
        let mark = spinner("subagents-spinner", IconSize::Nav, c.text_muted).into_any_element();
        Some(div().flex().flex_col().child(self.line(Bar::Subagents, mark, title, None, detail, None, cx)).children(self.rows(Bar::Subagents, rows)).into_any_element())
    }

    fn background_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let commands = &self.background.commands;
        if commands.is_empty() {
            return None;
        }
        let c = cx.theme().colors.clone();
        let title = if commands.len() == 1 { "Running in the background".to_owned() } else { format!("{} commands running in the background", commands.len()) };
        let one_line = |command: &Command| command.command.replace('\n', " ");
        let detail = (commands.len() == 1).then(|| one_line(&commands[0]));
        let now = self.now.unwrap_or_else(crate::time::now_ms);
        let rows: Vec<AnyElement> = commands
            .iter()
            .map(|command| {
                let (output, stop) = (command.id.clone(), command.id.clone());
                let group = SharedString::from(format!("bar-row-{}", command.id));
                let selector = format!("bar-row-{}", command.id);
                let shows_output = self.output.as_ref().is_some_and(|shown| shown.command.id == command.id);
                div()
                    .id(group.clone())
                    .debug_selector(move || selector)
                    .relative()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .h(px(ROW))
                    .pl(px(40.))
                    .pr(space::S1)
                    .child(div().flex_1().min_w_0().truncate().text_mono(TypeSize::Caption).text_color(c.text).child(one_line(command)))
                    .child(div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_muted).child(elapsed(now - command.started)))
                    .child({
                        // (On the press, as a menu's trigger: the panel's own press-outside has put
                        // it away by then, and this press mustn't open it again.)
                        let selector = format!("bar-output-{}", command.id);
                        div()
                            .debug_selector(move || selector)
                            .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                if this.dismissed.as_deref() != Some(output.as_str()) {
                                    this.toggle_output(&output, cx);
                                }
                                cx.stop_propagation();
                            }))
                            .child(Button::new(SharedString::from(format!("bar-output-{}", command.id)), "Output").ghost().size(ButtonSize::Small).tooltip("What it has printed so far"))
                    })
                    .child(self.button(format!("bar-stop-{}", command.id), "Stop", None, self.stopping.contains(&command.id), "Stop it: the agent hears that it ended, with what it printed", move |this, _, cx| this.stop_command(&stop, cx), cx))
                    .children(shows_output.then(|| self.output_panel(cx)))
                    .into_any_element()
            })
            .collect();
        let mark = spinner("background-spinner", IconSize::Nav, c.text_muted).into_any_element();
        Some(div().flex().flex_col().child(self.line(Bar::Background, mark, title, None, detail, None, cx)).children(self.rows(Bar::Background, rows)).into_any_element())
    }

    /// What a command has printed, over its row, until it's put away (Output again, or a click
    /// elsewhere).
    fn output_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let c = &theme.colors;
        let (words, printed) = self.output_words().unwrap_or_default();
        let panel = div()
            .id("command-output")
            .debug_selector(|| "command-output".into())
            .w(px(520.))
            .max_h(px(300.))
            .overflow_y_scroll()
            .p(space::S3)
            .rounded(radius::POPOVER)
            .bg(c.elevated)
            .border_1()
            .border_color(c.border)
            .shadow(theme.elevated_shadow())
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.dismissed = this.output.take().map(|shown| shown.command.id);
                this.reading_output = None;
                // (Only for this press: once it's been dispatched, it's forgotten.)
                cx.defer_in(window, |this, _, _| this.dismissed = None);
                cx.notify();
            }))
            .child(div().text_mono(TypeSize::Caption).text_color(if printed { c.text } else { c.text_muted }).child(words));
        div().absolute().top_0().right_0().child(deferred(anchored().anchor(Anchor::BottomRight).offset(point(px(0.), -space::S1)).snap_to_window_with_margin(px(8.)).child(panel)).with_priority(1)).into_any_element()
    }
}

impl Render for WorkBars {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.is_empty() {
            return div().into_any_element();
        }
        let c = cx.theme().colors.clone();
        let bars: Vec<AnyElement> = [self.work_bar(cx), self.subagents_bar(cx), self.background_bar(cx)].into_iter().flatten().collect();
        let count = bars.len();
        div()
            .debug_selector(|| "work-bars".into())
            .w_full()
            .max_w(px(COLUMN))
            .mb(space::S2)
            .flex()
            .flex_col()
            .rounded(radius::CARD - px(4.))
            .bg(c.sunken)
            .border_1()
            .border_color(c.border)
            .children(bars.into_iter().enumerate().map(|(ix, bar)| div().when(ix + 1 < count, |d| d.border_b_1().border_color(c.border)).child(bar)))
            .children(self.failure.clone().map(|why| div().px(space::S3).pb(space::S2).text_ui(TypeSize::Caption).text_color(c.warning).child(why)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bots_line_says_what_it_did_today_in_the_macs_words() {
        let work: Work = serde_json::from_value(serde_json::json!({ "sessions": [], "today": { "started": 5, "answered": 1, "tokens": 2_200_000 } })).unwrap();
        assert_eq!(work_progress(&work).as_deref(), Some("answered 1 ask · 2.2M tokens today"));
        assert_eq!(work_title(5, 5), "5 sessions at work");
        assert_eq!(work_title(0, 1), "1 session today, none at work");
        assert_eq!(elapsed(3_723_000.), "1:02:03");
        assert_eq!(asking("Wants to run a command: npm test"), "Asks to run npm test");
    }
}
