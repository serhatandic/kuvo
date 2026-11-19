//! Conversation activity in the header. The activity feeds stay live while the popover is closed.
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext as _, Context, DismissEvent, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, StyledImage, Subscription, Task, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px,
};
use kuvo_box::areas::Look;
use kuvo_box::artifacts::{Artifact, Outputs};
use kuvo_box::background::{Background, Command, Output};
use kuvo_box::bots::{Work, WorkChanged, WorkSession, WorkState};
use kuvo_box::follow::Followed;
use kuvo_box::{BoxClient, Follow, Timing};
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TypeSize,
    icon::{Mark, provider_mark, provider_mark_color},
    menu::{PopoverMenu, PopoverMenuHandle},
    small::status_dot,
    theme::{radius, space},
};

use crate::wakes::{self, WakeSchedule};

const OUTPUT_AGAIN: Duration = Duration::from_millis(1500);

/// Opens a session's conversation (the workspace's, handed in).
pub type Opener = Rc<dyn Fn(&str, &mut Window, &mut App)>;
pub(crate) type BotLook = Rc<dyn Fn(&str, Option<&str>, &App) -> Option<(String, Look)>>;

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
    background_fresh: bool,
    work_connected: bool,
    work_fresh: bool,
    work_target: Option<f64>,
    work_pending: bool,
    stopping_all: bool,
    active_rows: Rc<[SessionRow]>,
    recent_rows: Rc<[SessionRow]>,
    work: Option<Work>,
    handle: PopoverMenuHandle,
    trigger_focus: FocusHandle,
    wakes: Option<wakes::Status>,
    wake_computer: String,
    now: Option<f64>,
    stopping: HashSet<String>,
    failure: Option<String>,
    /// The latest read of each, so an older one coming back late is left.
    background_reads: u64,
    work_reads: u64,
    opener: Option<Opener>,
    bot_look: Option<BotLook>,
    _background_follow: Option<Follow<Followed<Background>>>,
    _work_follow: Option<Follow<Followed<WorkChanged>>>,
    /// When the bot's work was last said to change, as last seen.
    work_seen: Option<f64>,
    tasks: Vec<Task<()>>,
}

