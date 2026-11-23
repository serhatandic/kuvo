//! Conversation activity in the header. The activity feeds stay live while the popover is closed.
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext as _, Context, DismissEvent, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    Pixels, StatefulInteractiveElement, Styled, StyledImage, Subscription, Task, WeakEntity,
    Window, div, prelude::FluentBuilder as _, px, relative,
};
use kuvo_box::areas::Look;
use kuvo_box::artifacts::{Artifact, Outputs, Version};
use kuvo_box::background::{Activity, ActivitySession, Background};
use kuvo_box::bots::{Work, WorkChanged, WorkSession, WorkState};
use kuvo_box::follow::Followed;
use kuvo_box::session_context::{ContextSize, SessionContext, Summarize, short_tokens};
use kuvo_box::{BoxClient, Follow, Where};
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TextRow, TypeSize,
    icon::{Mark, provider_mark, provider_mark_color},
    menu::{PopoverMenu, PopoverMenuHandle, Tooltip},
    small::status_dot,
    text_field::{TextField, TextFieldEvent},
    theme::{radius, space},
};

use crate::wakes::{self, WakeSchedule};

/// Opens a session's conversation (the workspace's, handed in).
pub type Opener = Rc<dyn Fn(&str, Option<&str>, &mut Window, &mut App)>;
/// Opens what the side panel shows for this conversation: a process's output, its subagents, a side
/// chat (the workspace's, handed in).
pub type PanelOpener = Rc<dyn Fn(crate::panel::Arrival, &mut Window, &mut App)>;
pub(crate) type BotLook = Rc<dyn Fn(&str, &App) -> Option<(String, Look)>>;

pub struct WorkBars {
    client: BoxClient,
    session: String,
    live: bool,
    /// The bot this conversation is, whose sessions show in a bar of their own.
    bot: Option<String>,
    background: Background,
    background_fresh: bool,
    last_activity_from_work: bool,
    work_connected: bool,
    work_fresh: bool,
    work_target: Option<f64>,
    work_pending: bool,
    stopping_all: bool,
    legacy_activity: Activity,
    /// What its box said of it at least once (or couldn't): the popover draws once all have.
    background_heard: bool,
    work_heard: bool,
    /// Sessions at work for it (its subagents are counted apart, in `subagent_rows`).
    active_rows: Rc<[SessionRow]>,
    subagent_rows: Rc<[SessionRow]>,
    recent_rows: Rc<[SessionRow]>,
    /// The popover's sections folded shut, kept while the conversation's open.
    collapsed: HashSet<&'static str>,
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
    panel: Option<PanelOpener>,
    bot_look: Option<BotLook>,
    _background_follow: Option<Follow<Followed<Background>>>,
    _work_follow: Option<Follow<Followed<WorkChanged>>>,
    /// When the bot's work was last said to change, as last seen.
    work_seen: Option<f64>,
    /// Where the conversation lives on its box, once known (its context is read there).
    place: Option<Where>,
    /// How full its context is, as its box last said (none: it doesn't say, or not yet measured).
    context: Option<ContextSize>,
    /// Where it's summarized by itself, to pick in the popover, as its box offers it.
    summarize: Option<Summarize>,
    /// Why the summarize point picked last didn't take, in its box's words.
    summarize_failure: Option<String>,
    /// Its context was read (or couldn't be): the popover draws once it has.
    context_heard: bool,
    context_reads: u64,
    /// The conversation as last told (running, how many messages): its context is read again as
    /// a step or a turn ends.
    told: Option<(bool, usize)>,
    /// Pinned: the card docks at the top right of the conversation, kept open as you chat. One
    /// setting for every conversation, kept with your settings (`crate::yours::ActivityPin`).
    pinned: bool,
    /// The pinned card shows everything (as the popover does), not the summary of what's live.
    full: bool,
    pinned_card: Option<Entity<ActivityCard>>,
    /// How wide the conversation was last drawn, so a pinned card keeps the column clear of it
    /// only where that leaves the column room.
    pane_width: Rc<std::cell::Cell<Pixels>>,
    tasks: Vec<Task<()>>,
}

/// Its Activity was pinned or unpinned: the conversation draws the card, or stops.
pub struct PinChanged;

impl EventEmitter<PinChanged> for WorkBars {}

/// The pinned card's width (T3 Code's is about this), and what the conversation keeps clear for it.
const PINNED_WIDTH: f32 = 292.;
const PINNED_RESERVE: f32 = PINNED_WIDTH + 28.;
/// The narrowest a conversation's column gets for a pinned card beside it; narrower, it floats over.
const PINNED_MIN_COLUMN: f32 = 440.;
/// More sessions at work than this, and the pinned card folds them to one line until opened.
const PINNED_FOLD_OVER: usize = 3;
/// How near the summarize point (of it) the strip turns to a warning.
const NEAR_SUMMARY: f64 = 0.9;

impl WorkBars {
    /// Activity in `session`'s conversation. `live`: follow its box (the app); else it shows what
    /// it's read with [`WorkBars::read_at_once`] (tests and shots).
    pub fn new(client: BoxClient, session: String, live: bool, cx: &mut Context<Self>) -> WorkBars {
        let crate::yours::ActivityPin { pinned, full } = crate::yours::activity_pin(cx);
        let mut bars = WorkBars {
            client: client.clone(),
            session,
            live,
            bot: None,
            background: Background::default(),
            background_fresh: false,
            last_activity_from_work: false,
            work_connected: false,
            work_fresh: false,
            work_target: None,
            work_pending: false,
            stopping_all: false,
            legacy_activity: Activity::default(),
            background_heard: false,
            work_heard: false,
            active_rows: Rc::from([]),
            subagent_rows: Rc::from([]),
            recent_rows: Rc::from([]),
            collapsed: HashSet::new(),
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
            panel: None,
            bot_look: None,
            _background_follow: None,
            _work_follow: None,
            work_seen: None,
            place: None,
            context: None,
            summarize: None,
            summarize_failure: None,
            context_heard: false,
            context_reads: 0,
            told: None,
            pinned,
            full,
            pinned_card: None,
            pane_width: Rc::new(std::cell::Cell::new(px(0.))),
            tasks: Vec::new(),
        };
        // Pinned or unpinned in another conversation, or Summary and Full switched there: this one
        // follows (one setting for all).
        cx.observe_global::<crate::yours::Store>(|this: &mut WorkBars, cx| this.follow_pin(cx)).detach();
        if live {
            // What runs for it, now and as the box says it changes (one past the counts' limit too).
            let follow = client.follow_session_background(&bars.session);
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
    pub fn set_panel_opener(&mut self, opener: PanelOpener) {
        self.panel = Some(opener);
    }

    pub fn session(&self) -> &str {
        &self.session
    }

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
        self.work_heard = false;
        self.rebuild_sessions();
        self.work_reads += 1;
        self._work_follow = None;
        self.work_seen = None;
        cx.notify();
        let Some(bot) = bot.filter(|_| self.live) else { return };
        let follow = self.client.follow_work_changes();
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
        if self.live && !self.activity().sessions.is_empty() {
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
        self.work_heard = true;
        self.work_reads += 1;
        self.work_pending = false;
        self.work_connected = false;
        self.work_stale();
        cx.notify();
    }

    fn work_stale(&mut self) {
        if std::mem::replace(&mut self.work_fresh, false) {
            self.rebuild_sessions();
        }
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
                this.take_work(work);
            } else {
                this.work_heard = true;
                this.work_stale();
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
                self.background_heard = true;
                self.background_reads += 1;
                if std::mem::replace(&mut self.background_fresh, false) {
                    self.rebuild_sessions();
                }
                cx.notify();
            }
        }
    }

    /// What runs for it besides its turn, as its box said.
    pub fn take_background(&mut self, background: Background, cx: &mut Context<Self>) {
        let sessions_changed = !self.background_fresh || self.background.activity != background.activity
            || (background.activity.is_none() && self.background.subagents != background.subagents);
        self.background_fresh = true;
        self.background_heard = true;
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
                Ok(work) => self.take_work(work),
                Err(_) => self.work_stale(),
            }
        }
        cx.notify();
    }

    pub fn background(&self) -> &Background {
        &self.background
    }

    /// Its box has said what runs for it (and its bot's work, for a bot's conversation), or that it
    /// couldn't: before then the popover shows it loading rather than drawing part of it.
    fn heard(&self) -> bool {
        !self.live || (self.background_heard && (self.bot.is_none() || self.work_heard))
    }

    fn is_collapsed(&self, section: &str) -> bool {
        let toggled = self.collapsed.contains(section);
        // The pinned card's sessions start folded when many are at work, so the corner stays calm.
        if section == PINNED_SESSIONS { toggled != self.pinned_busy() } else { toggled }
    }

    /// More sessions at work than the pinned card lists unasked.
    fn pinned_busy(&self) -> bool {
        self.active_rows.len() + self.subagent_rows.len() > PINNED_FOLD_OVER
    }

    fn fold(&mut self, section: &'static str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(section) {
            self.collapsed.insert(section);
        }
        cx.notify();
    }

    fn activity(&self) -> &Activity {
        self.box_activity().unwrap_or(&self.legacy_activity)
    }

    fn box_activity(&self) -> Option<&Activity> {
        let background = self.background.activity.as_ref();
        let work = self.work.as_ref().and_then(|work| work.activity.as_ref());
        if self.last_activity_from_work { work.or(background) } else { background.or(work) }
    }

    fn fresh_activity(&self) -> Option<&Activity> {
        self.background.activity.as_ref().filter(|_| self.background_fresh)
            .or_else(|| self.work.as_ref().filter(|_| self.work_fresh)?.activity.as_ref())
    }

    fn is_running(&self) -> bool {
        if self.background_fresh && !self.background.commands.is_empty() {
            return true;
        }
        if let Some(activity) = self.fresh_activity() {
            return activity.working > 0;
        }
        (self.background_fresh && self.background.subagents.iter().any(|row| !row.asking))
            || (self.work_fresh && self.work.iter().flat_map(|work| &work.sessions)
                .any(|row| row.session != self.session && row.state == WorkState::Working))
    }

    fn take_work(&mut self, work: Work) {
        self.work_heard = true;
        let changed = !self.work_fresh || self.work.as_ref() != Some(&work);
        self.work_fresh = true;
        self.work = Some(work);
        if changed { self.rebuild_sessions(); }
    }

    fn rebuild_sessions(&mut self) {
        if self.background_fresh && self.background.activity.is_some() {
            self.last_activity_from_work = false;
        } else if self.work_fresh && self.work.as_ref().is_some_and(|work| work.activity.is_some()) {
            self.last_activity_from_work = true;
        }
        let sessions = self.work.as_ref().map_or(&[][..], |work| &work.sessions);
        // Older boxes give the two feeds separately. Only use their legacy rows until a box
        // supplies the combined list with its own counts.
        if self.box_activity().is_none() {
            let mut activity = Activity::default();
            let mut ids = HashSet::new();
            for session in sessions.iter().filter(|row| row.session != self.session && row.is_at_work()) {
                ids.insert(session.session.as_str());
                activity.sessions.push(ActivitySession { session: session.session.clone(), title: session.title.clone().unwrap_or_else(|| "Session".into()), state: session.state, computer: None, subagent: false });
            }
            for subagent in self.background.subagents.iter().filter(|row| row.session != self.session && ids.insert(&row.session)) {
                let state = if subagent.asking { WorkState::Asking } else { WorkState::Working };
                activity.sessions.push(ActivitySession { session: subagent.session.clone(), title: subagent.title.clone().unwrap_or_else(|| "Subagent".into()), state, computer: None, subagent: true });
            }
            activity.asking = activity.sessions.iter().filter(|row| row.state == WorkState::Asking).count();
            activity.working = activity.sessions.len() - activity.asking;
            self.legacy_activity = activity;
        }
        let (subagents, active): (Vec<_>, Vec<_>) = self.activity().sessions.iter().map(|row| {
            let provider = sessions.iter().find(|session| session.session == row.session)
                .and_then(work_session_provider);
            SessionRow { id: row.session.clone(), title: row.title.clone(), state: row.state, provider, computer: row.computer.clone(), subagent: row.subagent }
        }).partition(|row| row.subagent);
        let recent = sessions.iter().filter(|session| session.session != self.session
            && matches!(session.state, WorkState::Finished | WorkState::Failed | WorkState::Stopped)
            && !active.iter().chain(&subagents).any(|row| row.id == session.session))
            .map(SessionRow::from_work).collect::<Vec<_>>();
        self.active_rows = active.into();
        self.subagent_rows = subagents.into();
        self.recent_rows = recent.into();
    }

    /// Opens a session's conversation (on its computer), to watch it and talk to it.
    pub fn open(&self, session: &str, computer: Option<&str>, window: &mut Window, cx: &mut App) {
        if let Some(opener) = self.opener.clone() {
            opener(session, computer, window, cx);
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

    /// The conversation as its view last read it: where it lives, whether it runs and how many
    /// messages it has. While its Activity shows, its context is read again as a step or turn ends.
    pub fn conversation_changed(&mut self, place: Where, running: bool, messages: usize, cx: &mut Context<Self>) {
        self.place = Some(place);
        if self.pinned && self.pinned_card.is_none() {
            let card = self.new_card(cx);
            card.update(cx, |card, _| card.pinned = true);
            self.pinned_card = Some(card);
            cx.emit(PinChanged);
        }
        let told = Some((running, messages));
        if self.told != told {
            self.told = told;
            if self.handle.is_open() || self.pinned {
                self.read_context(cx);
            }
        }
    }

    /// Reads how full its context is now, as its box says (where it fails, the last known stays).
    fn read_context(&mut self, cx: &mut Context<Self>) {
        self.context_reads += 1;
        let read = self.context_reads;
        let (client, session, place) = (self.client.clone(), self.session.clone(), self.place);
        let work = move || -> kuvo_box::Result<(Where, SessionContext)> {
            if let Some(place) = place {
                return Ok((place, client.session_context_and_point(&session, place)?));
            }
            let location = client.where_is(&session)?;
            let place = client.conversation_runtime(&session, &location)?.place;
            let owner = client.conversation_owner(&location).ok_or_else(|| kuvo_box::Error::Unreachable(client.describe()))?;
            Ok((place, owner.session_context_and_point(&session, place)?))
        };
        let then = move |this: &mut WorkBars, result: kuvo_box::Result<(Where, SessionContext)>, cx: &mut Context<Self>| {
            if this.context_reads != read { return; }
            this.context_heard = true;
            if let Ok((place, context)) = result {
                this.place = Some(place);
                this.context = context.size;
                this.summarize = context.summarize;
            }
            cx.notify();
        };
        if self.live {
            self.spawn(cx, work, then);
        } else {
            let result = work();
            then(self, result, cx);
        }
    }

    /// Picks where conversations are summarized (a choice's id), through its box's setting, then
    /// reads its context again for where the box puts the mark now.
    fn pick_summarize(&mut self, choice: String, cx: &mut Context<Self>) {
        if self.summarize.as_ref().is_some_and(|summarize| summarize.value == choice) {
            return;
        }
        self.summarize_failure = None;
        let client = self.client.clone();
        let work = move || client.pick_summarize_point(&choice);
        let then = |this: &mut WorkBars, result: kuvo_box::Result<()>, cx: &mut Context<Self>| {
            if let Err(error) = result {
                this.summarize_failure = Some(kuvo_box::accounts::said(&error));
            }
            this.read_context(cx);
            cx.notify();
        };
        if self.live {
            self.spawn(cx, work, then);
        } else {
            let result = work();
            then(self, result, cx);
        }
    }

    /// A card for its Activity, built from what's here (this may be mid-update).
    fn new_card(&self, cx: &mut Context<Self>) -> Entity<ActivityCard> {
        let bars = cx.entity();
        let source = self.wake_source();
        let show_wakes = self.wake_label().is_some();
        let (sessions, recent) = (self.active_rows.clone(), self.recent_rows.clone());
        cx.new(|cx| ActivityCard::with_source(&bars, source, show_wakes, false, &sessions, &recent, cx))
    }

    fn set_pinned(&mut self, pinned: bool, cx: &mut Context<Self>) {
        self.pinned = pinned;
        crate::yours::change(crate::yours::Change::Activity(crate::yours::ActivityPin { pinned, full: self.full }), cx);
        cx.emit(PinChanged);
        cx.notify();
    }

    /// The pinned card's view: Full (everything, as the popover lists it) or the summary of
    /// what's live. Kept with the pin.
    fn set_full(&mut self, full: bool, cx: &mut Context<Self>) {
        if self.full == full {
            return;
        }
        self.full = full;
        crate::yours::change(crate::yours::Change::Activity(crate::yours::ActivityPin { pinned: self.pinned, full }), cx);
        cx.notify();
    }

    /// Follows the pin as it was changed in another conversation's card.
    fn follow_pin(&mut self, cx: &mut Context<Self>) {
        let crate::yours::ActivityPin { pinned, full } = crate::yours::activity_pin(cx);
        if (pinned, full) == (self.pinned, self.full) {
            return;
        }
        self.full = full;
        if pinned != self.pinned {
            self.pinned = pinned;
            if pinned && self.pinned_card.is_none() {
                let card = self.new_card(cx);
                card.update(cx, |card, _| card.pinned = true);
                self.pinned_card = Some(card);
                self.read_context(cx);
            } else if !pinned {
                self.pinned_card = None;
            }
            cx.emit(PinChanged);
        }
        cx.notify();
    }

    /// Pins `card` (the popover's, or a new one): it floats at the top right of the conversation
    /// and stays as you chat, until closed or unpinned.
    fn pin(&mut self, card: Option<Entity<ActivityCard>>, window: &mut Window, cx: &mut Context<Self>) {
        let card = card.or_else(|| self.handle.view().and_then(|view| view.downcast::<ActivityCard>().ok()))
            .unwrap_or_else(|| self.new_card(cx));
        self.handle.close(window, cx);
        card.update(cx, |card, cx| {
            card.pinned = true;
            card.preview = None;
            card.reading = None;
            card.drilldown = None;
            cx.notify();
        });
        self.pinned_card = Some(card);
        self.set_pinned(true, cx);
    }

    /// Unpins it: the card closes (the conversation widens once; nothing else opens).
    fn unpin(&mut self, cx: &mut Context<Self>) {
        self.pinned_card = None;
        self.set_pinned(false, cx);
    }

    /// Its pinned card, floating at the top right of the conversation, and how much of the
    /// conversation's right it keeps clear for it (none where the column would get too narrow:
    /// there it floats over).
    pub fn pinned_layer(&self) -> Option<(AnyElement, Pixels)> {
        let card = self.pinned_card.clone().filter(|_| self.pinned)?;
        let width = self.pane_width.clone();
        // (Until it's measured, room is kept: the card docks at once rather than a frame later.)
        let measured = self.pane_width.get();
        let reserve = if measured == px(0.) || measured - px(PINNED_RESERVE) >= px(PINNED_MIN_COLUMN) { px(PINNED_RESERVE) } else { px(0.) };
        let measure = gpui::canvas(
            move |bounds, window, _| {
                if width.get() != bounds.size.width {
                    width.set(bounds.size.width);
                    // (A refresh while drawing is dropped: the next frame draws it.)
                    window.request_animation_frame();
                }
            },
            |_, _, _, _| {},
        ).size_full();
        let layer = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(div().absolute().top_0().left_0().size_full().child(measure))
            .child(div().absolute().top(space::S3).right(space::S3).child(card))
            .into_any_element();
        Some((layer, reserve))
    }

    #[cfg(feature = "stand-in")]
    pub fn pin_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pin(None, window, cx);
        self.read_context(cx);
    }

    /// Its pinned card in Full, or the summary (shots).
    #[cfg(feature = "stand-in")]
    pub fn show_full_now(&mut self, full: bool, cx: &mut Context<Self>) {
        self.set_full(full, cx);
    }

    /// Unpinned (shots).
    #[cfg(feature = "stand-in")]
    pub fn unpin_now(&mut self, cx: &mut Context<Self>) {
        self.unpin(cx);
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
    computer: Option<String>,
    subagent: bool,
}

impl SessionRow {
    fn from_work(session: &WorkSession) -> Self {
        Self {
            id: session.session.clone(),
            title: session.title.clone().unwrap_or_else(|| "Session".into()),
            state: session.state,
            provider: work_session_provider(session),
            computer: None,
            subagent: false,
        }
    }
}

fn work_session_provider(session: &WorkSession) -> Option<String> {
    let account = session.account.as_ref()
        .map(|account| account.title.as_deref().unwrap_or(&account.name));
    resolve_session_provider(
        session.model.as_ref().and_then(|model| model.provider_id.as_deref()),
        session.model.as_ref().map(|model| model.id.as_str()),
        account,
    )
}

/// Resolves a provider or coding-agent identifier into the mark used for session rows,
/// preferring the same service icons as the usage rings (Claude, Codex/ChatGPT, Antigravity/Gemini,
/// Grok, Cursor, etc.).
pub(crate) fn session_mark(provider: &str) -> Option<(&'static str, &'static str, Option<gpui::Hsla>)> {
    let base = provider.split('@').next().unwrap_or(provider).trim().to_ascii_lowercase();
    let canonical = match base.as_str() {
        "anthropic" | "claude" | "claude-code" => "claude",
        "openai" | "chatgpt" | "codex" | "codex-cli" => "codex",
        "antigravity" | "antigravity-cli" => "antigravity",
        "xai" | "grok" | "grok-cli" => "grok",
        "github-copilot" | "copilot" => "copilot",
        "ollama-cloud" | "ollama" => "ollama",
        "opencode-go" | "opencode" => "opencode",
        "zai-coding-plan" | "zhipuai" | "zhipuai-coding-plan" | "zai" => "zai",
        other => other,
    };
    let (service, path) = provider_mark(canonical)?;
    let color = provider_mark_color(canonical).or_else(|| kuvo_ui::icon::mark_color(service));
    Some((service, path, color))
}

/// Infers a session mark's provider key from a model identifier when no explicit provider is known
/// (for instance Auto-routed sessions or sessions whose model omits `providerID`).
pub(crate) fn infer_provider_from_model_id(model_id: &str) -> Option<&'static str> {
    let trimmed = model_id.trim();
    if let Some((prefix, rest)) = trimmed.split_once('/') {
        if let Some((service, _, _)) = session_mark(prefix) {
            return Some(service);
        }
        return infer_provider_from_model_id(rest);
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("claude") {
        Some("claude")
    } else if lower.starts_with("gpt")
        || lower.starts_with("chatgpt")
        || lower.starts_with("codex")
        || matches!(lower.as_str(), "o1" | "o3" | "o4")
        || lower.starts_with("o1-")
        || lower.starts_with("o3-")
        || lower.starts_with("o4-")
    {
        Some("codex")
    } else if lower.starts_with("antigravity") {
        Some("antigravity")
    } else if lower.starts_with("gemini") {
        Some("google")
    } else if lower.starts_with("grok") {
        Some("grok")
    } else if lower.starts_with("cursor") || lower.starts_with("composer") {
        Some("cursor")
    } else if lower.starts_with("deepseek") {
        Some("deepseek")
    } else if lower.starts_with("mistral") || lower.starts_with("codestral") || lower.starts_with("devstral") {
        Some("mistral")
    } else if lower.starts_with("llama") {
        Some("meta")
    } else if lower.starts_with("qwen") {
        Some("alibaba")
    } else if lower.starts_with("kimi") || lower.starts_with("moonshot") {
        Some("moonshotai")
    } else if lower.starts_with("glm") {
        Some("zai")
    } else if lower.starts_with("minimax") {
        Some("minimax")
    } else {
        None
    }
}

fn infer_provider_from_account(account: &str) -> Option<&'static str> {
    let head = account.split('·').next().unwrap_or(account).trim();
    if let Some((service, _, _)) = session_mark(head) {
        return Some(service);
    }
    let lower = head.to_ascii_lowercase();
    if lower.contains("claude") || lower.contains("anthropic") {
        Some("claude")
    } else if lower.contains("codex") || lower.contains("chatgpt") || lower.contains("openai") {
        Some("codex")
    } else if lower.contains("antigravity") {
        Some("antigravity")
    } else if lower.contains("gemini") || lower.contains("google") {
        Some("google")
    } else if lower.contains("grok") || lower.contains("xai") {
        Some("grok")
    } else if lower.contains("cursor") {
        Some("cursor")
    } else if lower.contains("copilot") {
        Some("copilot")
    } else if lower.contains("opencode") {
        Some("opencode")
    } else if lower.contains("openrouter") {
        Some("openrouter")
    } else if lower.contains("ollama") {
        Some("ollama")
    } else if lower.contains("z.ai") || lower.contains("zhipu") {
        Some("zai")
    } else {
        None
    }
}