impl WorkBars {
    /// Activity in `session`'s conversation. `live`: follow its box (the app); else it shows what
    /// it's read with [`WorkBars::read_at_once`] (tests and shots).
    pub fn new(client: BoxClient, session: String, live: bool, cx: &mut Context<Self>) -> WorkBars {
        let mut bars = WorkBars {
            client: client.clone(),
            session,
            live,
            bot: None,
            background: Background::default(),
            background_fresh: false,
            work_connected: false,
            work_fresh: false,
            work_target: None,
            work_pending: false,
            stopping_all: false,
            active_rows: Rc::from([]),
            recent_rows: Rc::from([]),
            work: None,
            handle: Default::default(),
            trigger_focus: cx.focus_handle(),
            wakes: None,
            wake_computer: String::new(),
            now: None,
            stopping: HashSet::new(),
            failure: None,
            background_reads: 0,
            work_reads: 0,
            opener: None,
            bot_look: None,
            _background_follow: None,
            _work_follow: None,
            work_seen: None,
            tasks: Vec::new(),
        };
        if live {
            // What runs for it, now and as the box says it changes (one past the counts' limit too).
            let follow = client.follow_session_background(&bars.session, Timing::default());
            let updates = follow.updates().clone();
            bars._background_follow = Some(follow);
            bars.tasks.push(cx.spawn(async move |this, cx| {
                while let Ok(update) = updates.recv().await {
                    if this.update(cx, |this, cx| this.take_background_update(update, cx)).is_err() {
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

    /// How a row opens its conversation.
    pub fn set_opener(&mut self, opener: Opener) {
        self.opener = Some(opener);
    }

    pub(crate) fn set_bot_look(&mut self, lookup: BotLook) {
        self.bot_look = Some(lookup);
    }

    pub(crate) fn set_wakes(&mut self, wakes: Option<wakes::Status>, computer: String, now: Option<f64>, cx: &mut Context<Self>) {
        if self.wakes != wakes || self.wake_computer != computer || self.now != now {
            self.wakes = wakes;
            self.wake_computer = computer;
            self.now = now;
            cx.notify();
        }
    }

    pub(crate) fn tick_wakes(&self, cx: &mut Context<Self>) {
        if self.wakes.is_some() && self.handle.is_open() {
            cx.notify();
        }
    }

    fn wake_label(&self) -> Option<String> {
        self.wakes.as_ref()?.chip_label(self.now.unwrap_or_else(crate::time::now_ms))
    }

    fn wake_source(&self) -> wakes::Source {
        wakes::Source { client: self.client.clone(), session: self.session.clone(), computer: self.wake_computer.clone(), live: self.live, now: self.now }
    }

    /// The bot this conversation is (none: not a bot's): its activity follows its work.
    pub fn set_bot(&mut self, bot: Option<String>, cx: &mut Context<Self>) {
        if self.bot == bot {
            return;
        }
        self.bot = bot.clone();
        self.work = None;
        self.work_connected = false;
        self.work_fresh = false;
        self.work_target = None;
        self.work_pending = false;
        self.rebuild_sessions();
        self.work_reads += 1;
        self._work_follow = None;
        self.work_seen = None;
        cx.notify();
        let Some(bot) = bot.filter(|_| self.live) else { return };
        let follow = self.client.follow_work_changes(Timing::default());
        let updates = follow.updates().clone();
        self._work_follow = Some(follow);
        self.tasks.push(cx.spawn(async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                let went = this.update(cx, |this, cx| {
                    if this.bot.as_deref() != Some(bot.as_str()) { return; }
                    match update {
                        Followed::Now(changes) => this.work_changed(changes.changed.get(&bot).copied(), cx),
                        Followed::Failed(_) => this.work_disconnected(cx),
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

    fn work_disconnected(&mut self, cx: &mut Context<Self>) {
        self.work_reads += 1;
        self.work_pending = false;
        self.work_connected = false;
        self.work_fresh = false;
        cx.notify();
    }

    fn work_changed(&mut self, at: Option<f64>, cx: &mut Context<Self>) {
        self.work_connected = true;
        if self.work_target == at && self.work_pending { return; }
        self.work_target = at;
        if !self.work_fresh || self.work_seen != at {
            self.read_work(cx);
        }
    }

    fn read_work(&mut self, cx: &mut Context<Self>) {
        let Some(bot) = self.bot.clone().filter(|_| self.work_connected) else { return };
        self.work_reads += 1;
        let (read, at) = (self.work_reads, self.work_target);
        self.work_pending = true;
        let client = self.client.clone();
        self.spawn(cx, move || client.bot_work(&bot), move |this, work, cx| {
            if this.work_reads != read { return; }
            this.work_pending = false;
            if let Ok(work) = work {
                this.work_seen = at;
                this.work_fresh = true;
                this.take_work(work);
            } else {
                this.work_fresh = false;
                // An unchanged feed may produce no more events after this failed detail read.
                this.tasks.push(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_secs(2)).await;
                    this.update(cx, |this, cx| {
                        if this.work_reads == read {
                            this.read_work(cx);
                        }
                    }).ok();
                }));
            }
            cx.notify();
        });
    }

    fn take_background_update(&mut self, update: Followed<Background>, cx: &mut Context<Self>) {
        match update {
            Followed::Now(background) => self.take_background(background, cx),
            Followed::Failed(_) => {
                self.background_reads += 1;
                self.background_fresh = false;
                cx.notify();
            }
        }
    }

    /// What runs for it besides its turn, as its box said.
    pub fn take_background(&mut self, background: Background, cx: &mut Context<Self>) {
        self.background_fresh = true;
        let sessions_changed = self.background.subagents != background.subagents;
        self.background = background;
        if sessions_changed {
            self.rebuild_sessions();
        }
        cx.notify();
    }

    /// What its box says now, read at once on this thread (tests and shots).
    #[cfg(feature = "stand-in")]
    pub fn read_at_once(&mut self, cx: &mut Context<Self>) {
        let background = self.client.session_background(&self.session);
        self.take_background_update(match background {
            Ok(value) => Followed::Now(value),
            Err(error) => Followed::Failed(error),
        }, cx);
        if let Some(bot) = &self.bot {
            match self.client.bot_work(bot) {
                Ok(work) => {
                    self.work_fresh = true;
                    self.take_work(work);
                }
                Err(_) => self.work_fresh = false,
            }
        }
        cx.notify();
    }

    pub fn background(&self) -> &Background {
        &self.background
    }

    pub fn at_work(&self) -> impl Iterator<Item = &WorkSession> {
        self.work.iter().flat_map(|work| &work.sessions)
            .filter(|session| session.session != self.session && session.is_at_work())
    }

    fn is_running(&self) -> bool {
        (self.background_fresh && (!self.background.commands.is_empty()
            || self.background.subagents.iter().any(|subagent| !subagent.asking)))
            || (self.work_fresh && self.at_work().any(|session| session.state == WorkState::Working))
    }

    fn take_work(&mut self, work: Work) {
        let changed = self.work.as_ref().is_none_or(|previous| previous.sessions != work.sessions);
        self.work = Some(work);
        if changed { self.rebuild_sessions(); }
    }

    fn rebuild_sessions(&mut self) {
        let mut ids = HashSet::new();
        let mut active = Vec::new();
        let mut recent = Vec::new();
        for session in self.work.iter().flat_map(|work| &work.sessions) {
            if session.session == self.session || !session.is_at_work() || !ids.insert(&session.session) { continue; }
            active.push(SessionRow::from_work(session));
        }
        for subagent in &self.background.subagents {
            if subagent.session != self.session && ids.insert(&subagent.session) {
                active.push(SessionRow {
                    id: subagent.session.clone(),
                    title: subagent.title.clone().unwrap_or_else(|| "Subagent".into()),
                    state: if subagent.asking { WorkState::Asking } else { WorkState::Working },
                    provider: None,
                });
            }
        }
        for session in self.work.iter().flat_map(|work| &work.sessions) {
            if session.session != self.session
                && matches!(session.state, WorkState::Finished | WorkState::Failed | WorkState::Stopped)
                && ids.insert(&session.session)
            {
                recent.push(SessionRow::from_work(session));
            }
        }
        self.active_rows = active.into();
        self.recent_rows = recent.into();
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
        let id = id.to_owned();
        self.spawn(cx, move || client.stop_command(&command), move |this, result, cx| {
            this.stopping.remove(&id);
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
        let stopped = id.clone();
        self.spawn(cx, move || client.stop_session(&id, &[]), move |this, result, cx| {
            this.stopping.remove(&stopped);
            if let Err(error) = result {
                this.failure = Some(format!("It didn’t stop: {}", kuvo_box::accounts::said(&error)));
            }
            this.after_stop(cx);
        });
    }

    fn stop_all(&mut self, cx: &mut Context<Self>) {
        let Some(bot) = self.bot.clone() else { return };
        self.stopping_all = true;
        self.failure = None;
        let client = self.client.clone();
        self.spawn(cx, move || client.stop_bot_work(&bot), |this, result, cx| {
            this.stopping_all = false;
            if let Err(error) = result {
                this.failure = Some(format!("The sessions didn’t stop: {}", kuvo_box::accounts::said(&error)));
            }
            this.after_stop(cx);
        });
        cx.notify();
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


}


struct SessionRow {
    id: String,
    title: String,
    state: WorkState,
    provider: Option<String>,
}

impl SessionRow {
    fn from_work(session: &WorkSession) -> Self {
        Self {
            id: session.session.clone(),
            title: session.title.clone().unwrap_or_else(|| "Session".into()),
            state: session.state,
            provider: session.model.as_ref().and_then(|model| model.provider_id.clone()),
        }
    }
}

fn sessions_label(working: usize, asking: usize) -> String {
    match (working, asking) {
        (0, n) => format!("{n} waiting for you"),
        (n, 0) => format!("{n} working"),
        (n, m) => format!("{n} working · {m} waiting for you"),
    }
}

impl Focusable for WorkBars {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.trigger_focus.clone()
    }
}

impl Render for WorkBars {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self.is_running();
        let c = cx.theme().colors.clone();
        let this = cx.entity().downgrade();
        let button = Button::icon_only(
            "conversation-activity-button", IconName::Activity,
            if running { "Activity · Work in progress" } else { "Activity" },
        )
            .ghost()
            .size(ButtonSize::Medium)
            .icon_size(IconSize::Large)
            .selected(self.handle.is_open())
            .track_focus(self.trigger_focus.clone());
        div()
            .debug_selector(|| "conversation-activity-button".into())
            .relative()
            .flex_none()
            .child(PopoverMenu::button("conversation-activity-menu", button, move |_, cx| {
                let bars = this.upgrade().expect("activity trigger is visible");
                cx.new(|cx| ActivityCard::new(&bars, false, cx))
            }).with_handle(self.handle.clone()).align_right())
            .children(running.then(|| div().absolute().top(px(3.)).right(px(3.))
                .debug_selector(|| "conversation-activity-running".into()).child(status_dot(c.accent))))
    }
}

impl WorkBars {
    #[cfg(feature = "stand-in")]
    pub fn open_activity(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_activity_with_wakes(false, window, cx);
    }

    #[cfg(feature = "stand-in")]
    pub fn open_process_output(&self, id: &str, cx: &mut Context<Self>) {
        let Some(card) = self.handle.view().and_then(|view| view.downcast::<ActivityCard>().ok()) else { return };
        let Some(command) = self.background.commands.iter().find(|command| command.id == id).cloned() else { return };
        card.update(cx, |card, cx| card.show_output(command, self.client.clone(), self.live, cx));
    }

    #[cfg(feature = "stand-in")]
    pub(crate) fn open_activity_with_wakes(&mut self, force_wakes: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Build from the data here: the WorkBars entity is being updated by the caller.
        let bars = cx.entity();
        let source = self.wake_source();
        let show_wakes = force_wakes || self.wake_label().is_some();
        let sessions = self.active_rows.clone();
        let card = cx.new(|cx| ActivityCard::with_source(&bars, source, show_wakes, force_wakes, &sessions, cx));
        self.handle.open(card, window, cx);
        cx.notify();
    }

    #[cfg(feature = "stand-in")]
    pub fn open_outputs_draft(&mut self, draft: crate::drafts::OutputsDraft, window: &mut Window, cx: &mut Context<Self>) {
        let bars = cx.entity();
        let source = self.wake_source();
        let client = self.client.clone();
        let sessions = self.active_rows.clone();
        let card = cx.new(|cx| {
            let mut card = ActivityCard::with_source(&bars, source, false, false, &sessions, cx);
            card.set_outputs_draft(draft, &client);
            card
        });
        self.handle.open(card, window, cx);
        cx.notify();
    }
}

enum Preview {
    Loading,
    Text(String),
    CurrentFile(String),
    Image(std::sync::Arc<gpui::Image>),
    Message(String),
}

struct ActivityCard {
    activity: WeakEntity<WorkBars>,
    focus: FocusHandle,
    output: Option<Shown>,
    reading_output: Option<Task<()>>,
    expanded: bool,
    provider_requests: Vec<String>,
    providers: HashMap<String, String>,
    reading_providers: Option<Task<()>>,
    outputs: Option<Outputs>,
    outputs_failed: bool,
    preview: Option<(String, Preview)>,
    reading: Option<Task<()>>,
    more: Option<Task<()>>,
    loaded_pages: usize,
    wake_schedule: Option<Entity<WakeSchedule>>,
    force_wakes: bool,
    #[cfg(feature = "stand-in")]
    outputs_draft: Option<crate::drafts::OutputsDraft>,
    #[cfg(feature = "stand-in")]
    draft_thumbs: HashMap<String, std::sync::Arc<gpui::Image>>,
    _activity_subscription: Subscription,
    _outputs: Option<(Follow<Followed<Outputs>>, Task<()>)>,
}

impl Focusable for ActivityCard {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<DismissEvent> for ActivityCard {}

impl ActivityCard {
    fn new(activity: &Entity<WorkBars>, force_wakes: bool, cx: &mut Context<Self>) -> Self {
        let bars = activity.read(cx);
        let show_wakes = force_wakes || bars.wake_label().is_some();
        let (source, sessions) = (bars.wake_source(), bars.active_rows.clone());
        Self::with_source(activity, source, show_wakes, force_wakes, &sessions, cx)
    }

    #[cfg(feature = "stand-in")]
    fn set_outputs_draft(&mut self, draft: crate::drafts::OutputsDraft, client: &BoxClient) {
        self.outputs_draft = Some(draft);
        self.draft_thumbs.clear();
        if let Some(page) = &self.outputs {
            for artifact in page.artifacts.iter().filter(|a| a.kind == "image") {
                let media_type = artifact.media_type.split(';').next().unwrap_or_default();
                if let Some(format) = gpui::ImageFormat::from_mime_type(media_type)
                    && let Ok(bytes) = client.artifact_bytes(artifact)
                {
                    self.draft_thumbs.insert(
                        artifact.id.clone(),
                        std::sync::Arc::new(gpui::Image::from_bytes(format, bytes)),
                    );
                }
            }
        }
    }

    fn with_source(activity: &Entity<WorkBars>, source: wakes::Source, show_wakes: bool, force_wakes: bool, sessions: &[SessionRow], cx: &mut Context<Self>) -> Self {
        let schedule = show_wakes.then(|| cx.new(|cx| WakeSchedule::embedded(source.clone(), cx)));
        let wakes::Source { client, session, live, .. } = source;
        let mut card = Self {
            activity: activity.downgrade(),
            focus: cx.focus_handle(),
            output: None,
            reading_output: None,
            expanded: false,
            provider_requests: Vec::new(),
            providers: HashMap::new(),
            reading_providers: None,
            outputs: None,
            outputs_failed: false,
            preview: None,
            reading: None,
            more: None,
            loaded_pages: 1,
            wake_schedule: schedule,
            force_wakes,
            #[cfg(feature = "stand-in")]
            outputs_draft: None,
            #[cfg(feature = "stand-in")]
            draft_thumbs: HashMap::new(),
            _activity_subscription: cx.observe(activity, |this: &mut ActivityCard, activity, cx| {
                let bars = activity.read(cx);
                let (client, live, sessions) = (bars.client.clone(), bars.live, bars.active_rows.clone());
                let command_gone = this.output.as_ref().is_some_and(|output| !bars.background.commands.iter().any(|command| command.id == output.command.id));
                let wake_source = (this.wake_schedule.is_none() && bars.wake_label().is_some()).then(|| bars.wake_source());
                if command_gone {
                    this.close_output(cx);
                }
                if let Some(source) = wake_source {
                    this.wake_schedule = Some(cx.new(|cx| WakeSchedule::embedded(source, cx)));
                }
                this.read_providers(client, live, &sessions, cx);
                cx.notify();
            }),
            _outputs: None,
        };
        card.read_providers(client.clone(), live, sessions, cx);
        if live {
            let follow = client.follow_session_outputs(&session, Timing::default());
            let updates = follow.updates().clone();
            let task = cx.spawn(async move |this, cx| {
                while let Ok(update) = updates.recv().await {
                    if this.update(cx, |this, cx| this.take_outputs(update, cx)).is_err() {
                        break;
                    }
                }
            });
            card._outputs = Some((follow, task));
        } else {
            let outputs = client.session_outputs(&session, None);
            card.take_outputs(match outputs { Ok(page) => Followed::Now(page), Err(error) => Followed::Failed(error) }, cx);
        }
        card
    }

    fn read_providers(&mut self, client: BoxClient, live: bool, sessions: &[SessionRow], cx: &mut Context<Self>) {
        let mut ids: Vec<_> = sessions.iter().filter(|row| row.provider.is_none()).map(|row| row.id.clone()).collect();
        ids.sort();
        if self.provider_requests == ids {
            return;
        }
        self.provider_requests = ids.clone();
        self.reading_providers = None;
        self.providers.retain(|id, _| ids.binary_search(id).is_ok());
        if ids.is_empty() {
            return;
        }
        let read = move || {
            let ids: Vec<_> = ids.iter().map(String::as_str).collect();
            client.session_models(&ids).ok().map(|states| {
                states.into_iter().filter_map(|(id, state)| Some((id, state.model?.provider_id))).collect()
            })
        };
        if !live {
            self.providers = read().unwrap_or_default();
            return;
        }
        let task = cx.background_executor().spawn(async move { read() });
        self.reading_providers = Some(cx.spawn(async move |this, cx| {
            let providers = task.await;
            this.update(cx, |this, cx| {
                if let Some(providers) = providers {
                    this.providers = providers;
                    cx.notify();
                }
            }).ok();
        }));
    }

    fn take_outputs(&mut self, update: Followed<Outputs>, cx: &mut Context<Self>) {
        match update {
            Followed::Now(page) if self.loaded_pages > 1 => {
                let Some(activity) = self.activity.upgrade() else { return };
                let bars = activity.read(cx);
                let (client, session, pages) = (bars.client.clone(), bars.session.clone(), self.loaded_pages);
                let task = cx.background_executor().spawn(async move {
                    refresh_outputs(page, pages, |cursor| client.session_outputs(&session, Some(cursor)))
                });
                self.more = Some(cx.spawn(async move |this, cx| {
                    let result = task.await;
                    this.update(cx, |this, cx| {
                        this.more = None;
                        match result {
                            Ok(outputs) => { this.outputs = Some(outputs); this.outputs_failed = false; }
                            Err(_) => this.outputs_failed = true,
                        }
                        cx.notify();
                    }).ok();
                }));
            }
            Followed::Now(page) => {
                self.outputs = Some(page);
                self.outputs_failed = false;
                // Leave a requested next page running; its cursor is based on a stable record.
            }
            Followed::Failed(_) => self.outputs_failed = true,
        }
        cx.notify();
    }

    fn more_outputs(&mut self, cx: &mut Context<Self>) {
        if self.more.is_some() { return; }
        let Some(before) = self.outputs.as_ref().and_then(|page| page.next.clone()) else { return };
        let Some(activity) = self.activity.upgrade() else { return };
        let bars = activity.read(cx);
        let (client, session) = (bars.client.clone(), bars.session.clone());
        self.loaded_pages += 1;
        let task = cx.background_executor().spawn(async move { client.session_outputs(&session, Some(&before)) });
        self.more = Some(cx.spawn(async move |this, cx| {
            let answer = task.await;
            this.update(cx, |this, cx| {
                this.more = None;
                match answer {
                    Ok(page) => if let Some(outputs) = &mut this.outputs {
                        append_outputs(outputs, page);
                        this.outputs_failed = false;
                    },
                    Err(_) => { this.loaded_pages -= 1; this.outputs_failed = true; }
                }
                cx.notify();
            }).ok();
        }));
        cx.notify();
    }

    fn open_output(&mut self, artifact: &Artifact, window: &mut Window, cx: &mut Context<Self>) {
        let Some(activity) = self.activity.upgrade() else { return };
        let client = activity.read(cx).client.clone();
        let artifact = artifact.clone();
        self.close_output(cx);
        window.focus(&self.focus, cx);
        self.preview = Some((artifact.name.clone(), Preview::Loading));
        let task = cx.background_executor().spawn(async move { read_preview(&client, &artifact) });
        self.reading = Some(cx.spawn(async move |this, cx| {
            let preview = task.await;
            this.update(cx, |this, cx| {
                if let Some((_, shown)) = &mut this.preview {
                    *shown = preview;
                    cx.notify();
                }
            }).ok();
        }));
        cx.notify();
    }

    fn dismiss(&self, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    fn close_output(&mut self, cx: &mut Context<Self>) {
        if self.output.take().is_some() {
            self.reading_output = None;
            cx.notify();
        }
    }

    /// Shows what a command has printed, read again every moment while it shows until it ends;
    /// again, it's put away.
    fn toggle_output(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.output.as_ref().is_some_and(|shown| shown.command.id == id) {
            self.close_output(cx);
            return;
        }
        let Some(activity) = self.activity.upgrade() else { return };
        let bars = activity.read(cx);
        let Some(command) = bars.background.commands.iter().find(|command| command.id == id).cloned() else { return };
        self.show_output(command, bars.client.clone(), bars.live, cx);
    }

    fn show_output(&mut self, command: Command, client: BoxClient, live: bool, cx: &mut Context<Self>) {
        self.output = Some(Shown { command: command.clone(), read: None });
        cx.notify();
        if !live {
            if let Some(shown) = self.output.as_mut() {
                shown.read = Some(client.command_output(&command));
            }
            return;
        }
        self.reading_output = Some(cx.spawn(async move |this, cx| {
            loop {
                let (client, asked) = (client.clone(), command.clone());
                let read = cx.background_executor().spawn(async move { client.command_output(&asked) }).await;
                let ended = read == Output::Ended;
                let shown = this.update(cx, |this, cx| {
                    let Some(shown) = this.output.as_mut().filter(|shown| shown.command.id == command.id) else { return false };
                    // Keep the last successful read through a temporary failure.
                    if read != Output::Unread || !matches!(shown.read, Some(Output::Printed(_))) {
                        shown.read = Some(read);
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
    fn output_words(&self) -> Option<(String, bool)> {
        let shown = self.output.as_ref()?;
        Some(match &shown.read {
            Some(Output::Printed(text)) if !text.is_empty() => (text.clone(), true),
            Some(Output::Printed(_)) => ("Nothing printed yet.".into(), false),
            Some(Output::Ended) => ("This command has ended. What it printed is no longer kept.".into(), false),
            Some(Output::Unread) => ("Couldn’t read what it printed. Its computer may be away; this tries again on its own.".into(), false),
            None => ("Reading what it printed…".into(), false),
        })
    }

    fn outputs(&self, cx: &Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let note = if self.outputs_failed {
            Some("Couldn’t read outputs.")
        } else if self.outputs.is_none() {
            Some("Loading outputs…")
        } else if self.outputs.as_ref().is_some_and(|page| page.artifacts.is_empty()) {
            Some("No outputs yet")
        } else {
            None
        };
        section("activity-outputs", "Outputs", cx)
            .children(note.map(|note| div().debug_selector(move || note.into()).px(space::S2).py(space::S1_5).text_color(c.text_muted).child(note)))
            .children(self.outputs.iter().flat_map(|page| &page.artifacts).map(|artifact| {
                let selector = format!("activity-output-{}", artifact.id);
                let icon = match artifact.kind.as_str() {
                    "image" => IconName::Image,
                    "page" => IconName::Globe,
                    "code" => IconName::Terminal,
                    "download" => IconName::ArrowDown,
                    _ => IconName::File,
                };
                let artifact = artifact.clone();
                row(selector, cx)
                    .child(mark(icon, c.icon))
                    .child(div().min_w_0().truncate().child(artifact.name.clone()))
                    .on_click(cx.listener(move |this, _, window, cx| this.open_output(&artifact, window, cx)))
            }))
            .children(self.outputs.as_ref().is_some_and(|page| page.next.is_some()).then(|| {
                Button::new("activity-more-outputs", "Show more").ghost().quiet().size(ButtonSize::Small)
                    .loading(self.more.is_some()).on_click(cx.listener(|this, _, _, cx| this.more_outputs(cx)))
            }))
            .into_any_element()
    }

    fn avatar(&self, bars: &WorkBars, session: &SessionRow, location: &str, cx: &App) -> AnyElement {
        let c = &cx.theme().colors;
        let provider = session.provider.as_ref().or_else(|| self.providers.get(&session.id));
        let bot = bars.bot_look.as_ref().and_then(|lookup| lookup(&session.id, bars.bot.as_deref(), cx));
        let size = px(20.);
        let inner = size - px(4.);
        let (label, face) = if let Some((id, look)) = bot {
            (format!("bot-{id}"), crate::bot_face::still_avatar(&id, &look, kuvo_ui::bot::State::Idle, inner))
        } else if let Some((provider, (service, path))) = provider.and_then(|provider| Some((provider, provider_mark(provider)?))) {
            let color = provider_mark_color(provider).or_else(|| kuvo_ui::icon::mark_color(service));
            (service.to_owned(), Mark::new(path).size(px(11.)).color(color.unwrap_or(c.text)).into_any_element())
        } else {
            ("unknown".into(), Icon::new(IconName::User).size(IconSize::Nav).color(c.icon).into_any_element())
        };
        let selector = format!("activity-{location}-avatar-{}-{label}", session.id);
        div().debug_selector(move || selector)
            .flex().flex_none().items_center().justify_center().size(size)
            .rounded(radius::PILL).border_2().border_color(c.elevated).bg(c.sunken)
            .child(face).into_any_element()
    }

    fn session_row(&self, bars: &WorkBars, session: &SessionRow, active: bool, cx: &Context<Self>) -> AnyElement {
        let c = &cx.theme().colors;
        let avatar = self.avatar(bars, session, "row", cx);
        let id = session.id.clone();
        let stop = id.clone();
        let status = match session.state {
            WorkState::Working => "Working",
            WorkState::Asking => "Waiting for you",
            WorkState::Finished => "Finished",
            WorkState::Failed => "Failed",
            WorkState::Stopped => "Stopped",
            _ => "",
        };
        div().flex().items_center().gap(space::S1)
            .child(row(format!("activity-session-{id}"), cx).flex_1().min_w_0()
                .child(avatar)
                .child(div().flex_1().min_w_0().truncate().child(session.title.clone()))
                .child(div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_muted).child(status))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.dismiss(cx);
                    this.activity.update(cx, |bars, cx| bars.open(&id, window, cx)).ok();
                })))
            .children(active.then(|| Button::icon_only(SharedString::from(format!("activity-stop-{stop}")), IconName::Stop, "Stop session").ghost().quiet()
                .size(ButtonSize::Small).disabled(bars.stopping_all || bars.stopping.contains(&stop))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.activity.update(cx, |bars, cx| bars.stop_session(&stop, cx)).ok();
                }))))
            .into_any_element()
    }

    fn sessions(&self, bars: &WorkBars, cx: &Context<Self>) -> Option<AnyElement> {
        let sessions = &bars.active_rows;
        let recent = &bars.recent_rows;
        if sessions.is_empty() && recent.is_empty() { return None; }
        let c = cx.theme().colors.clone();
        let asking = sessions.iter().filter(|session| session.state == WorkState::Asking).count();
        let label = sessions_label(sessions.len() - asking, asking);
        let can_stop = bars.bot.is_some() && bars.work_fresh && bars.at_work().next().is_some();
        let fold = (!sessions.is_empty()).then(|| row("activity-toggle-sessions", cx)
            .child(
                div().flex().flex_none().items_center()
                    .children(sessions.iter().take(3).enumerate().map(|(index, session)| {
                        div().flex_none().when(index > 0, |d| d.ml(px(-6.)))
                            .child(self.avatar(bars, session, "stack", cx))
                    }))
                    .children((sessions.len() > 3).then(|| {
                        div().debug_selector(|| "activity-avatar-overflow".into()).ml(space::S1)
                            .text_ui(TypeSize::Caption).text_color(c.text_muted).child(format!("+{}", sessions.len() - 3))
                    })),
            )
            .child(div().flex_1().min_w_0().truncate().child(label))
            .child(mark(if self.expanded { IconName::ChevronUp } else { IconName::ChevronDown }, c.text_faint))
            .on_click(cx.listener(|this, _, _, cx| {
                this.expanded = !this.expanded;
                cx.notify();
            })));
        Some(section("activity-sessions", if bars.bot.is_some() { "Sessions" } else { "Subagents" }, cx)
            .child(div().flex().items_center()
                .children(fold.map(|fold| fold.flex_1().min_w_0()))
                .children(can_stop.then(|| Button::new("activity-stop-all", "Stop All").ghost().quiet()
                    .size(ButtonSize::Small).loading(bars.stopping_all)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.activity.update(cx, |bars, cx| bars.stop_all(cx)).ok();
                    })))))
            .children(sessions.iter().filter(|_| self.expanded).map(|session| self.session_row(bars, session, true, cx)))
            .children((!recent.is_empty()).then(|| {
                section("activity-recent", "Recent", cx)
                    .children(recent.iter().map(|session| self.session_row(bars, session, false, cx)))
            }))
            .into_any_element())
    }

    fn processes(&self, bars: &WorkBars, cx: &Context<Self>) -> Option<AnyElement> {
        if bars.background.commands.is_empty() { return None; }
        let c = cx.theme().colors.clone();
        Some(section("activity-processes", "Background processes", cx)
            .children(bars.background.commands.iter().map(|command| {
                let id = command.id.clone();
                let toggle = id.clone();
                let selected = self.output.as_ref().is_some_and(|shown| shown.command.id == id);
                div().flex().flex_col()
                    .child(row(format!("activity-process-{id}"), cx).when(selected, |d| d.bg(c.hover))
                        .child(mark(IconName::TerminalWindow, c.icon))
                        .child(div().min_w_0().truncate().text_mono(TypeSize::Small).child(command.command.lines().next().unwrap_or("").to_owned()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_output(&toggle, cx);
                        })))
                    .children(selected.then(|| {
                        let (words, printed) = self.output_words().unwrap_or_default();
                        let stopping = bars.stopping.contains(&id);
                        div().id("command-output").debug_selector(|| "command-output".into())
                            .flex().flex_col().gap(space::S1).p(space::S2).rounded(radius::NAV).bg(c.sunken)
                            .child(div().id("command-output-scroll").max_h(px(150.)).overflow_y_scroll()
                                .text_mono(TypeSize::Caption).text_color(if printed { c.text } else { c.text_muted }).child(words))
                            .child(div().self_end().debug_selector(|| "activity-stop-process".into())
                                .child(Button::new("activity-stop-process", "Stop").ghost().quiet().size(ButtonSize::Small).disabled(stopping)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.activity.update(cx, |bars, cx| bars.stop_command(&id, cx)).ok();
                                    }))))
                    }))
            })).into_any_element())
    }

    fn preview(&self, name: &str, preview: &Preview, window: &Window, cx: &Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        div().flex().flex_col().debug_selector(|| "activity-output-preview".into())
            .child(div().flex().items_center().gap(space::S2).px(space::S2)
                .child(Button::icon_only("activity-preview-back", IconName::ChevronLeft, "Back to Activity").ghost().size(ButtonSize::Small)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.preview = None;
                        this.reading = None;
                        window.focus(&this.focus, cx);
                        cx.notify();
                    })))
                .child(div().min_w_0().truncate().child(name.to_owned())))
            .child(div().id("activity-preview-body").border_t_1().border_color(c.border).p(space::S2)
                .max_h(window.viewport_size().height / 2.).overflow_y_scroll()
                .child(match preview {
                    Preview::Text(text) => crate::panel::files::text_preview(name, text, text.lines().count(), cx),
                    Preview::CurrentFile(text) => div().flex().flex_col().gap(space::S2)
                        .child(div().debug_selector(|| "activity-current-file".into()).text_color(c.text_muted).child("Current file"))
                        .child(crate::panel::files::text_preview(name, text, text.lines().count(), cx)).into_any_element(),
                    Preview::Image(img) => gpui::img(img.clone()).w_full().max_h(window.viewport_size().height / 2. - space::S4)
                        .object_fit(gpui::ObjectFit::Contain).into_any_element(),
                    Preview::Loading => div().text_color(c.text_muted).child("Reading…").into_any_element(),
                    Preview::Message(why) => div().text_color(c.text_muted).child(why.clone()).into_any_element(),
                }))
            .into_any_element()
    }
}

impl Render for ActivityCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(activity) = self.activity.upgrade() else { return div().into_any_element() };
        let bars = activity.read(cx);
        #[cfg(feature = "stand-in")]
        if let Some(draft) = self.outputs_draft {
            let outputs = self.outputs.as_ref().map(|page| page.artifacts.as_slice()).unwrap_or(&[]);
            let sessions: Vec<crate::drafts::DraftSession> = bars
                .active_rows
                .iter()
                .map(|row| crate::drafts::DraftSession {
                    id: row.id.clone(),
                    title: row.title.clone(),
                    state: row.state,
                    provider: row.provider.clone().or_else(|| self.providers.get(&row.id).cloned()),
                })
                .collect();
            return crate::drafts::render_outputs_popover(
                draft,
                outputs,
                &self.draft_thumbs,
                &sessions,
                &bars.background.commands,
                window,
                cx,
            );
        }
        let theme = cx.theme().clone();
        let c = &theme.colors;
        let content = if let Some((name, preview)) = &self.preview {
            self.preview(name, preview, window, cx)
        } else {
            let wake = bars.wake_label().or_else(|| self.force_wakes.then(|| "Wake-ups & Watches".into()));
            let outputs = self.outputs(cx);
            let sessions = self.sessions(bars, cx);
            let processes = self.processes(bars, cx);
            let wake = wake.zip(self.wake_schedule.clone()).map(|(label, schedule)| {
                section("activity-wakes", "Wake-ups", cx)
                    .child(div().debug_selector(|| "activity-wake-status".into()).px(space::S2).py(space::S1).text_color(c.text_muted).child(label))
                    .child(schedule).into_any_element()
            });
            div().flex().flex_col()
                .children(((!bars.background_fresh && (!bars.background.commands.is_empty() || !bars.background.subagents.is_empty()))
                    || (!bars.work_fresh && bars.work.is_some())).then(|| {
                    div().debug_selector(|| "activity-last-known".into()).p(space::S2)
                        .text_ui(TypeSize::Caption).text_color(c.text_muted).child("Last known activity")
                }))
                .child(outputs)
                .children([sessions, processes, wake].into_iter().flatten().map(|section| {
                    div().mt(space::S2).pt(space::S2).border_t_1().border_color(c.border).child(section)
                }))
                .children(bars.failure.clone().map(|why| div().p(space::S2).text_color(c.warning).child(why)))
                .into_any_element()
        };
        div()
            .id("conversation-activity-popover")
            .debug_selector(|| "conversation-activity-popover".into())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.dismiss(cx);
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.dismiss(cx)))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .tab_group()
            .occlude()
            .w(px(if self.preview.is_some() { 460. } else { 300. }).min(window.viewport_size().width - px(16.)))
            .max_h(window.viewport_size().height - px(64.))
            .overflow_y_scroll()
            .p(space::S3)
            .rounded(radius::POPOVER)
            .bg(c.elevated)
            .border_1()
            .border_color(c.border)
            .shadow(theme.elevated_shadow())
            .text_ui(TypeSize::Small)
            .text_color(c.text)
            .child(content)
            .into_any_element()
    }
}

fn section(id: &'static str, title: &'static str, cx: &App) -> gpui::Div {
    div().debug_selector(move || id.into()).flex().flex_col().flex_none()
        .child(div().px(space::S2).py(space::S1_5).text_color(cx.theme().colors.text_muted).child(title))
}

fn row(id: impl Into<SharedString>, cx: &App) -> gpui::Stateful<gpui::Div> {
    let id = id.into();
    let c = &cx.theme().colors;
    div().id(id.clone()).debug_selector(move || id.to_string())
        .flex().flex_none().items_center().gap(space::S2).h(px(30.)).px(space::S2).rounded(radius::NAV)
        .tab_index(0).cursor_pointer().hover(|d| d.bg(c.hover))
        .focus_visible(|d| d.bg(c.hover).border_1().border_color(c.accent))
}

fn mark(icon: IconName, color: gpui::Hsla) -> impl IntoElement {
    div().flex_none().child(Icon::new(icon).size(IconSize::Medium).color(color))
}

fn append_outputs(outputs: &mut Outputs, page: Outputs) {
    let mut ids: HashSet<_> = outputs.artifacts.iter().map(|artifact| artifact.id.clone()).collect();
    outputs.artifacts.extend(page.artifacts.into_iter().filter(|artifact| ids.insert(artifact.id.clone())));
    outputs.next = page.next;
}