/// Resolves the canonical provider/service key for a session from its explicit provider id, model id,
/// or account label, returning only keys that map to a known mark.
pub(crate) fn resolve_session_provider(
    provider_id: Option<&str>,
    model_id: Option<&str>,
    account: Option<&str>,
) -> Option<String> {
    if let Some(provider) = provider_id
        && let Some((service, _, _)) = session_mark(provider)
    {
        return Some(service.to_owned());
    }
    if let Some(model) = model_id
        && let Some(service) = infer_provider_from_model_id(model)
    {
        return Some(service.to_owned());
    }
    if let Some(account) = account
        && let Some(service) = infer_provider_from_account(account)
    {
        return Some(service.to_owned());
    }
    None
}

pub(crate) fn provider_from_model_state(state: &kuvo_box::models::ModelState) -> Option<String> {
    let provider_id = state
        .model
        .as_ref()
        .map(|model| model.provider_id.as_str())
        .or_else(|| state.plan.as_ref().map(|plan| plan.provider.as_str()))
        .or_else(|| state.account.as_ref().map(|account| account.id.as_str()));
    let model_id = state.model.as_ref().map(|model| model.id.as_str());
    let account = state.account.as_ref().map(|account| account.name.as_str());
    resolve_session_provider(provider_id, model_id, account)
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
        let dot = running.then(|| div().absolute().top(px(3.)).right(px(3.))
            .debug_selector(|| "conversation-activity-running".into()).child(status_dot(c.accent)));
        if self.pinned {
            // Pinned, its button closes the card, as T3 Code's does its panel.
            return div()
                .debug_selector(|| "conversation-activity-button".into())
                .relative()
                .flex_none()
                .child(Button::icon_only("conversation-activity-menu", IconName::Activity, "Activity · Pinned")
                    .ghost()
                    .size(ButtonSize::Medium)
                    .icon_size(IconSize::Large)
                    .selected(true)
                    .track_focus(self.trigger_focus.clone())
                    .on_click(cx.listener(|this, _, _, cx| this.unpin(cx))))
                .children(dot)
                .into_any_element();
        }
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
                bars.update(cx, |bars, cx| bars.read_context(cx));
                cx.new(|cx| ActivityCard::new(&bars, false, cx))
            }).with_handle(self.handle.clone()).align_right())
            .children(dot)
            .into_any_element()
    }
}

/// Filter category for the Outputs drill-down view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputKind {
    All,
    Images,
    Docs,
    Data,
    Code,
}

impl OutputKind {
    fn slug(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Images => "images",
            Self::Docs => "docs",
            Self::Data => "data",
            Self::Code => "code",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Images => "Images",
            Self::Docs => "Docs",
            Self::Data => "Data",
            Self::Code => "Code",
        }
    }

    fn count_label(self, n: usize) -> String {
        match self {
            Self::All => format!("{n} {}", if n == 1 { "item" } else { "items" }),
            Self::Images => format!("{n} {}", if n == 1 { "image" } else { "images" }),
            Self::Docs => format!("{n} {}", if n == 1 { "document" } else { "documents" }),
            Self::Data => format!("{n} {}", if n == 1 { "data file" } else { "data files" }),
            Self::Code => format!("{n} {}", if n == 1 { "code file" } else { "code files" }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FinishedFilter {
    All,
    Failed,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drilldown {
    Outputs(OutputKind),
    FinishedSessions,
}

impl WorkBars {
    #[cfg(feature = "stand-in")]
    pub fn open_activity(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_activity_with_wakes(false, window, cx);
    }

    #[cfg(feature = "stand-in")]
    pub fn open_activity_outputs(&mut self, kind: OutputKind, window: &mut Window, cx: &mut Context<Self>) {
        self.open_activity_with_wakes(false, window, cx);
        let Some(card) = self.handle.view().and_then(|view| view.downcast::<ActivityCard>().ok()) else { return };
        let (client, live) = (self.client.clone(), self.live);
        card.update(cx, |card, cx| card.show_outputs_kind_with(kind, client, live, window, cx));
    }

    /// Its popover opened with the summarize point's menu open over it (shots).
    #[cfg(feature = "stand-in")]
    pub fn open_activity_picking_summarize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_activity_with_wakes(false, window, cx);
        let Some(card) = self.handle.view().and_then(|view| view.downcast::<ActivityCard>().ok()) else { return };
        let Some(summarize) = self.summarize.clone() else { return };
        let menu = summarize_menu(summarize, cx.entity().downgrade(), cx);
        let handle = card.read(cx).summarize_menu.clone();
        handle.open(menu, window, cx);
        card.update(cx, |_, cx| cx.notify());
    }

    /// Its popover opened still loading, as it shows until every section has been read (shots).
    #[cfg(feature = "stand-in")]
    pub fn open_activity_loading(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_activity_with_wakes(false, window, cx);
        let Some(card) = self.handle.view().and_then(|view| view.downcast::<ActivityCard>().ok()) else { return };
        card.update(cx, |card, cx| {
            card.held_loading = true;
            cx.notify();
        });
    }

    /// A section of its popover folded, as if its heading was clicked (shots).
    #[cfg(feature = "stand-in")]
    pub fn fold_section(&mut self, section: &'static str, cx: &mut Context<Self>) {
        self.collapsed.insert(section);
        cx.notify();
    }

    #[cfg(feature = "stand-in")]
    pub fn open_activity_finished_sessions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_activity_with_wakes(false, window, cx);
        let Some(card) = self.handle.view().and_then(|view| view.downcast::<ActivityCard>().ok()) else { return };
        card.update(cx, |card, cx| card.show_finished_sessions(window, cx));
    }

    #[cfg(feature = "stand-in")]
    pub(crate) fn open_activity_with_wakes(&mut self, force_wakes: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Build from the data here: the WorkBars entity is being updated by the caller.
        let bars = cx.entity();
        let source = self.wake_source();
        let show_wakes = force_wakes || self.wake_label().is_some();
        let sessions = self.active_rows.clone();
        let recent = self.recent_rows.clone();
        let card = cx.new(|cx| ActivityCard::with_source(&bars, source, show_wakes, force_wakes, &sessions, &recent, cx));
        self.read_context(cx);
        self.handle.open(card, window, cx);
        cx.notify();
    }
}

/// The pinned card's sessions section, folded by its own rule.
const PINNED_SESSIONS: &str = "pinned-sessions";
const STRIP_IMAGES: usize = 3;
/// The most subagent marks in the Subagents row, each once.
const SUBAGENT_MARKS: usize = 4;
const GRID_THUMB_LIMIT: usize = 24;
const RECENT_FILES: usize = 3;
const INLINE_ACTIVE_SESSIONS: usize = 5;
const FINISHED_PAGE_SIZE: usize = 20;

enum Preview {
    Loading,
    Text(String),
    CurrentFile(String),
    Message(String),
}

struct ActivityCard {
    activity: WeakEntity<WorkBars>,
    focus: FocusHandle,
    /// When there are more than `INLINE_ACTIVE_SESSIONS` live sessions, whether the rest are shown.
    all_active: bool,
    drilldown: Option<Drilldown>,
    finished_filter: FinishedFilter,
    finished_limit: usize,
    finished_search: Entity<TextField>,
    _finished_search_sub: Subscription,
    provider_requests: Vec<String>,
    providers: HashMap<String, String>,
    reading_providers: Option<Task<()>>,
    outputs: Option<Outputs>,
    outputs_failed: bool,
    thumbnails: HashMap<String, (Version, Option<std::sync::Arc<gpui::Image>>)>,
    thumb_requests: Vec<(String, Version)>,
    reading_thumbs: Vec<Task<()>>,
    preview: Option<(String, Preview)>,
    reading: Option<Task<()>>,
    more: Option<Task<()>>,
    loaded_pages: usize,
    wake_schedule: Option<Entity<WakeSchedule>>,
    /// Draws again as its schedule is read (it waits for it before drawing).
    _wake_read: Option<Subscription>,
    force_wakes: bool,
    _activity_subscription: Subscription,
    _outputs: Option<(Follow<Followed<Outputs>>, Task<()>)>,
    /// The conversation's side chats, followed while the popover's open.
    side: Option<kuvo_box::side_chats::SideThreads>,
    /// Its side chats were read, or couldn't be, or its box keeps none.
    side_heard: bool,
    _side: Option<(Follow<Followed<kuvo_box::side_chats::SideThreads>>, Task<()>)>,
    /// Its subagents at work and done, as its box lists them (none: a box from before).
    subagents: Option<kuvo_box::subagents::Subagents>,
    subagents_heard: bool,
    _subagents: Option<(Follow<Followed<kuvo_box::subagents::Subagents>>, Task<()>)>,
    /// Pinned: drawn as the slim card at the top right of the conversation, kept open as you chat.
    pinned: bool,
    /// The summarize point's menu, while it's open (the card stays open under it).
    summarize_menu: PopoverMenuHandle,
    /// Held loading, to show how it looks (shots).
    #[cfg(feature = "stand-in")]
    held_loading: bool,
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
        let (source, sessions, recent) = (bars.wake_source(), bars.active_rows.clone(), bars.recent_rows.clone());
        Self::with_source(activity, source, show_wakes, force_wakes, &sessions, &recent, cx)
    }

    fn with_source(
        activity: &Entity<WorkBars>,
        source: wakes::Source,
        show_wakes: bool,
        force_wakes: bool,
        sessions: &[SessionRow],
        recent: &[SessionRow],
        cx: &mut Context<Self>,
    ) -> Self {
        let schedule = show_wakes.then(|| cx.new(|cx| WakeSchedule::embedded(source.clone(), cx)));
        let wakes::Source { client, session, live, .. } = source;
        let finished_search = cx.new(|cx| TextField::new(cx).bare().placeholder("Filter finished sessions…"));
        let finished_search_sub = cx.subscribe(&finished_search, |this: &mut ActivityCard, _, event: &TextFieldEvent, cx| {
            if let TextFieldEvent::Changed = event {
                this.finished_limit = FINISHED_PAGE_SIZE;
                cx.notify();
            }
        });
        let mut card = Self {
            activity: activity.downgrade(),
            focus: cx.focus_handle(),
            all_active: false,
            drilldown: None,
            finished_filter: FinishedFilter::All,
            finished_limit: FINISHED_PAGE_SIZE,
            finished_search,
            _finished_search_sub: finished_search_sub,
            provider_requests: Vec::new(),
            providers: HashMap::new(),
            reading_providers: None,
            outputs: None,
            outputs_failed: false,
            thumbnails: HashMap::new(),
            thumb_requests: Vec::new(),
            reading_thumbs: Vec::new(),
            preview: None,
            reading: None,
            more: None,
            loaded_pages: 1,
            summarize_menu: PopoverMenuHandle::default(),
            _wake_read: schedule.as_ref().map(|schedule| cx.observe(schedule, |_, _, cx| cx.notify())),
            wake_schedule: schedule,
            force_wakes,
            _activity_subscription: cx.observe(activity, |this: &mut ActivityCard, activity, cx| {
                let bars = activity.read(cx);
                let (client, live, sessions, recent) = (bars.client.clone(), bars.live, bars.active_rows.clone(), bars.recent_rows.clone());
                let wake_source = (this.wake_schedule.is_none() && bars.wake_label().is_some()).then(|| bars.wake_source());
                if let Some(source) = wake_source {
                    let schedule = cx.new(|cx| WakeSchedule::embedded(source, cx));
                    this._wake_read = Some(cx.observe(&schedule, |_, _, cx| cx.notify()));
                    this.wake_schedule = Some(schedule);
                }
                this.read_providers(client, live, &sessions, &recent, cx);
                cx.notify();
            }),
            _outputs: None,
            side: None,
            side_heard: false,
            _side: None,
            subagents: None,
            subagents_heard: false,
            _subagents: None,
            pinned: false,
            #[cfg(feature = "stand-in")]
            held_loading: false,
        };
        card.read_providers(client.clone(), live, sessions, recent, cx);
        if live {
            let follow = client.follow_session_outputs(&session);
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
            card.take_outputs_with(match outputs { Ok(page) => Followed::Now(page), Err(error) => Followed::Failed(error) }, client.clone(), false, cx);
        }
        // Its side chats, where its box keeps them as threads.
        if client.gate().has(kuvo_box::side_chats::SIDE_CHATS) {
            if live {
                let follow = client.follow_side_threads(&session);
                let updates = follow.updates().clone();
                let task = cx.spawn(async move |this, cx| {
                    while let Ok(update) = updates.recv().await {
                        let taken = this.update(cx, |this, cx| {
                            this.side_heard = true;
                            if let Followed::Now(threads) = update {
                                this.side = Some(threads);
                            }
                            cx.notify();
                        });
                        if taken.is_err() {
                            break;
                        }
                    }
                });
                card._side = Some((follow, task));
            } else {
                card.side = client.side_threads(&session).ok();
                card.side_heard = true;
            }
        } else {
            card.side_heard = true;
        }
        // Its subagents, at work and done, for the compact row that opens the side panel's tab.
        if client.gate().has(kuvo_box::subagents::SUBAGENTS) {
            if live {
                let follow = client.follow_subagents(&session);
                let updates = follow.updates().clone();
                let task = cx.spawn(async move |this, cx| {
                    while let Ok(update) = updates.recv().await {
                        let taken = this.update(cx, |this, cx| {
                            this.subagents_heard = true;
                            if let Followed::Now(list) = update {
                                this.subagents = Some(list);
                            }
                            cx.notify();
                        });
                        if taken.is_err() {
                            break;
                        }
                    }
                });
                card._subagents = Some((follow, task));
            } else {
                card.subagents = client.subagents(&session).ok();
                card.subagents_heard = true;
            }
        } else {
            card.subagents_heard = true;
        }
        card
    }

    /// Every section has what its box said (or that it couldn't say): until then the popover
    /// shows it loading, so nothing moves as each arrives.
    fn loaded(&self, bars: &WorkBars, cx: &App) -> bool {
        #[cfg(feature = "stand-in")]
        if self.held_loading {
            return false;
        }
        bars.heard()
            && (bars.context_heard || bars.context_reads == 0)
            && (self.outputs.is_some() || self.outputs_failed)
            && self.side_heard
            && self.subagents_heard
            && self.wake_schedule.as_ref().is_none_or(|schedule| schedule.read(cx).is_read())
    }

    /// The conversation's side chats, the latest first as its box lists them, each opening its tab
    /// in the side panel (as Codex's: a chat bubble and its title; "Answering…" while it is).
    fn side_chats(&self, bars: &WorkBars, cx: &Context<Self>) -> Option<AnyElement> {
        let threads = self.side.as_ref().filter(|side| !side.threads.is_empty())?;
        let c = cx.theme().colors.clone();
        let rows = threads.threads.iter().map(|thread| {
            let id = thread.id.clone();
            let mut row = TextRow::truncated(format!("activity-side-chat-{id}"), thread.title.clone())
                .debug_selector(format!("activity-side-chat-{id}"))
                .leading(Icon::new(IconName::Message).size(IconSize::Nav).color(c.icon));
            if thread.answering {
                row = row.trailing(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("Answering…"));
            }
            popover_row(row, cx).on_click(cx.listener(move |this, _, window, cx| {
                this.dismiss(cx);
                this.open_in_panel(crate::panel::Arrival::SideChat(id.clone()), window, cx);
            })).into_any_element()
        }).collect();
        Some(self.section(bars, "side-chats", "Side chats", None, None, rows, cx))
    }

    /// Its subagents in one row, as Codex's: their marks and how many work, wait on you and are
    /// done ("2 working · 6 done"), opening the side panel's Subagents tab. As its box lists them;
    /// a box from before lists only those at work.
    fn subagents(&self, bars: &WorkBars, cx: &Context<Self>) -> Option<AnyElement> {
        let c = cx.theme().colors.clone();
        let mut marks: Vec<Option<String>> = Vec::new();
        let (working, asking, done) = if let Some(list) = &self.subagents {
            for model in list.active.iter().map(|row| row.model.as_deref()).chain(list.done.iter().map(|row| row.model.as_deref())) {
                marks.push(model.and_then(infer_provider_from_model_id).map(str::to_owned));
            }
            let asking = list.active.iter().filter(|row| row.asking).count();
            (list.active.len() - asking, asking, list.done.len())
        } else {
            let rows = &bars.subagent_rows;
            for row in rows.iter() {
                marks.push(row.provider.clone().or_else(|| self.providers.get(&row.id).cloned()));
            }
            let asking = rows.iter().filter(|row| row.state == WorkState::Asking).count();
            (rows.len() - asking, asking, 0)
        };
        if working + asking + done == 0 {
            return None;
        }
        // Each mark once, in the order they're listed (the newest at work first).
        let mut seen = HashSet::new();
        let marks: Vec<Option<String>> = marks.into_iter().filter(|mark| seen.insert(mark.clone())).take(SUBAGENT_MARKS).collect();
        let mut words = Vec::new();
        if working > 0 { words.push(format!("{working} working")); }
        let attention = (asking > 0).then(|| {
            let start = words.iter().map(|word: &String| word.len() + 3).sum::<usize>();
            let word = format!("{asking} waiting for you");
            let range = start..start + word.len();
            words.push(word);
            range
        });
        if done > 0 { words.push(format!("{done} done")); }
        let label = words.join(" · ");
        let icons = div().flex().flex_none().items_center().gap(px(6.)).children(marks.into_iter().map(|mark| {
            let (selector, face) = match mark.as_deref().and_then(session_mark) {
                Some((service, path, color)) => (format!("activity-subagents-mark-{service}"), Mark::new(path).size(px(14.)).color(color.unwrap_or(c.text)).into_any_element()),
                None => ("activity-subagents-mark-unknown".to_owned(), Icon::new(IconName::Bot).size(IconSize::Nav).color(c.icon).into_any_element()),
            };
            div().debug_selector(move || selector).flex().flex_none().items_center().justify_center().size(px(16.)).child(face)
        }));
        let hover = c.hover;
        let row = div()
            .id("activity-subagents-row")
            .debug_selector(|| "activity-subagents-row".into())
            .w_full()
            .min_w_0()
            .h(px(28.))
            .px(space::S2)
            .flex()
            .items_center()
            .gap(space::S2)
            .rounded(radius::NAV)
            .tab_index(0)
            .cursor_pointer()
            .hover(move |d| d.bg(hover))
            .focus_visible(|d| d.bg(hover).border_1().border_color(c.accent))
            .tooltip(|_, cx| Tooltip::new("Open Subagents in the side panel").build(cx))
            .child(icons)
            .child({
                let selector = format!("activity-subagents-label-{label}");
                div().debug_selector(move || selector).flex_1().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text_muted)
                    .child(kuvo_ui::small::status_words(label, attention, cx))
            })
            .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).color(c.text_faint))
            .on_click(cx.listener(|this, _, window, cx| {
                this.dismiss(cx);
                this.open_in_panel(crate::panel::Arrival::Subagents(None), window, cx);
            }))
            .into_any_element();
        Some(self.section(bars, "subagents", "Subagents", None, None, vec![row], cx))
    }

    fn show_outputs_kind(&mut self, kind: OutputKind, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(activity) = self.activity.upgrade() {
            let bars = activity.read(cx);
            let (client, live) = (bars.client.clone(), bars.live);
            self.show_outputs_kind_with(kind, client, live, window, cx);
        } else {
            self.drilldown = Some(Drilldown::Outputs(kind));
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    fn show_outputs_kind_with(
        &mut self,
        kind: OutputKind,
        client: BoxClient,
        live: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.drilldown = Some(Drilldown::Outputs(kind));
        window.focus(&self.focus, cx);
        if kind == OutputKind::Images {
            self.read_thumbnails(client, live, cx);
        }
        cx.notify();
    }

    fn show_finished_sessions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.drilldown = Some(Drilldown::FinishedSessions);
        self.finished_filter = FinishedFilter::All;
        self.finished_limit = FINISHED_PAGE_SIZE;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn read_providers(
        &mut self,
        client: BoxClient,
        live: bool,
        sessions: &[SessionRow],
        recent: &[SessionRow],
        cx: &mut Context<Self>,
    ) {
        let mut ids: Vec<_> = sessions
            .iter()
            .chain(recent.iter())
            .filter(|row| row.provider.is_none() && row.computer.is_none())
            .map(|row| row.id.clone())
            .collect();
        ids.sort();
        ids.dedup();
        if self.provider_requests == ids {
            return;
        }
        self.provider_requests = ids.clone();
        self.reading_providers = None;
        self.providers.retain(|id, _| ids.binary_search(id).is_ok());
        if ids.is_empty() {
            return;
        }
        let expected = ids.clone();
        let read = move || {
            let ids: Vec<_> = ids.iter().map(String::as_str).collect();
            client.session_models(&ids).ok().map(|states| {
                states
                    .into_iter()
                    .filter_map(|(id, state)| {
                        let provider = provider_from_model_state(&state)?;
                        Some((id, provider))
                    })
                    .collect()
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
                if this.provider_requests == expected
                    && let Some(providers) = providers
                {
                    this.providers = providers;
                    cx.notify();
                }
            }).ok();
        }));
    }

    fn thumb_budget(&self) -> usize {
        if matches!(self.drilldown, Some(Drilldown::Outputs(OutputKind::Images))) {
            GRID_THUMB_LIMIT
        } else {
            STRIP_IMAGES
        }
    }

    fn read_thumbnails(&mut self, client: BoxClient, live: bool, cx: &mut Context<Self>) {
        let Some(page) = &self.outputs else { return };
        let limit = self.thumb_budget();
        let visible_ids: HashSet<&str> = page
            .artifacts
            .iter()
            .filter(|a| output_kind(a) == OutputKind::Images)
            .take(limit)
            .map(|a| a.id.as_str())
            .collect();
        self.thumbnails.retain(|id, _| visible_ids.contains(id.as_str()));

        let pending: Vec<_> = page.artifacts.iter().filter(|a| output_kind(a) == OutputKind::Images).take(limit)
            .filter(|a| self.thumbnails.get(&a.id).is_none_or(|(version, img)| *version != a.latest || img.is_none()))
            .cloned().collect();
        let keys: Vec<_> = pending.iter().map(|a| (a.id.clone(), a.latest.clone())).collect();
        if keys.is_empty() {
            self.thumb_requests.clear();
            self.reading_thumbs.clear();
            return;
        }
        if self.thumb_requests == keys {
            return;
        }
        self.thumb_requests = keys;
        if !live {
            for artifact in pending {
                let (id, n, image) = read_thumbnail(&client, artifact);
                self.thumbnails.insert(id, (n, image));
            }
            self.thumb_requests.clear();
            return;
        }
        self.reading_thumbs = pending.into_iter().map(|artifact| {
            let client = client.clone();
            let task = cx.background_executor().spawn(async move { read_thumbnail(&client, artifact) });
            cx.spawn(async move |this, cx| {
                let (id, version, image) = task.await;
                this.update(cx, |this, cx| {
                    this.thumb_requests.retain(|(reading, _)| reading != &id);
                    this.thumbnails.insert(id, (version, image));
                    cx.notify();
                }).ok();
            })
        }).collect();
    }

    fn take_outputs(&mut self, update: Followed<Outputs>, cx: &mut Context<Self>) {
        let Some(activity) = self.activity.upgrade() else { return };
        let bars = activity.read(cx);
        let (client, live) = (bars.client.clone(), bars.live);
        self.take_outputs_with(update, client, live, cx);
    }

    fn take_outputs_with(&mut self, update: Followed<Outputs>, client: BoxClient, live: bool, cx: &mut Context<Self>) {
        match update {
            Followed::Now(page) if self.loaded_pages > 1 => {
                let Some(activity) = self.activity.upgrade() else { return };
                let bars = activity.read(cx);
                let (session, pages) = (bars.session.clone(), self.loaded_pages);
                let fetch_client = client.clone();
                let task = cx.background_executor().spawn(async move {
                    refresh_outputs(page, pages, |cursor| fetch_client.session_outputs(&session, Some(cursor)))
                });
                self.more = Some(cx.spawn(async move |this, cx| {
                    let result = task.await;
                    this.update(cx, |this, cx| {
                        this.more = None;
                        match result {
                            Ok(outputs) => {
                                this.outputs = Some(outputs);
                                this.outputs_failed = false;
                                this.read_thumbnails(client, live, cx);
                            }
                            Err(_) => this.outputs_failed = true,
                        }
                        cx.notify();
                    }).ok();
                }));
            }
            Followed::Now(page) => {
                self.outputs = Some(page);
                self.outputs_failed = false;
                self.read_thumbnails(client, live, cx);
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
        let (client, session, live) = (bars.client.clone(), bars.session.clone(), bars.live);
        let fetch_client = client.clone();
        self.loaded_pages += 1;
        let task = cx.background_executor().spawn(async move { fetch_client.session_outputs(&session, Some(&before)) });
        self.more = Some(cx.spawn(async move |this, cx| {
            let answer = task.await;
            this.update(cx, |this, cx| {
                this.more = None;
                match answer {
                    Ok(page) => if let Some(outputs) = &mut this.outputs {
                        append_outputs(outputs, page);
                        this.outputs_failed = false;
                        this.read_thumbnails(client, live, cx);
                    },
                    Err(_) => { this.loaded_pages -= 1; this.outputs_failed = true; }
                }
                cx.notify();
            }).ok();
        }));
        cx.notify();
    }

    fn open_image_output(&mut self, artifact_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(activity) = self.activity.upgrade() else { return };
        let bars = activity.read(cx);
        let client = bars.client.clone();
        let return_focus = bars.trigger_focus.clone();
        let here = client.here();
        let images: Vec<Artifact> = self
            .outputs
            .as_ref()
            .map(|page| page.artifacts.iter().filter(|a| output_kind(a) == OutputKind::Images).cloned().collect())
            .unwrap_or_default();
        if images.is_empty() {
            return;
        }
        let active = images.iter().position(|a| a.id == artifact_id).unwrap_or(0);
        let items: Vec<crate::image_viewer::ViewerItem> = images
            .into_iter()
            .map(|artifact| {
                let id = artifact.id.clone();
                let name = artifact.name.clone();
                let local_path = (here && artifact.gone.is_none() && !artifact.path.is_empty())
                    .then(|| std::path::PathBuf::from(&artifact.path));
                let load_client = client.clone();
                let loader: std::sync::Arc<dyn Fn() -> crate::pictures::Read + Send + Sync> =
                    std::sync::Arc::new(move || {
                        match load_client.artifact_bytes(&artifact) {
                            Ok(bytes) => crate::pictures::shown(bytes),
                            Err(kuvo_box::Error::NotFound) => crate::pictures::Read::Missing,
                            Err(kuvo_box::Error::Refused(refusal)) if matches!(refusal.status, 404 | 410) => {
                                crate::pictures::Read::Missing
                            }
                            Err(_) => crate::pictures::Read::Failed {
                                why: "Couldn’t load this image.".into(),
                                again: true,
                            },
                        }
                    });
                crate::image_viewer::ViewerItem {
                    id,
                    name,
                    caption: None,
                    source_tag: "Outputs".into(),
                    local_path,
                    read: crate::pictures::Read::Reading,
                    loader: Some(loader),
                }
            })
            .collect();
        self.dismiss(cx);
        crate::image_viewer::open(
            crate::image_viewer::OpenRequest {
                items,
                active,
                return_focus: Some(return_focus),
                on_change: None,
            },
            window,
            cx,
        );
    }

    fn open_output(&mut self, artifact: &Artifact, window: &mut Window, cx: &mut Context<Self>) {
        if output_kind(artifact) == OutputKind::Images {
            self.open_image_output(&artifact.id, window, cx);
            return;
        }
        let Some(activity) = self.activity.upgrade() else { return };
        let client = activity.read(cx).client.clone();
        let artifact = artifact.clone();
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

    /// Closes the popover (a pinned card stays: it goes only by Unpin or its button).
    fn dismiss(&self, cx: &mut Context<Self>) {
        if !self.pinned {
            cx.emit(DismissEvent);
        }
    }

    /// Pins it, or unpins it (closed): after this event, as the card moves from the popover to
    /// the conversation.
    fn set_pinned(&self, pinned: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (bars, card) = (self.activity.clone(), cx.entity());
        window.defer(cx, move |window, cx| {
            bars.update(cx, |bars, cx| if pinned { bars.pin(Some(card), window, cx) } else { bars.unpin(cx) }).ok();
        });
    }

    /// The popover's heading: its name, and Pin.
    fn heading(&self, cx: &Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        div()
            .debug_selector(|| "activity-heading".into())
            .px(space::S2)
            .pb(space::S1)
            .flex()
            .items_center()
            .justify_between()
            .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child("Activity"))
            .child(Button::icon_only("activity-pin", IconName::Pin, "Pin beside the conversation")
                .ghost()
                .size(ButtonSize::Small)
                .on_click(cx.listener(|this, _, window, cx| this.set_pinned(true, window, cx))))
            .into_any_element()
    }

    /// How full its context is, as its box says: used of its model's window (from 0 before a
    /// measurement), with a mark where it's summarized by itself and, under it, that point to pick
    /// (the box's setting, for every conversation); near it, in the warning colour. None where its
    /// box says neither.
    fn context_strip(&self, bars: &WorkBars, cx: &Context<Self>) -> Option<AnyElement> {
        let context = bars.context;
        let summarize = bars.summarize.clone();
        if context.is_none() && summarize.is_none() {
            return None;
        }
        let c = cx.theme().colors.clone();
        let near = context.as_ref().is_some_and(context_near);
        let figures = context.as_ref().map(|context| {
            let words = context_words(context);
            let selector = format!("activity-context-words-{words}");
            div()
                .debug_selector(move || selector)
                .text_mono(TypeSize::Caption)
                .when(!self.pinned, |d| d.medium())
                .text_color(if near { c.warning } else if self.pinned { c.text_muted } else { c.text })
                .child(words)
        });
        // (Pinned, the card's controls have a row of their own above it.)
        let control = (!self.pinned).then(|| {
            Button::icon_only("activity-pin", IconName::Pin, "Pin beside the conversation")
                .ghost()
                .size(ButtonSize::Small)
                .on_click(cx.listener(|this, _, window, cx| this.set_pinned(true, window, cx)))
        });
        let top = div().flex().items_center().justify_between().gap(space::S2)
            .child(div().min_w_0().flex().items_baseline().gap(space::S1_5)
                .child(div().flex_none().text_ui(TypeSize::Small).medium().text_color(c.text).child("Context"))
                .children(figures))
            .children(control);
        // (Where the box doesn't offer the point to pick, near it says so on a line of its own.)
        let note = context.and_then(|context| context.summarize_at).filter(|_| near && summarize.is_none()).map(|at| {
            div().debug_selector(|| "activity-context-near".into()).text_ui(TypeSize::Caption).text_color(c.warning)
                .child(format!("Summarizes at {}", short_tokens(at)))
        });
        let picker = summarize.map(|summarize| self.summarize_row(summarize, near, cx));
        Some(div()
            .debug_selector(|| "activity-context".into())
            .px(space::S2)
            .pb(space::S1)
            .flex()
            .flex_col()
            .gap(space::S1_5)
            .child(top)
            .children(context.as_ref().and_then(|context| context_bar(context, cx)))
            .children(note)
            .children(picker)
            .children(bars.summarize_failure.clone().map(|why| kuvo_ui::Notice::said("activity-summarize-failure", kuvo_ui::Tone::Warning, why)))
            .into_any_element())
    }

    /// "Summarize at", and the point in use, which opens the choices: the box's setting for
    /// every conversation, as Settings › Conversations has it.
    fn summarize_row(&self, summarize: Summarize, near: bool, cx: &Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let open = self.summarize_menu.is_open();
        let current = summarize.current().map(|choice| choice.title.clone()).unwrap_or_else(|| summarize.value.clone());
        let selector = format!("activity-summarize-at-{current}");
        let trigger = div()
            .id("activity-summarize")
            .debug_selector(move || selector)
            .flex()
            .items_center()
            .gap(space::S0_5)
            .px(space::S1_5)
            .h(px(20.))
            .rounded(px(5.))
            .cursor_pointer()
            .when(open, |d| d.bg(c.hover))
            .hover(|d| d.bg(c.hover))
            .text_color(if near { c.warning } else { c.text })
            .child(current)
            .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(if near { c.warning } else { c.text_muted }));
        let label = summarize.label.clone();
        let activity = self.activity.clone();
        let menu = PopoverMenu::new("activity-summarize-menu", trigger, move |_, cx| summarize_menu(summarize.clone(), activity.clone(), cx))
        .with_handle(self.summarize_menu.clone())
        .align_right();
        div()
            .when(near, |d| d.debug_selector(|| "activity-context-near".into()))
            .flex()
            .items_center()
            .justify_between()
            .text_ui(TypeSize::Caption)
            .text_color(if near { c.warning } else { c.text_muted })
            .child(label)
            .child(menu)
            .into_any_element()
    }

    /// At the pinned card's top right: Summary or Full (kept with the pin), and Unpin, which
    /// closes it (the conversation widens once; nothing else opens).
    fn pinned_controls(&self, cx: &Context<Self>) -> AnyElement {
        let full = self.activity.upgrade().is_some_and(|bars| bars.read(cx).full);
        let dark = cx.theme().appearance.is_dark();
        let choice = |label: &'static str, on: bool, to_full: bool| {
            (SharedString::from(format!("activity-view-{label}")), SharedString::from(label), on, cx.listener(move |this: &mut ActivityCard, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<ActivityCard>| {
                this.show_full(to_full, cx);
            }))
        };
        let switch = crate::insights::segmented(dark, vec![choice("Summary", !full, false), choice("Full", full, true)], TypeSize::Caption, cx);
        div().flex_none().flex().items_center().gap(space::S1)
            .child(switch)
            .child(Button::icon_only("activity-unpin", IconName::Pin, "Unpin")
                .ghost()
                .size(ButtonSize::Small)
                .selected(true)
                .on_click(cx.listener(|this, _, window, cx| this.set_pinned(false, window, cx))))
            .into_any_element()
    }

    /// The pinned card's view: Full, or the summary (leaving a row opened in it).
    fn show_full(&mut self, full: bool, cx: &mut Context<Self>) {
        if !full {
            self.preview = None;
            self.reading = None;
            self.drilldown = None;
        }
        self.activity.update(cx, |bars, cx| bars.set_full(full, cx)).ok();
        cx.notify();
    }

    /// The pinned card's top: "Activity" with the card's controls, then its context (where its box
    /// says how full it is).
    fn pinned_top(&self, bars: &WorkBars, cx: &Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        // Summary or Full and Unpin on a row of their own, so the context's figures keep the
        // card's width.
        let controls = div().px(space::S2).pb(space::S2).flex().items_center().justify_between()
            .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child("Activity"))
            .child(self.pinned_controls(cx));
        div().flex().flex_col().child(controls).children(self.context_strip(bars, cx)).into_any_element()
    }

    /// The pinned card's summary: only what's live, as T3 Code's pinned panel keeps its rows: its
    /// context, the sessions at work (folded when many are), and a line each for subagents and
    /// processes while any run. (What it made is in Full.)
    fn pinned_view(&self, bars: &WorkBars, cx: &Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let hairline = || div().my(space::S1_5).h(px(1.)).w_full().bg(c.border).into_any_element();
        let mut card = div().flex().flex_col().child(self.pinned_top(bars, cx));
        if !self.loaded(bars, cx) {
            return card.child(div().debug_selector(|| "activity-loading".into()).h(px(40.)).flex().items_center().justify_center()
                .child(kuvo_ui::small::LoadingLabel::new("activity-loading-label", "Loading…"))).into_any_element();
        }

        let sessions: Vec<&SessionRow> = bars.active_rows.iter().collect();
        if !sessions.is_empty() {
            let asking = sessions.iter().filter(|row| row.state == WorkState::Asking).count();
            let working = sessions.len() - asking;
            // ("2 working" alone where that's all of them: never "2 · 2 working".)
            let mut words = if asking > 0 { vec![sessions.len().to_string()] } else { Vec::new() };
            if working > 0 { words.push(format!("{working} working")); }
            let attention = (asking > 0).then(|| {
                let start = words.iter().map(|word| word.len() + " · ".len()).sum::<usize>();
                let word = format!("{asking} waiting");
                let range = start..start + word.len();
                words.push(word);
                range
            });
            let label = words.join(" · ");
            let selector = format!("activity-pinned-sessions-meta-{label}");
            let meta = div().debug_selector(move || selector).text_ui(TypeSize::Caption).text_color(c.text_faint)
                .child(kuvo_ui::small::status_words(label, attention, cx)).into_any_element();
            let rows = sessions.iter().map(|row| self.pinned_session_row(bars, row, cx)).collect();
            card = card.child(hairline()).child(self.section(bars, PINNED_SESSIONS, "Sessions", Some(meta), None, rows, cx));
        }

        // Subagents at work (or waiting on you) now, as its box lists them; a box from before: those
        // its activity lists.
        let subagents = match &self.subagents {
            Some(list) => list.active.len(),
            None => bars.subagent_rows.len(),
        };
        let commands = bars.background.commands.len();
        if subagents > 0 || commands > 0 {
            card = card.child(hairline());
        }
        // (Each opens Full, in the card: the conversation never moves while it's pinned.)
        if subagents > 0 {
            let trailing = div().flex().items_center().gap(space::S1_5).text_ui(TypeSize::Caption).text_color(c.text_faint)
                .child(format!("{subagents} running")).child(status_dot(c.success));
            card = card.child(summary_row("activity-pinned-subagents", IconName::Bot, "Subagents", trailing.into_any_element(), cx)
                .tooltip(|_, cx| Tooltip::new("Show everything").build(cx))
                .on_click(cx.listener(|this, _, _, cx| this.show_full(true, cx))));
        }
        if commands > 0 {
            let trailing = div().flex().items_center().gap(space::S1_5).text_ui(TypeSize::Caption).text_color(c.text_faint)
                .child(format!("{commands} running")).child(status_dot(c.success));
            card = card.child(summary_row("activity-pinned-processes", IconName::TerminalWindow, "Processes", trailing.into_any_element(), cx)
                .tooltip(|_, cx| Tooltip::new("Show running processes").build(cx))
                .on_click(cx.listener(|this, _, _, cx| this.show_full(true, cx))));
        }
        if sessions.is_empty() && subagents == 0 && commands == 0 && bars.context.is_none() {
            card = card.child(div().debug_selector(|| "activity-empty".into()).px(space::S2).py(space::S1_5)
                .text_ui(TypeSize::Caption).text_color(c.text_muted).child("Nothing running."));
        }
        card.children(bars.failure.clone().map(|why| div().px(space::S2).pt(space::S1).child(kuvo_ui::Notice::said("work-bars-failure", kuvo_ui::Tone::Warning, why))))
            .into_any_element()
    }

    /// One line for a session at work in the pinned card: its provider's mark (or bot), its title, and "Waiting"
    /// while it waits on you. It opens its conversation (a subagent, the side panel's Subagents).
    fn pinned_session_row(&self, bars: &WorkBars, row: &SessionRow, cx: &Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let asking = row.state == WorkState::Asking;
        let (id, computer, subagent) = (row.id.clone(), row.computer.clone(), row.subagent);
        let selector = format!("activity-pinned-session-{id}");
        let hover = c.hover;
        let avatar = self.avatar(bars, row, "pinned", cx);
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .w_full()
            .min_w_0()
            .flex_none()
            .h(px(26.))
            .px(space::S2)
            .rounded(radius::NAV)
            .flex()
            .items_center()
            .gap(space::S2)
            .tab_index(0)
            .cursor_pointer()
            .hover(move |d| d.bg(hover))
            .focus_visible(move |d| d.bg(hover).border_1().border_color(c.accent))
            .child(avatar)
            .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text).child(row.title.clone()))
            .children(asking.then(|| status_dot(c.accent)))
            .children(asking.then(|| div().flex_none().text_ui(TypeSize::Caption).text_color(c.attention).child("Waiting")))
            .on_click(cx.listener(move |this, _, window, cx| {
                if subagent {
                    this.open_in_panel(crate::panel::Arrival::Subagents(None), window, cx);
                } else {
                    this.activity.update(cx, |bars, cx| bars.open(&id, computer.as_deref(), window, cx)).ok();
                }
            }))
            .into_any_element()
    }

    fn outputs(&self, bars: &WorkBars, cx: &Context<Self>) -> Option<AnyElement> {
        let c = cx.theme().colors.clone();
        let artifacts = self.outputs.as_ref().map(|page| page.artifacts.as_slice()).unwrap_or(&[]);
        let has_next = self.outputs.as_ref().is_some_and(|page| page.next.is_some());

        if self.outputs_failed && artifacts.is_empty() {
            let note = div()
                .px(space::S2)
                .py(space::S1)
                .text_ui(TypeSize::Caption)
                .text_color(c.text_muted)
                .child("Couldn’t read outputs. They show again once its computer answers.")
                .into_any_element();
            return Some(self.section(bars, "outputs", "Outputs", None, None, vec![note], cx));
        }

        // Never render an "Outputs" heading over an empty body.
        if artifacts.is_empty() {
            return None;
        }

        let images: Vec<&Artifact> = artifacts.iter().filter(|a| output_kind(a) == OutputKind::Images).collect();
        let files: Vec<&Artifact> = artifacts.iter().filter(|a| output_kind(a) != OutputKind::Images).collect();
        let more_images = images.len().saturating_sub(STRIP_IMAGES);
        let hidden_files = &files[RECENT_FILES.min(files.len())..];
        let hidden_docs = hidden_files.iter().filter(|a| output_kind(a) == OutputKind::Docs).count();
        let hidden_data = hidden_files.iter().filter(|a| output_kind(a) == OutputKind::Data).count();
        let hidden_code = hidden_files.iter().filter(|a| output_kind(a) == OutputKind::Code).count();
        let has_hidden_files = hidden_docs + hidden_data + hidden_code > 0;
        let can_see_all = more_images > 0 || has_hidden_files || has_next || (!images.is_empty() && !files.is_empty());

        let count = div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(artifacts.len().to_string()).into_any_element();
        let see_all = can_see_all.then(|| {
                let hover = c.hover;
                div()
                    .id("activity-see-all-outputs")
                    .debug_selector(|| "activity-see-all-outputs".into())
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .px(px(4.))
                    .py(px(1.))
                    .rounded(radius::SMALL)
                    .cursor_pointer()
                    .hover(move |d| d.bg(hover))
                    .text_ui(TypeSize::Caption)
                    .text_color(c.accent)
                    .child("All")
                    .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).color(c.accent))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.show_outputs_kind(OutputKind::All, window, cx);
                    }))
                    .into_any_element()
            });

        let visible_images: Vec<&Artifact> = images.iter().take(STRIP_IMAGES).copied().collect();
        let captions = strip_captions(&visible_images.iter().map(|a| a.name.as_str()).collect::<Vec<_>>());
        let show_more_images_tile = more_images > 0 || (has_next && images.len() >= STRIP_IMAGES);
        let media_strip = (!visible_images.is_empty()).then(|| {
            let hover = c.hover;
            let thumb_w = if show_more_images_tile { px(90.) } else { px(112.) };
            div()
                .debug_selector(|| "activity-media-strip".into())
                .flex()
                .items_start()
                .gap(space::S1_5)
                .px(space::S1)
                .pt(space::S1)
                .pb(space::S1)
                .children(visible_images.into_iter().zip(captions).map(|(artifact, caption)| {
                    let selector = format!("activity-output-{}", artifact.id);
                    let caption_selector = format!("activity-output-caption-{caption}");
                    let group: SharedString = format!("output-tile-{}", artifact.id).into();
                    let full_name = artifact.name.clone();
                    let preview = match self.thumbnails.get(&artifact.id).and_then(|(_, img)| img.as_ref()) {
                        Some(image) => gpui::img(image.clone())
                            .size_full()
                            .rounded(radius::NAV - px(1.))
                            .object_fit(gpui::ObjectFit::Cover)
                            .into_any_element(),
                        None => div()
                            .size_full()
                            .bg(c.sunken)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::Image).size(IconSize::Medium).color(c.icon))
                            .into_any_element(),
                    };
                    let clicked = artifact.clone();
                    div()
                        .id(SharedString::from(selector.clone()))
                        .debug_selector(move || selector)
                        .group(group.clone())
                        .flex_none()
                        .w(thumb_w)
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .cursor_pointer()
                        .tab_index(0)
                        .tooltip(move |_, cx| Tooltip::new(full_name.clone()).build(cx))
                        .on_click(cx.listener(move |this, _, window, cx| this.open_output(&clicked, window, cx)))
                        .child(
                            div()
                                .relative()
                                .w(thumb_w)
                                .h(px(56.))
                                .rounded(radius::NAV)
                                .overflow_hidden()
                                .border_1()
                                .border_color(c.border)
                                .child(preview)
                                .child(
                                    div()
                                        .absolute()
                                        .top(px(4.))
                                        .right(px(4.))
                                        .size(px(18.))
                                        .rounded(radius::SMALL)
                                        .bg(gpui::black().opacity(0.64))
                                        .opacity(0.)
                                        .group_hover(group, |s| s.opacity(1.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(Icon::new(IconName::ExpandEditor).size(IconSize::Indicator).color(gpui::white())),
                                ),
                        )
                        .child(
                            div()
                                .debug_selector(move || caption_selector)
                                .w(thumb_w)
                                .truncate()
                                .text_ui(TypeSize::Caption)
                                .text_color(c.text_muted)
                                .child(caption),
                        )
                }))
                .children(show_more_images_tile.then(|| {
                    let count_text = if more_images > 0 { format!("+{more_images}") } else { "More".into() };
                    let noun = if more_images == 1 { "image" } else { "images" };
                    div()
                        .id("activity-more-images")
                        .debug_selector(|| "activity-more-images".into())
                        .flex_1()
                        .min_w(px(54.))
                        .h(px(56.))
                        .rounded(radius::NAV)
                        .bg(c.sunken)
                        .border_1()
                        .border_color(c.border)
                        .cursor_pointer()
                        .hover(move |d| d.bg(hover))
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child(count_text))
                        .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(noun))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.show_outputs_kind(OutputKind::Images, window, cx);
                        }))
                }))
        });

        let mut kind_pills: Vec<AnyElement> = Vec::new();
        if hidden_docs > 0 {
            let label = format!("+{hidden_docs} {}", if hidden_docs == 1 { "doc" } else { "docs" });
            kind_pills.push(self.kind_more_pill("activity-more-docs", label, IconName::File, OutputKind::Docs, cx));
        }
        if hidden_data > 0 {
            let label = format!("+{hidden_data} data");
            kind_pills.push(self.kind_more_pill("activity-more-data", label, IconName::Chart, OutputKind::Data, cx));
        }
        if hidden_code > 0 {
            let label = format!("+{hidden_code} code");
            kind_pills.push(self.kind_more_pill("activity-more-code", label, IconName::Terminal, OutputKind::Code, cx));
        }
        if kind_pills.is_empty() && has_next && !files.is_empty() {
            kind_pills.push(self.kind_more_pill("activity-more-files", "More files…".into(), IconName::File, OutputKind::All, cx));
        }

        let folders = telling_folders(artifacts);
        let files_block = (!files.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .children(files.iter().take(RECENT_FILES).map(|artifact| {
                    self.file_row(artifact, &folders, cx)
                }))
                .when(!kind_pills.is_empty(), |d| {
                    d.child(
                        div()
                            .debug_selector(|| "activity-more-files".into())
                            .px(space::S2)
                            .pt(space::S1)
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(space::S1_5)
                            .children(kind_pills),
                    )
                })
        });

        let body = media_strip.map(IntoElement::into_any_element).into_iter()
            .chain(files_block.map(IntoElement::into_any_element))
            .collect();
        Some(self.section(bars, "outputs", "Outputs", Some(count), see_all, body, cx))
    }

    /// One of the popover's sections: its heading folds it away and back (a chevron beside it, as
    /// Codex's does), with `meta` after the chevron and `trailing` at its end; folded, only the
    /// heading stays, its counts still saying what's under it. Remembered while the conversation's
    /// open.
    #[allow(clippy::too_many_arguments)]
    fn section(
        &self,
        bars: &WorkBars,
        key: &'static str,
        title: &'static str,
        meta: Option<AnyElement>,
        trailing: Option<AnyElement>,
        body: Vec<AnyElement>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = cx.theme().colors.clone();
        let collapsed = bars.is_collapsed(key);
        let hover = c.hover;
        let fold_id = format!("activity-fold-{key}");
        let fold_selector = fold_id.clone();
        let header_selector = format!("activity-{key}-header");
        let selector = format!("activity-{key}");
        let toggle = div()
            .id(SharedString::from(fold_id))
            .debug_selector(move || fold_selector)
            .flex_1()
            .min_w_0()
            .h(px(24.))
            .flex()
            .items_center()
            .gap(space::S1)
            .px(space::S2)
            .rounded(radius::NAV)
            .tab_index(0)
            .cursor_pointer()
            .hover(move |d| d.bg(hover))
            .focus_visible(|d| d.bg(hover).border_1().border_color(c.accent))
            .tooltip(move |_, cx| Tooltip::new(if collapsed { format!("Show {title}") } else { format!("Hide {title}") }).build(cx))
            .child(div().debug_selector(move || format!("activity-{key}-title-{title}")).flex_none().text_ui(TypeSize::Small).medium().text_color(c.text).child(title))
            .child(
                Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown })
                    .size(IconSize::XSmall)
                    .color(c.text_faint),
            )
            .children(meta.map(|meta| div().ml(px(2.)).min_w_0().truncate().child(meta)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.activity.update(cx, |bars, cx| bars.fold(key, cx)).ok();
                cx.notify();
            }));
        div()
            .debug_selector(move || selector)
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(2.))
            .child(
                div()
                    .debug_selector(move || header_selector)
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(space::S1)
                    .child(toggle)
                    .children(trailing.map(|trailing| div().flex_none().child(trailing))),
            )
            .when(!collapsed, |d| d.children(body))
            .into_any_element()
    }

    /// A file it made, opening its preview: its name, and its folder beside it where another
    /// listed has the same name (two `fleet.rs`, one in `src`, one in `tests`).
    fn file_row(&self, artifact: &Artifact, folders: &HashMap<String, String>, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        let c = cx.theme().colors.clone();
        let selector = format!("activity-output-{}", artifact.id);
        let clicked = artifact.clone();
        let mut row = TextRow::truncated(selector.clone(), middle_trim(&artifact.name, 34))
            .debug_selector(selector)
            .leading(Icon::new(output_icon(artifact)).size(IconSize::Nav).color(c.icon))
            .trailing(div().text_mono(TypeSize::Caption).text_color(c.text_faint).child(ext_tag(&artifact.name)));
        if let Some(folder) = folders.get(&artifact.id) {
            row = row.secondary(folder.clone(), false, Some(c.text_faint)).secondary_size(TypeSize::Caption);
        }
        popover_row(row, cx).on_click(cx.listener(move |this, _, window, cx| this.open_output(&clicked, window, cx)))
    }

    fn kind_more_pill(
        &self,
        id: &'static str,
        label: String,
        icon: IconName,
        kind: OutputKind,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = &cx.theme().colors;
        let hover = c.hover;
        div()
            .id(id)
            .debug_selector(move || id.into())
            .px(space::S2)
            .py(px(3.))
            .rounded(radius::PILL)
            .bg(c.sunken)
            .border_1()
            .border_color(c.border)
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .flex()
            .items_center()
            .gap(px(4.))
            .text_ui(TypeSize::Caption)
            .text_color(c.text_muted)
            .child(Icon::new(icon).size(IconSize::Indicator).color(c.icon))
            .child(label)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.show_outputs_kind(kind, window, cx);
            }))
            .into_any_element()
    }

    fn all_outputs_view(&self, active_kind: OutputKind, cx: &Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let artifacts = self.outputs.as_ref().map(|page| page.artifacts.as_slice()).unwrap_or(&[]);
        let folders = telling_folders(artifacts);
        let has_next = self.outputs.as_ref().is_some_and(|page| page.next.is_some());
        let count_of = |k: OutputKind| -> usize {
            match k {
                OutputKind::All => artifacts.len(),
                other => artifacts.iter().filter(|a| output_kind(a) == other).count(),
            }
        };
        let filtered: Vec<&Artifact> = match active_kind {
            OutputKind::All => artifacts.iter().collect(),
            other => artifacts.iter().filter(|a| output_kind(a) == other).collect(),
        };

        let tabs = [OutputKind::All, OutputKind::Images, OutputKind::Docs, OutputKind::Data, OutputKind::Code]
            .into_iter()
            .filter(|&k| k == OutputKind::All || k == active_kind || count_of(k) > 0)
            .map(|kind| {
                let selected = kind == active_kind;
                let count = count_of(kind);
                let hover = c.hover;
                let sel = format!("activity-outputs-tab-{}", kind.slug());
                div()
                    .id(SharedString::from(sel.clone()))
                    .debug_selector(move || sel)
                    .px(space::S2)
                    .py(px(3.))
                    .rounded(radius::PILL)
                    .bg(if selected { c.primary } else { c.sunken })
                    .border_1()
                    .border_color(if selected { c.primary } else { c.border })
                    .cursor_pointer()
                    .when(!selected, |d| d.hover(move |s| s.bg(hover)))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .text_ui(TypeSize::Caption)
                    .text_color(if selected { c.text_on_primary } else { c.text_muted })
                    .child(kind.label())
                    .child(
                        div()
                            .text_mono(TypeSize::Caption)
                            .text_color(if selected { c.text_on_primary.opacity(0.8) } else { c.text_faint })
                            .child(count.to_string()),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.show_outputs_kind(kind, window, cx);
                    }))
                    .into_any_element()
            });

        let body: AnyElement = if active_kind == OutputKind::Images {
            let names: Vec<&str> = filtered.iter().map(|a| a.name.as_str()).collect();
            let captions = strip_captions(&names);
            let grid_items: Vec<(&Artifact, String)> = filtered.iter().copied().zip(captions).collect();
            div()
                .id("activity-images-grid")
                .debug_selector(|| "activity-images-grid".into())
                .w_full()
                .min_w_0()
                .border_t_1()
                .border_color(c.border)
                .pt(space::S2)
                .flex()
                .flex_col()
                .gap(space::S2)
                .when(grid_items.is_empty(), |d| {
                    d.child(div().px(space::S2).py(space::S3).text_ui(TypeSize::Caption).text_color(c.text_muted).child("No images yet."))
                })
                .children(grid_items.chunks(3).map(|chunk| {
                    div()
                        .w_full()
                        .flex()
                        .gap(space::S2)
                        .children(chunk.iter().map(|(artifact, caption)| {
                            let selector = format!("activity-output-{}", artifact.id);
                            let full_name = artifact.name.clone();
                            let clicked = (*artifact).clone();
                            let preview = match self.thumbnails.get(&artifact.id).and_then(|(_, img)| img.as_ref()) {
                                Some(image) => gpui::img(image.clone())
                                    .size_full()
                                    .rounded(radius::NAV - px(1.))
                                    .object_fit(gpui::ObjectFit::Cover)
                                    .into_any_element(),
                                None => div()
                                    .size_full()
                                    .bg(c.sunken)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(Icon::new(IconName::Image).size(IconSize::Medium).color(c.icon))
                                    .into_any_element(),
                            };
                            div()
                                .id(SharedString::from(selector.clone()))
                                .debug_selector(move || selector)
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .cursor_pointer()
                                .tooltip(move |_, cx| Tooltip::new(full_name.clone()).build(cx))
                                .on_click(cx.listener(move |this, _, window, cx| this.open_output(&clicked, window, cx)))
                                .child(
                                    div()
                                        .w_full()
                                        .h(px(74.))
                                        .rounded(radius::NAV)
                                        .overflow_hidden()
                                        .border_1()
                                        .border_color(c.border)
                                        .child(preview),
                                )
                                .child(
                                    div()
                                        .text_ui(TypeSize::Caption)
                                        .text_color(c.text_muted)
                                        .truncate()
                                        .child(caption.clone()),
                                )
                        }))
                        .children((chunk.len()..3).map(|_| div().flex_1().min_w_0()))
                }))
                .when(!grid_items.is_empty(), |d| {
                    d.child(
                        div()
                            .pt(space::S2)
                            .border_t_1()
                            .border_color(c.border)
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(space::S1_5)
                                    .text_ui(TypeSize::Caption)
                                    .text_color(c.text_faint)
                                    .child(Icon::new(IconName::ExpandEditor).size(IconSize::XSmall).color(c.text_faint))
                                    .child("Click any image to open full screen"),
                            )
                            .children(has_next.then(|| {
                                Button::new("activity-more-outputs", "Load more…")
                                    .ghost()
                                    .quiet()
                                    .size(ButtonSize::Small)
                                    .loading(self.more.is_some())
                                    .on_click(cx.listener(|this, _, _, cx| this.more_outputs(cx)))
                            })),
                    )
                })
                .into_any_element()
        } else {
            div()
                .id("activity-all-outputs-list")
                .w_full()
                .min_w_0()
                .border_t_1()
                .border_color(c.border)
                .pt(space::S1_5)
                .flex()
                .flex_col()
                .when(filtered.is_empty(), |d| {
                    d.child(div().px(space::S2).py(space::S3).text_ui(TypeSize::Caption).text_color(c.text_muted).child("No files of this kind."))
                })
                .children(filtered.into_iter().map(|artifact| {
                    self.file_row(artifact, &folders, cx)
                }))
                .children(has_next.then(|| {
                    div().pt(space::S1).child(
                        Button::new("activity-more-outputs", "Load more…")
                            .ghost()
                            .quiet()
                            .size(ButtonSize::Small)
                            .loading(self.more.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.more_outputs(cx))),
                    )
                }))
                .into_any_element()
        };

        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(space::S2)
            .debug_selector(|| "activity-all-outputs".into())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .px(space::S1)
                    .child(
                        Button::icon_only("activity-outputs-back", IconName::ChevronLeft, "Back to Activity")
                            .ghost()
                            .size(ButtonSize::Small)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.drilldown = None;
                                window.focus(&this.focus, cx);
                                cx.notify();
                            })),
                    )
                    .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child("Outputs"))
                    .child(
                        div()
                            .text_ui(TypeSize::Caption)
                            .text_color(c.text_faint)
                            .child(active_kind.count_label(count_of(active_kind))),
                    ),
            )
            .child(div().px(space::S1).flex().flex_wrap().items_center().gap(space::S1).children(tabs))
            .child(body)
            .into_any_element()
    }

    fn finished_sessions_view(&self, bars: &WorkBars, cx: &Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let all_recent = &bars.recent_rows;
        let query = self.finished_search.read(cx).text().trim().to_ascii_lowercase();
        let failed_count = all_recent.iter().filter(|s| s.state == WorkState::Failed).count();
        let stopped_count = all_recent.iter().filter(|s| s.state == WorkState::Stopped).count();

        let filtered: Vec<&SessionRow> = all_recent
            .iter()
            .filter(|s| match self.finished_filter {
                FinishedFilter::All => true,
                FinishedFilter::Failed => s.state == WorkState::Failed,
                FinishedFilter::Stopped => s.state == WorkState::Stopped,
            })
            .filter(|s| query.is_empty() || s.title.to_ascii_lowercase().contains(&query))
            .collect();
        let shown_count = filtered.len().min(self.finished_limit);
        let has_more = filtered.len() > shown_count;

        let filter_pill = |id: &'static str, label: &'static str, count: usize, filter: FinishedFilter| {
            let selected = self.finished_filter == filter;
            let hover = c.hover;
            div()
                .id(id)
                .debug_selector(move || id.into())
                .px(space::S2)
                .py(px(3.))
                .rounded(radius::PILL)
                .bg(if selected { c.primary } else { c.sunken })
                .border_1()
                .border_color(if selected { c.primary } else { c.border })
                .cursor_pointer()
                .when(!selected, |d| d.hover(move |s| s.bg(hover)))
                .flex()
                .items_center()
                .gap(px(4.))
                .text_ui(TypeSize::Caption)
                .text_color(if selected { c.text_on_primary } else { c.text_muted })
                .child(label)
                .child(
                    div()
                        .text_mono(TypeSize::Caption)
                        .text_color(if selected { c.text_on_primary.opacity(0.8) } else { c.text_faint })
                        .child(count.to_string()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.finished_filter = filter;
                    this.finished_limit = FINISHED_PAGE_SIZE;
                    cx.notify();
                }))
        };

        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(space::S2)
            .debug_selector(|| "activity-finished-view".into())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .px(space::S1)
                    .child(
                        Button::icon_only("activity-finished-back", IconName::ChevronLeft, "Back to Activity")
                            .ghost()
                            .size(ButtonSize::Small)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.drilldown = None;
                                window.focus(&this.focus, cx);
                                cx.notify();
                            })),
                    )
                    .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child("Finished sessions"))
                    .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(all_recent.len().to_string())),
            )
            .child(
                div()
                    .debug_selector(|| "activity-finished-search".into())
                    .h(px(30.))
                    .px(space::S2)
                    .rounded(radius::NAV)
                    .bg(c.sunken)
                    .border_1()
                    .border_color(c.border)
                    .flex()
                    .items_center()
                    .gap(space::S1_5)
                    .child(Icon::new(IconName::Search).size(IconSize::XSmall).color(c.text_faint))
                    .child(div().flex_1().min_w_0().child(self.finished_search.clone())),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S1)
                    .child(filter_pill("activity-finished-filter-all", "All", all_recent.len(), FinishedFilter::All))
                    .child(filter_pill("activity-finished-filter-failed", "Failed", failed_count, FinishedFilter::Failed))
                    .child(filter_pill("activity-finished-filter-stopped", "Stopped", stopped_count, FinishedFilter::Stopped)),
            )
            .child(
                div()
                    .id("activity-finished-list")
                    .w_full()
                    .min_w_0()
                    .border_t_1()
                    .border_color(c.border)
                    .pt(space::S1)
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .when(filtered.is_empty(), |d| {
                        d.child(div().px(space::S2).py(space::S3).text_ui(TypeSize::Caption).text_color(c.text_muted).child("No matching sessions."))
                    })
                    .children(filtered.iter().take(shown_count).map(|session| self.session_row(bars, session, false, cx)))
                    .when(has_more, |d| {
                        d.child(
                            div()
                                .pt(space::S2)
                                .px(space::S2)
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_ui(TypeSize::Caption)
                                .text_color(c.text_faint)
                                .child(format!("Showing {shown_count} of {}", filtered.len()))
                                .child(
                                    Button::new("activity-finished-more", "Show older…")
                                        .ghost()
                                        .quiet()
                                        .size(ButtonSize::Small)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.finished_limit += FINISHED_PAGE_SIZE;
                                            cx.notify();
                                        })),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }

    fn avatar(&self, bars: &WorkBars, session: &SessionRow, location: &str, cx: &App) -> AnyElement {
        let c = &cx.theme().colors;
        let provider = session.provider.as_deref().or_else(|| self.providers.get(&session.id).map(String::as_str));
        let bot = bars.bot_look.as_ref().and_then(|lookup| lookup(&session.id, cx));
        let size = px(20.);
        let inner = size - px(4.);
        let (label, face) = if let Some((id, look)) = bot {
            (format!("bot-{id}"), crate::bot_face::still_avatar(&id, &look, kuvo_ui::bot::State::Idle, inner))
        } else if let Some((service, path, color)) = provider.and_then(session_mark) {
            (service.to_owned(), Mark::new(path).size(px(14.)).color(color.unwrap_or(c.text)).into_any_element())
        } else {
            ("unknown".into(), Icon::new(IconName::Bot).size(IconSize::Nav).color(c.icon).into_any_element())
        };
        let selector = format!("activity-{location}-avatar-{}-{label}", session.id);
        // (The mark alone, as the usage rings draw it: no disc behind it.)
        div().debug_selector(move || selector)
            .flex().flex_none().items_center().justify_center().size(size)
            .child(face).into_any_element()
    }

    /// A session's row, opening its conversation (on its computer).
    fn session_row(&self, bars: &WorkBars, session: &SessionRow, active: bool, cx: &Context<Self>) -> AnyElement {
        let c = &cx.theme().colors;
        let avatar = self.avatar(bars, session, "row", cx);
        let id = session.id.clone();
        let stop = id.clone();
        let computer = session.computer.clone();
        let local = computer.is_none();
        let (status, status_color) = match session.state {
            WorkState::Working => ("Working", c.text_faint),
            WorkState::Asking => ("Waiting for you", c.attention),
            WorkState::Finished => ("Finished", c.text_faint),
            WorkState::Failed => ("Failed", c.danger),
            WorkState::Stopped => ("Stopped", c.warning),
            _ => ("", c.text_faint),
        };
        let mut row = TextRow::truncated(format!("activity-session-{id}"), session.title.clone())
            .debug_selector(format!("activity-session-{id}"))
            .leading(avatar);
        if session.state == WorkState::Asking {
            row = row.trailing(status_dot(c.accent));
        }
        row = row.trailing(div().text_ui(TypeSize::Caption).text_color(status_color).child(status));
        div().w_full().min_w_0().flex().items_center().gap(space::S1)
            .child(
                popover_row(row, cx)
                .flex_1()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.dismiss(cx);
                    this.activity.update(cx, |bars, cx| bars.open(&id, computer.as_deref(), window, cx)).ok();
                })),
            )
            .children((active && local).then(|| Button::icon_only(SharedString::from(format!("activity-stop-{stop}")), IconName::Stop, "Stop session").ghost().quiet()
                .size(ButtonSize::Small).disabled(bars.stopping_all || bars.stopping.contains(&stop))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.activity.update(cx, |bars, cx| bars.stop_session(&stop, cx)).ok();
                }))))
            .into_any_element()
    }

    /// Shows `arrival` in the conversation's side panel.
    fn open_in_panel(&self, arrival: crate::panel::Arrival, window: &mut Window, cx: &mut App) {
        let opener = self.activity.upgrade().and_then(|bars| bars.read(cx).panel.clone());
        if let Some(opener) = opener {
            opener(arrival, window, cx);
        }
    }

    /// The sessions at work for it (those it started, a bot's), each opening its conversation, and
    /// a bot's finished ones behind a row of their own. Its subagents are counted apart.
    fn sessions(&self, bars: &WorkBars, cx: &Context<Self>) -> Option<AnyElement> {
        let sessions = &bars.active_rows;
        let recent = &bars.recent_rows;
        if sessions.is_empty() && recent.is_empty() { return None; }
        let c = cx.theme().colors.clone();
        let asking = sessions.iter().filter(|row| row.state == WorkState::Asking).count();
        let working = sessions.len() - asking;
        let can_stop = bars.bot.is_some() && bars.work_fresh && !sessions.is_empty();

        let meta = (!sessions.is_empty()).then(|| {
            div().text_ui(TypeSize::Caption).text_color(if asking > 0 { c.attention } else { c.text_faint })
                .child(sessions_label(working, asking)).into_any_element()
        });
        let stop_all = can_stop.then(|| {
            Button::new("activity-stop-all", "Stop All")
                .ghost()
                .quiet()
                .size(ButtonSize::Small)
                .loading(bars.stopping_all)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.activity.update(cx, |bars, cx| bars.stop_all(cx)).ok();
                }))
                .into_any_element()
        });

        let visible_active = if self.all_active { sessions.len() } else { sessions.len().min(INLINE_ACTIVE_SESSIONS) };
        let hidden_active = sessions.len().saturating_sub(visible_active);

        let finished_disclosure = (!recent.is_empty()).then(|| {
            let count = recent.len();
            let failed = recent.iter().filter(|s| s.state == WorkState::Failed).count();
            let label = format!("{count} finished {}", if count == 1 { "session" } else { "sessions" });
            let hover = c.hover;
            div()
                .id("activity-finished-sessions")
                .debug_selector(|| "activity-finished-sessions".into())
                .h(px(28.))
                .px(space::S2)
                .rounded(radius::NAV)
                .tab_index(0)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .focus_visible(|d| d.bg(hover).border_1().border_color(c.accent))
                .flex()
                .items_center()
                .justify_between()
                .gap(space::S2)
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .child(
                            div()
                                .debug_selector(|| "activity-finished-icon-check".into())
                                .flex()
                                .flex_none()
                                .items_center()
                                .justify_center()
                                .size(px(20.))
                                .child(Icon::new(IconName::Check).size(IconSize::XSmall).color(c.text_faint)),
                        )
                        .child(div().truncate().text_ui(TypeSize::Small).text_color(c.text_muted).child(label)),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .when(failed > 0, |d| {
                            d.child(div().text_ui(TypeSize::Caption).text_color(c.danger).child(format!("{failed} failed")))
                        })
                        .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).color(c.text_faint)),
                )
                .on_click(cx.listener(move |this, _, window, cx| this.show_finished_sessions(window, cx)))
                .into_any_element()
        });

        let mut body: Vec<AnyElement> = sessions.iter().take(visible_active).map(|session| self.session_row(bars, session, true, cx)).collect();
        if hidden_active > 0 || (self.all_active && sessions.len() > INLINE_ACTIVE_SESSIONS) {
            let toggle_label = if self.all_active {
                "Show fewer".to_owned()
            } else {
                format!("Show all {}", sessions.len())
            };
            let hover = c.hover;
            body.push(
                div()
                    .id("activity-toggle-sessions")
                    .debug_selector(|| "activity-toggle-sessions".into())
                    .h(px(26.))
                    .px(space::S2)
                    .rounded(radius::NAV)
                    .tab_index(0)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_ui(TypeSize::Caption)
                    .text_color(c.accent)
                    .child(toggle_label)
                    .child(Icon::new(if self.all_active { IconName::ChevronUp } else { IconName::ChevronDown }).size(IconSize::XSmall).color(c.accent))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.all_active = !this.all_active;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        body.extend(finished_disclosure);
        Some(self.section(bars, "sessions", "Sessions", meta, stop_all, body, cx))
    }

    fn processes(&self, bars: &WorkBars, cx: &Context<Self>) -> Option<AnyElement> {
        if bars.background.commands.is_empty() { return None; }
        let c = cx.theme().colors.clone();
        let now = bars.now.unwrap_or_else(crate::time::now_ms);
        let running_count = bars.background.commands.len();
        let meta = div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(format!("{running_count} running")).into_any_element();
        let rows = bars.background.commands.iter().map(|command| {
                    let id = command.id.clone();
                    let uptime = format_uptime(command.started, now);
                    let mut row = TextRow::truncated(
                        format!("activity-process-{id}"),
                        command.command.lines().next().unwrap_or("").to_owned(),
                    )
                    .debug_selector(format!("activity-process-{id}"))
                    .mono(true)
                    .leading(Icon::new(IconName::TerminalWindow).size(IconSize::Nav).color(c.icon));
                    if let Some(uptime) = uptime {
                        row = row.trailing(div().text_mono(TypeSize::Caption).text_color(c.text_faint).child(uptime));
                    }
                    row = row.trailing(status_dot(c.success));
                    // Its output, as it prints, in its own tab in the side panel.
                    let (session, opened) = (bars.session.clone(), command.clone());
                    popover_row(row, cx).on_click(cx.listener(move |this, _, window, cx| {
                        this.dismiss(cx);
                        let arrival = crate::panel::Arrival::Process { session: session.clone(), command: opened.clone() };
                        this.open_in_panel(arrival, window, cx);
                    })).into_any_element()
                }).collect();
        // (What its agent runs now, in the foreground or left running: the box lists both.)
        Some(self.section(bars, "processes", "Processes", Some(meta), None, rows, cx))
    }

    fn preview(&self, name: &str, preview: &Preview, cx: &Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        div().w_full().min_w_0().flex().flex_col().debug_selector(|| "activity-output-preview".into())
            .child(div().w_full().min_w_0().flex().items_center().gap(space::S2).px(space::S2)
                .child(Button::icon_only("activity-preview-back", IconName::ChevronLeft, "Back to Activity").ghost().size(ButtonSize::Small)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.preview = None;
                        this.reading = None;
                        window.focus(&this.focus, cx);
                        cx.notify();
                    })))
                .child(div().flex_1().min_w_0().truncate().child(name.to_owned())))
            // (The popover scrolls as one: the preview has no scroll of its own.)
            .child(div().id("activity-preview-body").border_t_1().border_color(c.border).p(space::S2)
                .child(match preview {
                    Preview::Text(text) => crate::panel::files::text_preview(name, text, text.lines().count(), cx),
                    Preview::CurrentFile(text) => div().flex().flex_col().gap(space::S2)
                        .child(div().debug_selector(|| "activity-current-file".into()).text_color(c.text_muted).child("Current file"))
                        .child(crate::panel::files::text_preview(name, text, text.lines().count(), cx)).into_any_element(),
                    Preview::Loading => div().text_color(c.text_muted).child("Reading…").into_any_element(),
                    Preview::Message(why) => div().text_color(c.text_muted).child(why.clone()).into_any_element(),
                }))
            .into_any_element()
    }
}