fn refresh_outputs<E>(mut first: Outputs, pages: usize, mut next: impl FnMut(&str) -> Result<Outputs, E>) -> Result<Outputs, E> {
    for _ in 1..pages {
        let Some(cursor) = first.next.as_deref() else { break };
        let page = next(cursor)?;
        append_outputs(&mut first, page);
    }
    Ok(first)
}

fn read_preview(client: &BoxClient, artifact: &Artifact) -> Preview {
    // For tracked files the artifact record points at Git rather than keeping a second copy.
    if artifact.gone.is_none() && artifact.latest.reason.as_deref() == Some("git") {
        return match client.read_text(&artifact.path) {
            Ok(file) if file.text => Preview::CurrentFile(file.content),
            Ok(_) => Preview::Message("A preview isn’t available for this file type.".into()),
            Err(error) => Preview::Message(error.to_string()),
        };
    }
    let bytes = match client.artifact_bytes(artifact) {
        Ok(bytes) => bytes,
        Err(error) => return Preview::Message(error.to_string()),
    };
    let media_type = artifact.media_type.split(';').next().unwrap_or_default();
    if let Some(format) = gpui::ImageFormat::from_mime_type(media_type) {
        return Preview::Image(std::sync::Arc::new(gpui::Image::from_bytes(format, bytes)));
    }
    if (media_type.starts_with("text/") || media_type == "application/json")
        && let Ok(text) = String::from_utf8(bytes)
    {
        return Preview::Text(text);
    }
    Preview::Message("A preview isn’t available for this file type.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stand_in::{Replies, StandIn};
    use serde_json::json;

    #[gpui::test]
    fn failed_activity_is_last_known_and_work_retries_the_same_timestamp(cx: &mut gpui::TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, 1000.);
        let bars = cx.update(|cx| cx.new(|cx| WorkBars::new(BoxClient::new(stand_in.clone()), "ses_mira".into(), false, cx)));
        cx.update(|cx| bars.update(cx, |bars, cx| {
            bars.set_bot(Some("bot_mira".into()), cx);
            bars.work_changed(Some(1.), cx);
        }));
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(bars.read(cx).is_running());
            assert_eq!(bars.read(cx).work_seen, Some(1.));
        });

        stand_in.set_down(true);
        cx.update(|cx| bars.update(cx, |bars, cx| {
            bars.work_changed(Some(2.), cx);
            bars.take_background_update(Followed::Failed(kuvo_box::Error::Unreachable("offline".into())), cx);
        }));
        cx.run_until_parked();
        cx.update(|cx| {
            let bars = bars.read(cx);
            assert!(!bars.is_running());
            assert_eq!(bars.work_seen, Some(1.));
            assert!(!bars.active_rows.is_empty());
        });

        stand_in.set_down(false);
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        cx.update(|cx| {
            let bars = bars.read(cx);
            assert_eq!(bars.work_seen, Some(2.));
            assert!(bars.is_running());
        });
        cx.update(|cx| bars.update(cx, |bars, cx| {
            bars.work_disconnected(cx);
            assert!(!bars.is_running());
            assert!(!bars.active_rows.is_empty());
        }));
    }

    #[gpui::test]
    fn session_rows_are_deduplicated_and_kept_when_background_is_unchanged(cx: &mut gpui::TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, 1000.);
        cx.update(|cx| {
            let bars = cx.new(|cx| WorkBars::new(BoxClient::new(stand_in), "ses_mira".into(), false, cx));
            bars.update(cx, |bars, cx| {
                bars.set_bot(Some("bot_mira".into()), cx);
                bars.read_at_once(cx);
                let background: Background = serde_json::from_value(json!({ "subagents": [
                    { "session": "ses_signup", "since": 1. },
                    { "session": "ses_extra", "since": 2. },
                    { "session": "ses_extra", "since": 3. }
                ] })).unwrap();
                bars.take_background(background.clone(), cx);
                let first = bars.active_rows.clone();
                assert_eq!(first.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(), ["ses_signup", "ses_cms", "ses_extra"]);
                bars.take_background(background, cx);
                assert!(Rc::ptr_eq(&first, &bars.active_rows));
            });
        });
    }

    #[test]
    fn a_live_update_refreshes_the_loaded_range_using_new_cursors() {
        fn page(ids: &[&str], next: Option<&str>) -> Outputs {
            serde_json::from_value(json!({
                "artifacts": ids.iter().map(|id| json!({
                    "id": id, "path": "/tmp/output", "name": "output",
                    "kind": "file", "type": "text/plain", "latest": {"n": 1}
                })).collect::<Vec<_>>(),
                "next": next,
            })).unwrap()
        }
        let updated = refresh_outputs(page(&["new", "a"], Some("after-a")), 3, |cursor| match cursor {
            "after-a" => Ok(page(&["b", "c"], Some("after-c"))),
            "after-c" => Ok(page(&["d", "e"], Some("after-e"))),
            _ => Err("unexpected cursor"),
        }).unwrap();
        assert_eq!(updated.artifacts.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(), ["new", "a", "b", "c", "d", "e"]);
        assert_eq!(updated.next.as_deref(), Some("after-e"));
    }

    #[test]
    fn mixed_activity_specifies_whom_sessions_wait_for() {
        assert_eq!(sessions_label(2, 1), "2 working · 1 waiting for you");
    }
}