impl ActivityCard {
    /// Everything the popover lists (or the row opened in it): the popover's body, and the pinned
    /// card's in Full.
    fn full_content(&self, bars: &WorkBars, cx: &Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        if let Some((name, preview)) = &self.preview {
            self.preview(name, preview, cx)
        } else if let Some(drilldown) = self.drilldown {
            match drilldown {
                Drilldown::Outputs(kind) => self.all_outputs_view(kind, cx),
                Drilldown::FinishedSessions => self.finished_sessions_view(bars, cx),
            }
        } else if !self.loaded(bars, cx) {
            // Nothing drawn until every section has what its box said, so none jumps as it arrives.
            div()
                .debug_selector(|| "activity-loading".into())
                .h(px(72.))
                .flex()
                .items_center()
                .justify_center()
                .child(kuvo_ui::small::LoadingLabel::new("activity-loading-label", "Loading…"))
                .into_any_element()
        } else {
            let wake_label = bars.wake_label();
            let show_wake = wake_label.is_some() || self.force_wakes;
            let wake = show_wake.then(|| self.wake_schedule.clone()).flatten().map(|schedule| {
                let counts = schedule.read(cx).summary_counts();
                let header_right = wake_label
                    .as_ref()
                    .filter(|l| l.starts_with("Waking") || l.contains("paused") || l.contains("due"))
                    .cloned()
                    .or(counts)
                    .or(wake_label.clone());
                let meta = header_right.map(|right| {
                    let is_status = wake_label.is_some();
                    div()
                        .when(is_status, |d| d.debug_selector(|| "activity-wake-status".into()))
                        .text_ui(TypeSize::Caption)
                        .text_color(c.text_faint)
                        .child(right)
                        .into_any_element()
                });
                self.section(bars, "wakes", "Wake-ups & Watches", meta, None, vec![schedule.into_any_element()], cx)
            });

            // As Codex orders its: what's scheduled, what it made, who works for it, then the rest.
            let sections: Vec<AnyElement> = [
                wake,
                self.outputs(bars, cx),
                self.sessions(bars, cx),
                self.subagents(bars, cx),
                self.side_chats(bars, cx),
                self.processes(bars, cx),
            ]
            .into_iter()
            .flatten()
            .collect();

            // One calm note when there's nothing at all, never headings over nothing.
            let empty_note = sections.is_empty().then(|| {
                div()
                    .debug_selector(|| "activity-empty".into())
                    .px(space::S2)
                    .py(space::S3)
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(space::S1)
                    .child(Icon::new(IconName::Activity).size(IconSize::Large).color(c.text_faint))
                    .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child("Nothing here yet"))
                    .child(
                        div()
                            .max_w(px(280.))
                            .text_center()
                            .text_ui(TypeSize::Caption)
                            .text_color(c.text_muted)
                            .child("Files it makes, the sessions and subagents it starts, side chats and wake-ups show here."),
                    )
            });

            let mut list = div().flex().flex_col()
                .children(((!bars.background_fresh && (!bars.background.commands.is_empty() || !bars.background.subagents.is_empty()))
                    || (!bars.work_fresh && bars.work.is_some())).then(|| {
                    div().debug_selector(|| "activity-last-known".into()).px(space::S2).pb(space::S1_5)
                        .text_ui(TypeSize::Caption).text_color(c.text_muted).child("Last known activity")
                }))
                .children(empty_note);

            for (index, section) in sections.into_iter().enumerate() {
                if index > 0 {
                    list = list.child(div().my(space::S1_5).h(px(1.)).w_full().bg(c.border));
                }
                list = list.child(section);
            }

            list.children(bars.failure.clone().map(|why| div().px(space::S2).child(kuvo_ui::Notice::said("work-bars-failure", kuvo_ui::Tone::Warning, why))))
                .into_any_element()
        }
    }
}

impl Render for ActivityCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(activity) = self.activity.upgrade() else { return div().into_any_element() };
        let bars = activity.read(cx);
        let theme = cx.theme().clone();
        let c = &theme.colors;
        if self.pinned {
            let max_h = (window.viewport_size().height - px(140.)).max(px(120.));
            // (Full, or a row opened in it, shows inside the card at its width: never the popover.)
            let full = bars.full || self.preview.is_some() || self.drilldown.is_some();
            let body = if full {
                div().flex().flex_col().child(self.pinned_top(bars, cx)).child(div().my(space::S1_5).h(px(1.)).w_full().bg(c.border))
                    .child(self.full_content(bars, cx)).into_any_element()
            } else {
                self.pinned_view(bars, cx)
            };
            return div()
                .id("activity-pinned-card")
                .debug_selector(|| "activity-pinned-card".into())
                .track_focus(&self.focus)
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .tab_group()
                .occlude()
                .w(px(PINNED_WIDTH))
                .max_h(max_h)
                .overflow_y_scroll()
                .p(space::S3)
                .rounded(radius::POPOVER)
                .bg(c.elevated)
                .border_1()
                .border_color(c.border)
                .shadow(theme.elevated_shadow())
                .text_ui(TypeSize::Small)
                .text_color(c.text)
                .child(body)
                .into_any_element();
        }
        // Keep the popover compact and strictly below its header trigger button so `anchor::place`
        // never clamps upward over the button itself.
        let cap = if self.preview.is_some() { px(560.) } else { px(480.) };
        let max_h = cap.min((window.viewport_size().height - px(120.)).max(px(160.)));
        let top = (self.preview.is_none() && self.drilldown.is_none()).then(|| {
            // (Its context, with Pin, heads it where the box says how full it is; else "Activity".)
            let top = self.context_strip(bars, cx);
            let ruled = top.is_some();
            div().flex().flex_col().child(top.unwrap_or_else(|| self.heading(cx)))
                .when(ruled, |d| d.child(div().my(space::S1_5).h(px(1.)).w_full().bg(c.border)))
        });
        let content = self.full_content(bars, cx);
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
            // (A click in the summarize point's menu, drawn over its edge, isn't outside it.)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| if !this.summarize_menu.is_open() { this.dismiss(cx) }))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .tab_group()
            .occlude()
            .w(px(if self.preview.is_some() { 460. } else { 396. }).min(window.viewport_size().width - px(16.)))
            .max_h(max_h)
            .overflow_y_scroll()
            .p(space::S3)
            .rounded(radius::POPOVER)
            .bg(c.elevated)
            .border_1()
            .border_color(c.border)
            .shadow(theme.elevated_shadow())
            .text_ui(TypeSize::Small)
            .text_color(c.text)
            .children(top)
            .child(content)
            .into_any_element()
    }
}

/// The summarize point's choices, the one in use checked, in its box's words; picking one sends
/// it to the box.
fn summarize_menu(summarize: Summarize, activity: WeakEntity<WorkBars>, cx: &mut App) -> Entity<kuvo_ui::menu::Menu> {
    cx.new(|cx| {
        let mut menu = kuvo_ui::menu::Menu::new(cx).width(px(150.));
        for choice in &summarize.choices {
            let (id, activity) = (choice.id.clone(), activity.clone());
            let mut entry = kuvo_ui::menu::MenuEntry::new(choice.title.clone()).checked(choice.id == summarize.value)
                .on_select(move |_, cx| { activity.update(cx, |bars, cx| bars.pick_summarize(id.clone(), cx)).ok(); });
            if let Some(line) = &choice.line {
                entry = entry.detail(line.clone());
            }
            menu = menu.entry(entry);
        }
        if let Some(line) = &summarize.line {
            menu = menu.separator().header(line.clone());
        }
        menu
    })
}

/// "412k of 1M", or "412k used" where its box doesn't know the model's window.
fn context_words(context: &ContextSize) -> String {
    match context.window {
        Some(window) => format!("{} of {}", short_tokens(context.used), short_tokens(window)),
        None => format!("{} used", short_tokens(context.used)),
    }
}

/// Near (or past) where it's summarized by itself.
fn context_near(context: &ContextSize) -> bool {
    context.summarize_at.is_some_and(|at| context.used as f64 >= at as f64 * NEAR_SUMMARY)
}

/// A slim bar of used against the window, with a tick at the summarize point; none without a window.
fn context_bar(context: &ContextSize, cx: &App) -> Option<AnyElement> {
    let window = context.window.filter(|window| *window > 0)? as f32;
    let c = &cx.theme().colors;
    let near = context_near(context);
    // (From 0: nothing used yet draws no sliver.)
    let fill = if context.used == 0 { 0. } else { (context.used as f32 / window).clamp(0.015, 1.) };
    let tick = context.summarize_at.map(|at| at as f32 / window).filter(|tick| *tick > 0. && *tick < 1.);
    Some(div()
        .debug_selector(|| "activity-context-bar".into())
        .relative()
        .w_full()
        .h(px(3.))
        .rounded(radius::PILL)
        .bg(c.text.opacity(0.1))
        .child(div().h_full().w(relative(fill)).rounded(radius::PILL).bg(if near { c.warning } else { c.text_muted }))
        .children(tick.map(|tick| div()
            .debug_selector(|| "activity-context-tick".into())
            .absolute()
            .top(px(-3.))
            .left(relative(tick))
            .w(px(1.))
            .h(px(9.))
            .rounded(px(1.))
            .bg(if near { c.warning } else { c.border_strong })))
        .into_any_element())
}

/// A one-line row of the pinned card: its icon, its name, and what's under it at its end.
fn summary_row(id: &'static str, icon: IconName, label: &'static str, trailing: AnyElement, cx: &App) -> gpui::Stateful<gpui::Div> {
    let c = &cx.theme().colors;
    let hover = c.hover;
    let accent = c.accent;
    div()
        .id(id)
        .debug_selector(move || id.into())
        .w_full()
        .min_w_0()
        .flex_none()
        .h(px(28.))
        .px(space::S2)
        .rounded(radius::NAV)
        .flex()
        .items_center()
        .gap(space::S2)
        .tab_index(0)
        .cursor_pointer()
        .hover(move |d| d.bg(hover))
        .focus_visible(move |d| d.bg(hover).border_1().border_color(accent))
        .child(div().flex_none().size(px(16.)).flex().items_center().justify_center().child(Icon::new(icon).size(IconSize::Nav).color(c.icon)))
        .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text).child(label))
        .child(div().flex_none().child(trailing))
}

fn popover_row(row: TextRow, cx: &App) -> gpui::Stateful<gpui::Div> {
    let c = &cx.theme().colors;
    row.into_div(cx)
        .flex_none()
        .h(px(28.))
        .px(space::S2)
        .rounded(radius::NAV)
        .tab_index(0)
        .cursor_pointer()
        .hover(|d| d.bg(c.hover))
        .focus_visible(|d| d.bg(c.hover).border_1().border_color(c.accent))
}

fn format_uptime(started_ms: f64, now_ms: f64) -> Option<String> {
    if started_ms <= 0. || now_ms < started_ms {
        return None;
    }
    let secs = ((now_ms - started_ms) / 1_000.).round() as u64;
    if secs < 60 {
        Some(format!("{secs}s"))
    } else if secs < 3_600 {
        Some(format!("{}m", secs / 60))
    } else {
        Some(format!("{}h", secs / 3_600))
    }
}

/// Classifies an output artifact into a user-facing kind for kind-filtered "+N more" drill-downs.
pub fn output_kind(artifact: &Artifact) -> OutputKind {
    if artifact.kind == "image" || crate::image_viewer::is_image_path(&artifact.name) {
        return OutputKind::Images;
    }
    let ext = artifact
        .name
        .rsplit('.')
        .next()
        .filter(|e| *e != artifact.name)
        .map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("csv" | "tsv" | "xlsx" | "xls" | "parquet" | "sql" | "sqlite" | "db" | "ndjson") => {
            OutputKind::Data
        }
        Some(
            "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "py" | "go" | "java" | "kt"
            | "swift" | "c" | "h" | "cpp" | "hpp" | "cs" | "rb" | "php" | "sh" | "zsh" | "bash"
            | "json" | "toml" | "yaml" | "yml" | "xml" | "css" | "scss" | "less",
        ) => OutputKind::Code,
        Some("md" | "markdown" | "txt" | "pdf" | "doc" | "docx" | "rtf" | "html" | "htm" | "org" | "rst") => {
            OutputKind::Docs
        }
        _ => match artifact.kind.as_str() {
            "code" => OutputKind::Code,
            "download" => OutputKind::Data,
            _ => OutputKind::Docs,
        },
    }
}

fn output_icon(artifact: &Artifact) -> IconName {
    match output_kind(artifact) {
        OutputKind::Images => IconName::Image,
        OutputKind::Data => IconName::Chart,
        OutputKind::Code => IconName::Terminal,
        OutputKind::Docs | OutputKind::All => {
            if artifact.kind == "page" {
                IconName::Globe
            } else {
                IconName::File
            }
        }
    }
}

/// For each listed file whose name another listed file has too, the end of its folder that tells
/// them apart (`src`, `tests`; `kuvo/src` where the last folders match too).
fn telling_folders(artifacts: &[Artifact]) -> HashMap<String, String> {
    let mut by_name: HashMap<&str, Vec<&Artifact>> = HashMap::new();
    for artifact in artifacts {
        by_name.entry(artifact.name.as_str()).or_default().push(artifact);
    }
    let mut told = HashMap::new();
    for same in by_name.into_values().filter(|same| same.len() > 1) {
        let folders: Vec<Vec<&str>> = same.iter()
            .map(|artifact| artifact.path.rsplit('/').skip(1).filter(|part| !part.is_empty()).collect())
            .collect();
        let deepest = folders.iter().map(Vec::len).max().unwrap_or(0);
        // The fewest folders from the end that make each one's differ (all of them, at most).
        let depth = (1..=deepest.max(1)).find(|depth| {
            let ends: HashSet<Vec<&str>> = folders.iter().map(|parts| parts.iter().take(*depth).copied().collect()).collect();
            ends.len() == folders.len()
        }).unwrap_or(deepest.max(1));
        for (artifact, parts) in same.iter().zip(&folders) {
            let mut end: Vec<&str> = parts.iter().take(depth).copied().collect();
            end.reverse();
            if !end.is_empty() {
                told.insert(artifact.id.clone(), end.join("/"));
            }
        }
    }
    told
}

fn ext_tag(name: &str) -> String {
    name.rsplit('.')
        .next()
        .filter(|ext| ext.len() <= 4 && *ext != name)
        .map(|ext| ext.to_ascii_uppercase())
        .unwrap_or_else(|| "FILE".into())
}

/// Middle-truncates long filenames so distinguishing suffixes stay visible.
fn middle_trim(name: &str, max_chars: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max_chars || max_chars < 10 {
        return name.to_owned();
    }
    let tail = (max_chars / 3).clamp(4, 11);
    let head = max_chars.saturating_sub(tail + 1);
    let start: String = chars[..head].iter().collect();
    let end: String = chars[chars.len() - tail..].iter().collect();
    format!("{start}…{end}")
}

const CAPTION_LIMIT: usize = 15;

fn is_caption_sep(ch: char) -> bool {
    matches!(ch, '-' | '_' | ' ' | '.' | '/')
}

fn strip_extension(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, ext))
            if !stem.is_empty()
                && !ext.is_empty()
                && ext.len() <= 5
                && ext.chars().all(|c| c.is_ascii_alphanumeric()) =>
        {
            stem
        }
        _ => name,
    }
}

fn shared_prefix_bytes(stems: &[&str]) -> usize {
    if stems.len() < 2 {
        return 0;
    }
    let first = stems[0];
    let mut raw_bytes = 0usize;
    let mut raw_chars = 0usize;
    let mut sep_bytes = 0usize;
    let mut iters: Vec<_> = stems[1..].iter().map(|s| s.chars()).collect();
    for ch in first.chars() {
        if iters.iter_mut().all(|it| it.next() == Some(ch)) {
            raw_bytes += ch.len_utf8();
            raw_chars += 1;
            if is_caption_sep(ch) {
                sep_bytes = raw_bytes;
            }
        } else {
            break;
        }
    }
    if sep_bytes > 0 && stems.iter().all(|s| s.len() > sep_bytes) {
        return sep_bytes;
    }
    if raw_chars >= 6 && stems.iter().all(|s| s.len() > raw_bytes) {
        // Keep at least a couple of characters of context if stripping leaves 1-char tails.
        let min_tail = stems.iter().map(|s| s[raw_bytes..].chars().count()).min().unwrap_or(0);
        if min_tail >= 2 {
            return raw_bytes;
        }
        let back = 2 - min_tail;
        let keep_chars = raw_chars.saturating_sub(back);
        if keep_chars >= 4 {
            let cut = first.char_indices().nth(keep_chars).map_or(0, |(idx, _)| idx);
            if stems.iter().all(|s| s.len() > cut) {
                return cut;
            }
        }
    }
    0
}

fn shared_suffix_bytes(stems: &[&str]) -> usize {
    if stems.len() < 2 {
        return 0;
    }
    let first = stems[0];
    let mut raw_bytes = 0usize;
    let mut sep_bytes = 0usize;
    let mut iters: Vec<_> = stems[1..].iter().map(|s| s.chars().rev()).collect();
    for ch in first.chars().rev() {
        if iters.iter_mut().all(|it| it.next() == Some(ch)) {
            raw_bytes += ch.len_utf8();
            if is_caption_sep(ch) {
                sep_bytes = raw_bytes;
            }
        } else {
            break;
        }
    }
    if sep_bytes > 0 && stems.iter().all(|s| s.len() > sep_bytes) {
        sep_bytes
    } else {
        0
    }
}

fn take_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn join_pair(first: &str, last: &str, limit: usize) -> String {
    let first_len = first.chars().count();
    let last_len = last.chars().count();
    if first_len + 1 + last_len <= limit {
        return format!("{first}-{last}");
    }
    let tail_budget = last_len.min((limit / 2).max(4));
    let head_budget = limit.saturating_sub(tail_budget + 1);
    if head_budget >= 3 {
        let head = take_chars(first, head_budget);
        let tail = if last_len <= tail_budget {
            last.to_owned()
        } else {
            take_chars(last, tail_budget)
        };
        format!("{head}-{tail}")
    } else {
        take_chars(first, limit.saturating_sub(1)) + "…"
    }
}

fn compact_stem(stem: &str, limit: usize) -> String {
    if stem.chars().count() <= limit {
        return stem.to_owned();
    }
    let tokens: Vec<&str> = stem.split(|c| matches!(c, '-' | '_' | ' ')).filter(|t| !t.is_empty()).collect();
    match tokens.as_slice() {
        [first, .., last] if tokens.len() >= 2 => join_pair(first, last, limit),
        _ => middle_trim(stem, limit),
    }
}

/// Builds readable, distinct captions for the visible thumbnail strip: drops extensions and any
/// prefix (or suffix, when needed) shared across visible names, then keeps distinguishing tokens
/// within the tile's width. Full filenames remain on hover.
fn strip_captions(names: &[&str]) -> Vec<String> {
    if names.is_empty() {
        return Vec::new();
    }
    let raw_stems: Vec<&str> = names.iter().map(|name| strip_extension(name)).collect();
    let prefix = shared_prefix_bytes(&raw_stems);
    let mut trimmed: Vec<&str> = raw_stems
        .iter()
        .map(|s| s[prefix..].trim_start_matches(is_caption_sep))
        .collect();
    if trimmed.iter().any(|s| s.is_empty()) {
        trimmed = raw_stems.clone();
    }
    if trimmed.len() >= 2 && trimmed.iter().any(|s| s.chars().count() > CAPTION_LIMIT) {
        let suffix = shared_suffix_bytes(&trimmed);
        if suffix > 0 {
            let shortened: Vec<&str> = trimmed
                .iter()
                .map(|s| s[..s.len() - suffix].trim_end_matches(is_caption_sep))
                .collect();
            if shortened.iter().all(|s| !s.is_empty()) {
                trimmed = shortened;
            }
        }
    }

    let mut captions: Vec<String> = trimmed.iter().map(|s| compact_stem(s, CAPTION_LIMIT)).collect();

    // If shortening made two visible tiles collide, switch each colliding tile to its first
    // differing token (paired with its trailing token when it fits), or fall back to extensions.
    let mut seen: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, cap) in captions.iter().enumerate() {
        seen.entry(cap.clone()).or_default().push(idx);
    }
    for group in seen.into_values().filter(|g| g.len() > 1) {
        let tokenized: Vec<Vec<&str>> = group
            .iter()
            .map(|&i| trimmed[i].split(|c| matches!(c, '-' | '_' | ' ')).filter(|t| !t.is_empty()).collect())
            .collect();
        let max_tokens = tokenized.iter().map(Vec::len).max().unwrap_or(0);
        let diff_at = (0..max_tokens).find(|&pos| {
            let first = tokenized[0].get(pos).copied();
            tokenized[1..].iter().any(|toks| toks.get(pos).copied() != first)
        });
        let mut resolved = Vec::with_capacity(group.len());
        if let Some(pos) = diff_at {
            for toks in &tokenized {
                let candidate = match (toks.get(pos).copied(), toks.last().copied()) {
                    (Some(diff), Some(last)) if pos + 1 < toks.len() && diff != last => {
                        join_pair(diff, last, CAPTION_LIMIT)
                    }
                    (Some(diff), _) => {
                        if diff.chars().count() <= CAPTION_LIMIT {
                            diff.to_owned()
                        } else {
                            middle_trim(diff, CAPTION_LIMIT)
                        }
                    }
                    _ => String::new(),
                };
                resolved.push(candidate);
            }
        }
        let unique = resolved.len() == group.len()
            && resolved.iter().all(|s| !s.is_empty())
            && resolved.iter().collect::<HashSet<_>>().len() == group.len();
        if unique {
            for (idx, cap) in group.into_iter().zip(resolved) {
                captions[idx] = cap;
            }
        } else {
            for idx in group {
                let name = names[idx];
                captions[idx] = if name.chars().count() <= CAPTION_LIMIT {
                    name.to_owned()
                } else {
                    middle_trim(name, CAPTION_LIMIT)
                };
            }
        }
    }
    captions
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

fn read_thumbnail(client: &BoxClient, artifact: Artifact) -> (String, Version, Option<std::sync::Arc<gpui::Image>>) {
    let fetched = client.artifact_bytes(&artifact).or_else(|error| match &error {
        kuvo_box::Error::Unreachable(_) => client.artifact_bytes(&artifact),
        kuvo_box::Error::Refused(refusal) if refusal.status >= 500 => client.artifact_bytes(&artifact),
        _ => Err(error),
    });
    let image = fetched.ok().and_then(|bytes| match crate::pictures::shown(bytes) {
        crate::pictures::Read::Shown { thumb, .. } => Some(thumb),
        _ => None,
    });
    (artifact.id, artifact.latest, image)
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
    fn session_rows_use_box_activity_and_are_kept_when_it_is_unchanged(cx: &mut gpui::TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, 1000.);
        cx.update(|cx| {
            let bars = cx.new(|cx| WorkBars::new(BoxClient::new(stand_in), "ses_mira".into(), false, cx));
            bars.update(cx, |bars, cx| {
                bars.set_bot(Some("bot_mira".into()), cx);
                bars.read_at_once(cx);
                let background: Background = serde_json::from_value(json!({ "activity": {
                    "working": 2, "asking": 1, "sessions": [
                        { "session": "ses_signup", "title": "Sign-up", "state": "working" },
                        { "session": "ses_cms", "title": "CMS", "state": "asking" },
                        { "session": "ses_extra", "title": "On another computer", "state": "working", "computer": "peer" }
                    ]
                }})).unwrap();
                bars.take_background(background.clone(), cx);
                let first = bars.active_rows.clone();
                assert_eq!(first.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(), ["ses_signup", "ses_cms", "ses_extra"]);
                bars.take_background(background, cx);
                assert!(Rc::ptr_eq(&first, &bars.active_rows));
            });
        });
    }

    #[gpui::test]
    fn current_bot_work_replaces_a_failed_background_snapshot(cx: &mut gpui::TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, 1000.);
        stand_in.set_bot_work("bot_mira", vec![]);
        cx.update(|cx| {
            let client = BoxClient::new(stand_in);
            let bars = cx.new(|cx| WorkBars::new(client.clone(), "ses_mira".into(), false, cx));
            bars.update(cx, |bars, cx| {
                bars.set_bot(Some("bot_mira".into()), cx);
                let background: Background = serde_json::from_value(json!({ "activity": {
                    "working": 1, "asking": 0, "sessions": [
                        { "session": "ses_previous", "title": "Previous work", "state": "working" }
                    ]
                }})).unwrap();
                bars.take_background(background.clone(), cx);
                bars.take_work(client.bot_work("bot_mira").unwrap());
                assert!(bars.is_running());
                assert_eq!(bars.active_rows[0].id, "ses_previous");

                bars.take_background_update(Followed::Failed(kuvo_box::Error::Unreachable("offline".into())), cx);
                assert!(!bars.is_running());
                assert_eq!((bars.activity().working, bars.activity().asking), (0, 0));
                assert!(bars.active_rows.is_empty());

                bars.work_disconnected(cx);
                assert!(!bars.is_running());
                assert_eq!((bars.activity().working, bars.activity().asking), (0, 0));
                assert!(bars.active_rows.is_empty());

                bars.take_background(background, cx);
                assert!(bars.is_running());
                assert_eq!(bars.active_rows[0].id, "ses_previous");
            });
        });
    }

    /// A live popover draws nothing until its box has said what runs for the conversation (and,
    /// a bot's, its work), or that it couldn't: until then it shows it loading.
    #[gpui::test]
    fn the_popover_waits_until_what_runs_and_a_bots_work_are_heard(cx: &mut gpui::TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, 1000.);
        cx.update(|cx| {
            let bars = cx.new(|cx| WorkBars::new(BoxClient::new(stand_in.clone()), "ses_mira".into(), true, cx));
            bars.update(cx, |bars, cx| {
                assert!(!bars.heard(), "nothing said yet");
                bars.take_background_update(Followed::Failed(kuvo_box::Error::Unreachable("offline".into())), cx);
                assert!(bars.heard(), "a box that couldn't say is heard too: no endless loading");
                bars.set_bot(Some("bot_mira".into()), cx);
                assert!(!bars.heard(), "a bot's conversation waits for its work too");
                bars.take_work(Work::default());
                assert!(bars.heard());
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
    fn files_with_one_name_are_told_apart_by_their_folders() {
        let listed: Vec<Artifact> = serde_json::from_value(json!([
            { "id": "a", "path": "/w/gpui/kuvo/src/fleet.rs", "name": "fleet.rs", "kind": "code", "type": "text/plain", "latest": { "n": 1 } },
            { "id": "b", "path": "/w/gpui/kuvo/tests/fleet.rs", "name": "fleet.rs", "kind": "code", "type": "text/plain", "latest": { "n": 1 } },
            { "id": "c", "path": "/w/gpui/kuvo/src/lib.rs", "name": "lib.rs", "kind": "code", "type": "text/plain", "latest": { "n": 1 } },
            { "id": "d", "path": "/w/one/src/mod.rs", "name": "mod.rs", "kind": "code", "type": "text/plain", "latest": { "n": 1 } },
            { "id": "e", "path": "/w/two/src/mod.rs", "name": "mod.rs", "kind": "code", "type": "text/plain", "latest": { "n": 1 } },
        ])).unwrap();
        let told = telling_folders(&listed);
        assert_eq!(told.get("a").map(String::as_str), Some("src"));
        assert_eq!(told.get("b").map(String::as_str), Some("tests"));
        assert_eq!(told.get("c"), None, "a name only one has needs no folder");
        assert_eq!((told.get("d").map(String::as_str), told.get("e").map(String::as_str)), (Some("one/src"), Some("two/src")));
    }

    #[test]
    fn mixed_activity_specifies_whom_sessions_wait_for() {
        assert_eq!(sessions_label(2, 1), "2 working · 1 waiting for you");
    }

    #[test]
    fn strip_captions_drops_shared_long_prefix_and_extension_for_distinct_labels() {
        let captions = strip_captions(&[
            "screens-insights-overview-light.png",
            "screens-insights-overview-dark.png",
            "screens-computer-pairing-waiting-light.png",
        ]);
        assert_eq!(captions, vec!["insights-light", "insights-dark", "computer-light"]);
        assert_eq!(captions.iter().collect::<HashSet<_>>().len(), 3);
    }

    #[test]
    fn strip_captions_keeps_differing_part_when_three_names_share_same_prefix_and_suffix() {
        let short = strip_captions(&[
            "screens-settings-models-providers-light.png",
            "screens-settings-models-accounts-light.png",
            "screens-settings-models-defaults-light.png",
        ]);
        assert_eq!(short, vec!["providers-light", "accounts-light", "defaults-light"]);

        let long = strip_captions(&[
            "screens-settings-models-providers-overview-light.png",
            "screens-settings-models-accounts-overview-light.png",
            "screens-settings-models-defaults-overview-light.png",
        ]);
        assert_eq!(long, vec!["providers", "accounts", "defaults"]);
        assert_eq!(long.iter().collect::<HashSet<_>>().len(), 3);
    }

    #[test]
    fn strip_captions_disambiguates_colliding_outer_tokens() {
        let captions = strip_captions(&[
            "screens-insights-alpha-overview-light.png",
            "screens-insights-beta-overview-light.png",
            "screens-insights-gamma-overview-light.png",
        ]);
        assert_eq!(captions, vec!["alpha", "beta", "gamma"]);
        assert_eq!(captions.iter().collect::<HashSet<_>>().len(), 3);
    }

    #[test]
    fn session_icon_choice_matches_usage_ring_marks_for_providers_models_and_accounts() {
        // Explicit agent/provider IDs (including account-scoped keys) map to usage-ring marks.
        assert_eq!(
            session_mark("claude-code").map(|(s, p, _)| (s, p)),
            Some(("claude", "marks/claude.svg")),
        );
        assert_eq!(
            session_mark("anthropic@work").map(|(s, p, _)| (s, p)),
            Some(("claude", "marks/claude.svg")),
        );
        assert_eq!(
            session_mark("codex-cli@personal").map(|(s, p, _)| (s, p)),
            Some(("codex", "marks/codex.svg")),
        );
        assert_eq!(
            session_mark("openai").map(|(s, p, _)| (s, p)),
            Some(("codex", "marks/codex.svg")),
        );
        assert_eq!(
            session_mark("chatgpt").map(|(s, p, _)| (s, p)),
            Some(("codex", "marks/codex.svg")),
        );
        assert_eq!(
            session_mark("google").map(|(s, p, _)| (s, p)),
            Some(("google", "marks/providers/google.svg")),
        );
        assert_eq!(
            session_mark("gemini-cli").map(|(s, p, _)| (s, p)),
            Some(("google", "marks/providers/google.svg")),
        );
        assert_eq!(
            session_mark("antigravity-cli").map(|(s, p, _)| (s, p)),
            Some(("antigravity", "marks/antigravity.svg")),
        );
        assert_eq!(
            session_mark("xai").map(|(s, p, _)| (s, p)),
            Some(("grok", "marks/grok.svg")),
        );
        assert_eq!(
            session_mark("grok-cli").map(|(s, p, _)| (s, p)),
            Some(("grok", "marks/grok.svg")),
        );

        // Model ID fallback covers Auto-routed or provider-less sessions.
        assert_eq!(
            resolve_session_provider(Some("auto"), Some("claude-opus-4-6"), None).as_deref(),
            Some("claude"),
        );
        assert_eq!(
            resolve_session_provider(Some("auto"), Some("gpt-5.4"), None).as_deref(),
            Some("codex"),
        );
        assert_eq!(
            resolve_session_provider(None, Some("gemini-3-pro"), None).as_deref(),
            Some("google"),
        );
        assert_eq!(
            resolve_session_provider(None, Some("google/gemini-2.5-pro"), None).as_deref(),
            Some("google"),
        );
        assert_eq!(
            resolve_session_provider(None, Some("grok-4"), None).as_deref(),
            Some("grok"),
        );

        // Account fallback works when neither providerID nor model ID is present.
        assert_eq!(
            resolve_session_provider(None, None, Some("Codex · ChatGPT")).as_deref(),
            Some("codex"),
        );
        assert_eq!(
            resolve_session_provider(None, None, Some("Antigravity")).as_deref(),
            Some("antigravity"),
        );
    }
}
