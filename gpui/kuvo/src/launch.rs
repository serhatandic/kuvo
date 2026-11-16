//! What the window shows as the app opens: this computer's Kuvo set up where it isn't yet, then
//! the app itself ([`Home`]).
//!
//! It decides nothing about the computer. The box's installer, carried in the app, says what this
//! computer holds and what to run next (`kuvo_box::install`); this runs that, draws what it says
//! in the first run's own card (the Welcome's row for this Mac), and attaches. A build that isn't
//! the app in its bundle never installs: it only attaches to what's there.
//!
//! - Someone whose Kuvo is here already sees the quiet "Starting…" the window always had, then
//!   their sessions.
//! - A new Mac sees the Welcome card, its row saying it's being set up, then the same card's
//!   "Ready" from the first run itself.
//! - A refusal or failure is the box's own words under the row, with Try Again (Return).
//! - Someone with an earlier Kuvo Code's sessions sees a card of its own ahead of the Welcome:
//!   the move's steps as the box lists them, where it is, where it stopped and why in the box's
//!   words with Try Again, then that this Mac is ready, then, where this Mac's Kuvo says macOS
//!   doesn't allow Kuvo Helper something yet, the first run's step for it. They land in their
//!   sessions (the box counts a moved Mac as welcomed).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Task, Window, div, prelude::FluentBuilder as _, px};
use kuvo_box::BoxClient;
use kuvo_box::boxes::Boxes;
use kuvo_box::install::{End, Installer, MoveFacts, OnlyAttaches, Outcome, Progress, Step, Verb};
use kuvo_ui::{ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TypeSize, small::spinner, theme::{radius, space}};

use crate::first_run::{card_alone, card_frame, heading, icon_tile, list, two_lines};
use crate::permissions::HeadsUp;
use crate::{Home, title_bar};

/// How long, and how often, a box just made or started is waited for before the window says it
/// didn't start.
const STARTS_WITHIN: Duration = Duration::from_secs(60);
const ASKED_EVERY: Duration = Duration::from_millis(500);
/// How long a Kuvo that's here already is waited for as the app opens (at login the two start
/// together; a move just finished starts it).
const HERE_WITHIN: Duration = Duration::from_secs(20);
/// How often a move still at work from an earlier open is asked about.
const MOVE_ASKED_EVERY: Duration = Duration::from_secs(2);
/// How long that's kept up before the window says it's taking long, with Try Again.
pub const WAITS_WITHIN: Duration = Duration::from_secs(30 * 60);

/// This computer, as the window names it before any box has said its name.
pub const HERE: &str = if cfg!(target_os = "macos") { "This Mac" } else { "This computer" };

const HERE_IN_WORDS: &str = if cfg!(target_os = "macos") { "this Mac" } else { "this computer" };

const SETTING_UP: &str = if cfg!(target_os = "macos") { "Setting up Kuvo on this Mac…" } else { "Setting up Kuvo on this computer…" };

const NOT_ANSWERING: &str = if cfg!(target_os = "macos") { "Kuvo on this Mac isn’t answering." } else { "Kuvo on this computer isn’t answering." };

/// Your boxes, and the one the window opens with; or why there's none to show.
pub type Attached = Result<(Boxes, BoxClient), String>;

/// How this app sets up and reaches this computer's Kuvo.
#[derive(Clone)]
pub struct Opens {
    /// The installer the app carries, or why this build installs nothing.
    pub installer: Result<Installer, OnlyAttaches>,
    /// Reaches your boxes: the one on this computer, and the computers kept from the last run.
    pub attach: Arc<dyn Fn() -> Attached + Send + Sync>,
    /// How long something else at work on this computer's Kuvo is waited for ([`WAITS_WITHIN`];
    /// a test's is shorter).
    pub waits_within: Duration,
}

/// What the window shows.
#[derive(Clone)]
pub enum Stage {
    /// The installer is asked what this computer holds; or a box that's here is being reached.
    Asking,
    /// It's being set up (`new`) or moved (`move`). `title`: the box's words for its step; `at`:
    /// which step that is, of how many, where the box says (a move's).
    SettingUp { verb: Verb, title: Option<SharedString>, at: Option<(u32, u32)> },
    /// Made or started: its server is up in a moment.
    Starting,
    /// It went no further: `line` says what, `message` why, in the box's words where it gave
    /// any; `note` a second, quieter line.
    Stopped { line: SharedString, message: SharedString, note: Option<SharedString> },
    /// Its Kuvo answers, with something to say before the app opens (the way on is a button).
    Said { what: Said, boxes: Boxes, first: BoxClient },
    /// After a move: what macOS doesn't allow the program this Mac's agents run under yet, each
    /// with its own Open System Settings, before the app opens ([`crate::permissions`]).
    Allow { boxes: Boxes, first: BoxClient },
    Open(Entity<Home>),
}

/// What's said before the app opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Said {
    /// An earlier Kuvo Code's sessions were moved.
    Moved,
    /// What the box says of what's here that it left as it is (a Mac with two setups), in its words.
    Left(SharedString),
}

/// A move from an earlier Kuvo Code, as the box has said it so far.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Moving {
    /// Every step in order, in the box's words (none where it lists none: only the one it's at
    /// is drawn then).
    pub steps: Vec<(String, SharedString)>,
    /// The step it's at, from 1, of how many, and the box's words for it.
    pub at: Option<u32>,
    pub of: Option<u32>,
    pub title: Option<SharedString>,
    /// Past its last step: Kuvo is being started.
    pub finishing: bool,
    /// The step it stopped at, from 1.
    pub stopped_at: Option<u32>,
    /// What it didn't carry over, each in the box's words.
    pub dropped: Vec<SharedString>,
}

impl Moving {
    pub fn listing(facts: &MoveFacts) -> Moving {
        Moving { steps: facts.steps.iter().map(|(step, title)| (step.clone(), title.clone().into())).collect(), ..Moving::default() }
    }

    fn take(&mut self, step: &Step) {
        self.at = step.index.or(self.at);
        self.of = step.of.or(self.of);
        self.title = step.title.clone().map(SharedString::from);
        self.finishing = step.step == "done";
    }

    /// How a step in the list stands (`place` from 1).
    fn state_of(&self, place: u32) -> StepState {
        if let Some(stopped) = self.stopped_at {
            return if place < stopped { StepState::Done } else if place == stopped { StepState::Stopped } else { StepState::Ahead };
        }
        match self.at {
            _ if self.finishing => StepState::Done,
            Some(at) if place < at => StepState::Done,
            Some(at) if place == at => StepState::Now,
            _ => StepState::Ahead,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StepState {
    Done,
    Now,
    Stopped,
    Ahead,
}

pub struct Launch {
    opens: Opens,
    live: bool,
    stage: Stage,
    /// Whether it's drawn in the first run's card (a computer being set up or moved) rather than
    /// the quiet middle of the window (one whose Kuvo is here already).
    card: bool,
    /// The installer said a step while it was still being asked what's here (a computer with no
    /// engine yet has it fetched first): the quiet line says it's being set up.
    // BOX-NEEDED: whether this is a first setup isn't known until `status` answers, so the
    // Welcome card can't be shown for a new Mac's download; the installer could say so first.
    setting_up: bool,
    /// How long something else has been waited for (`waits`), since the start or Try Again.
    waited: Duration,
    /// A move from an earlier Kuvo Code under way or just done, drawn in a card of its own.
    moving: Option<Moving>,
    /// What the box said of what's here that it left as it is: said once its Kuvo answers.
    left: Option<SharedString>,
    /// The program this Mac's agents run under, as its Kuvo names it (said once a move is done).
    helper: Option<SharedString>,
    /// What macOS allows this Mac's agents, asked once a move is done: a step before the app
    /// opens, where something isn't allowed yet.
    allow: Option<(Entity<HeadsUp>, gpui::Subscription)>,
    focus: FocusHandle,
    wants_focus: bool,
    _work: Option<Task<()>>,
}

impl Launch {
    /// `live`: whether the app it opens follows your boxes (the app), or shows what it's handed
    /// (tests and shots).
    pub fn new(opens: Opens, live: bool, window: &mut Window, cx: &mut Context<Self>) -> Launch {
        let mut launch = Launch { opens, live, stage: Stage::Asking, card: false, setting_up: false, waited: Duration::ZERO, moving: None, left: None, helper: None, allow: None, focus: cx.focus_handle(), wants_focus: true, _work: None };
        launch.begin(window, cx);
        launch
    }

    pub fn stage(&self) -> &Stage {
        &self.stage
    }

    /// The app, once it's open.
    pub fn home(&self) -> Option<&Entity<Home>> {
        match &self.stage {
            Stage::Open(home) => Some(home),
            _ => None,
        }
    }

    /// From the start (as the app opens, and Try Again): asks the installer, runs what it names.
    pub fn begin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.stage, Stage::Open(_) | Stage::Said { .. } | Stage::Allow { .. }) {
            return;
        }
        self.stage = Stage::Asking;
        // (A move tried again keeps its card and its list while the box is asked: only where it
        // stopped is forgotten.)
        self.card = self.moving.is_some();
        if let Some(moving) = &mut self.moving {
            *moving = Moving { steps: std::mem::take(&mut moving.steps), ..Moving::default() };
            self.stage = Stage::SettingUp { verb: Verb::Move, title: None, at: None };
        }
        self.left = None;
        self.setting_up = false;
        self.waited = Duration::ZERO;
        cx.notify();
        self.ask(window, cx);
    }

    /// Asks the installer what this computer holds and runs what it names, what's drawn left as
    /// it is until it says.
    fn ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let installer = match self.opens.installer.clone() {
            Ok(installer) => installer,
            // A build that installs nothing only attaches, and says so where there's nothing to
            // attach to.
            Err(OnlyAttaches(why)) => return self.attach(None, None, Some(why.into()), window, cx),
        };
        let (said, drawn) = async_channel::unbounded();
        let work = cx.background_spawn(async move { installer.set_up(&mut |progress| drop(said.send_blocking(progress))) });
        self._work = Some(cx.spawn_in(window, async move |this, cx| {
            // Each step as it's said, then how it ended (the steps' sender goes with the work).
            while let Ok(progress) = drawn.recv().await {
                if this.update(cx, |this, cx| this.progress(progress, cx)).is_err() {
                    return;
                }
            }
            let outcome = work.await;
            this.update_in(cx, |this, window, cx| this.ended(outcome, window, cx)).ok();
        }));
    }

    /// What the installer is doing now.
    pub fn progress(&mut self, progress: Progress, cx: &mut Context<Self>) {
        match progress {
            Progress::Asking => {}
            Progress::Running { verb, verdict, moved } => {
                // A Mac whose Kuvo is here and only needs starting again stays quiet: nothing is
                // being set up for whoever sees it.
                self.card = verdict.as_deref() != Some("ours");
                self.stage = Stage::SettingUp { verb, title: None, at: None };
                self.moving = (self.card && verb == Verb::Move).then(|| Moving::listing(&moved));
            }
            Progress::Step(step) => match &mut self.stage {
                // (A move's card already up, the box asked again: what it does first isn't one of
                // the move's steps, and is said under them.)
                Stage::SettingUp { verb: Verb::Move, .. } if self.moving.is_some() && step.index.is_none() => {
                    if let Some(moving) = &mut self.moving {
                        moving.title = step.title.map(SharedString::from);
                    }
                }
                Stage::SettingUp { title, at, .. } => {
                    if let Some(moving) = &mut self.moving {
                        moving.take(&step);
                    }
                    *title = step.title.map(SharedString::from);
                    *at = step.index.zip(step.of);
                }
                Stage::Asking => self.setting_up = true,
                _ => {}
            },
        }
        cx.notify();
    }

    /// How the installer's run ended.
    fn ended(&mut self, outcome: Outcome, window: &mut Window, cx: &mut Context<Self>) {
        let said = outcome.message.clone().map(SharedString::from);
        // What a move watched here didn't carry over, as the box says it now.
        if let Some(moving) = &mut self.moving {
            if moving.steps.is_empty() {
                moving.steps = Moving::listing(&outcome.moved).steps;
            }
            if !outcome.moved.dropped.is_empty() {
                moving.dropped = outcome.moved.dropped.iter().cloned().map(SharedString::from).collect();
            }
        }
        // (The box asked again as a move is tried again, and there's no move to run after all.)
        if !matches!(outcome.end, End::Waits | End::Stopped | End::Started | End::Broken) && !(outcome.end == End::Attach && outcome.verdict.as_deref() == Some("ours")) {
            self.moving = None;
            self.card = false;
        }
        match outcome.end {
            // This Mac's own Kuvo is here and kept running: it's up, or will be in a moment.
            End::Attach if outcome.verdict.as_deref() == Some("ours") => {
                // (A move watched from here, finished by the box's own run: said as done.)
                if let Some(moving) = &mut self.moving {
                    moving.finishing = true;
                }
                self.stage = Stage::Starting;
                cx.notify();
                self.await_box(HERE_WITHIN, window, cx);
            }
            // What's here is left as it is, and the box says so in words (a Mac with two setups):
            // said once its Kuvo answers, before the app opens.
            End::Attach => {
                self.left = said.clone().filter(|_| outcome.verdict.as_deref() == Some("two"));
                self.attach(None, said, None, window, cx)
            }
            End::Started => {
                match &mut self.moving {
                    Some(moving) if outcome.ran == Some(Verb::Move) => moving.finishing = true,
                    _ => self.moving = None,
                }
                self.stage = Stage::Starting;
                cx.notify();
                self.await_box(STARTS_WITHIN, window, cx);
            }
            // Whatever stopped it, a box that answers here is still the user's to use (an earlier
            // build's that keeps running until it's moved; one whose update couldn't be checked).
            End::NotStarted => self.attach(Some("Kuvo is set up, but isn’t running".into()), said, None, window, cx),
            // A move the box still runs in a process of its own (the app quit while it worked):
            // drawn where it is, and asked about again until it's done.
            // Something else holds this computer's Kuvo: a move the box still runs in a process of
            // its own (the app quit while it worked), drawn where it is; or its setup, begun by
            // another open of the app. Asked about again until it's done, for a while: past
            // that, said, with Try Again.
            End::Waits if self.waited < self.opens.waits_within => {
                match outcome.progress {
                    Some(step) => {
                        self.card = true;
                        let mut moving = Moving::listing(&outcome.moved);
                        moving.take(&step);
                        self.moving = Some(moving);
                        self.stage = Stage::SettingUp { verb: Verb::Move, title: step.title.map(SharedString::from), at: step.index.zip(step.of) };
                    }
                    // (No move's step to draw: the quiet line, which claims no more than that.)
                    None => {
                        self.setting_up = true;
                        if !matches!(self.stage, Stage::SettingUp { .. }) {
                            self.stage = Stage::Asking;
                        }
                    }
                }
                self.waited += MOVE_ASKED_EVERY;
                cx.notify();
                self._work = Some(cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(MOVE_ASKED_EVERY).await;
                    this.update_in(cx, |this, window, cx| this.ask(window, cx)).ok();
                }));
            }
            End::Waits => {
                let moving = outcome.progress.is_some() || matches!(self.stage, Stage::SettingUp { verb: Verb::Move, .. });
                let line = if moving { "Still moving your sessions" } else { "Kuvo is still being set up" };
                self.stop(line.into(), said.unwrap_or_else(|| format!("It’s taking longer than expected on {HERE_IN_WORDS}. It goes on by itself: try again in a while.").into()), None, cx);
            }
            End::Stopped if outcome.ran == Some(Verb::Move) => {
                // Where it stopped, as the box says (else the step it was last at); and whether
                // it stopped where it only goes forward from, which trying again finishes.
                let stuck = outcome.verdict.as_deref() == Some("stuck");
                if let Some(moving) = &mut self.moving {
                    moving.stopped_at = outcome.moved.stopped_at.as_deref().and_then(|step| outcome.moved.place_of(step)).or(moving.at);
                    moving.finishing = false;
                }
                let line = if stuck { "Couldn’t finish moving" } else { "Couldn’t move your sessions" };
                self.attach(Some(line.into()), said, None, window, cx)
            }
            End::Stopped => self.attach(Some("Couldn’t set up Kuvo".into()), said, None, window, cx),
            // (An installer that broke under a move's card: said there.)
            End::Broken if self.moving.is_some() => self.attach(Some("Couldn’t move your sessions".into()), Some(format!("Kuvo couldn’t go on with the move on {HERE_IN_WORDS}. {}", outcome.message.unwrap_or_default()).trim().to_owned().into()), None, window, cx),
            End::Broken => self.attach(Some("Couldn’t set up Kuvo".into()), Some(format!("Kuvo couldn’t be set up on {HERE_IN_WORDS}. {}", outcome.message.unwrap_or_default()).trim().to_owned().into()), None, window, cx),
        }
    }

    /// Reaches what's here, once. `line` and `message`: what stopped setting up, said where
    /// nothing answers; `note`: a second line.
    fn attach(&mut self, line: Option<SharedString>, message: Option<SharedString>, note: Option<SharedString>, window: &mut Window, cx: &mut Context<Self>) {
        let attach = self.opens.attach.clone();
        let reached = cx.background_spawn(async move { attach() });
        self._work = Some(cx.spawn_in(window, async move |this, cx| {
            let reached = reached.await;
            this.update_in(cx, |this, window, cx| match reached {
                // (What stopped setting up here isn't passed over for another computer's
                // sessions: it's said, with Try Again.)
                Ok((boxes, _)) if line.is_some() && boxes.nearest().is_none() => this.stop(line.unwrap_or_default(), message.unwrap_or_else(|| NOT_ANSWERING.into()), note, cx),
                // (An earlier build's Kuvo still answers while its move can't be made: it's used.)
                Ok((boxes, first)) if line.is_some() => {
                    this.moving = None;
                    this.open(boxes, first, window, cx)
                }
                Ok((boxes, first)) => this.reached(boxes, first, window, cx),
                Err(why) => this.stop(line.unwrap_or_else(|| "Kuvo isn’t running".into()), message.unwrap_or_else(|| why.into()), note, cx),
            })
            .ok();
        }));
    }

    /// Waits up to `within` for this computer's Kuvo to answer, asking again every half second.
    fn await_box(&mut self, within: Duration, window: &mut Window, cx: &mut Context<Self>) {
        let attach = self.opens.attach.clone();
        self._work = Some(cx.spawn_in(window, async move |this, cx| {
            let mut waited = Duration::ZERO;
            loop {
                let asked = attach.clone();
                let reached = cx.background_spawn(async move { asked() }).await;
                let here = reached.as_ref().is_ok_and(|(boxes, _)| boxes.nearest().is_some());
                if here || waited >= within {
                    let wait_more = Some("It may still be starting. Try again in a moment.".into());
                    this.update_in(cx, |this, window, cx| match reached {
                        Ok((boxes, first)) if here => this.reached(boxes, first, window, cx),
                        Ok(_) => this.stop("Kuvo didn’t start".into(), NOT_ANSWERING.into(), wait_more, cx),
                        Err(why) => this.stop("Kuvo didn’t start".into(), why.into(), Some("It may still be starting. Try again in a moment.".into()), cx),
                    })
                    .ok();
                    return;
                }
                cx.background_executor().timer(ASKED_EVERY).await;
                waited += ASKED_EVERY;
            }
        }));
    }

    fn stop(&mut self, line: SharedString, message: SharedString, note: Option<SharedString>, cx: &mut Context<Self>) {
        self.stage = Stage::Stopped { line, message, note };
        self.wants_focus = true;
        cx.notify();
    }

    /// Its Kuvo answers: the app opens, after what's to be said first (a move that's done here;
    /// what the box left as it is), with a button to go on.
    fn reached(&mut self, boxes: Boxes, first: BoxClient, window: &mut Window, cx: &mut Context<Self>) {
        let what = match (self.left.take(), &self.moving) {
            (Some(words), _) if boxes.nearest().is_some() => Said::Left(words),
            (_, Some(moving)) if moving.finishing => Said::Moved,
            _ => return self.open(boxes, first, window, cx),
        };
        // Whoever comes from an earlier Kuvo Code is asked what macOS should allow the program
        // their agents run under now, as this Mac's Kuvo says it (a step after this card), or,
        // where nothing needs them, told once what macOS will name from now on.
        if what == Said::Moved
            && let Some(nearest) = boxes.nearest()
        {
            let allow = cx.new(|cx| HeadsUp::new(nearest, self.live, window, cx));
            let seen = cx.observe(&allow, |_, _, cx| cx.notify());
            allow.update(cx, |allow, cx| allow.read(cx));
            self.allow = Some((allow, seen));
        }
        self.card = true;
        self.stage = Stage::Said { what, boxes, first };
        self.wants_focus = true;
        cx.notify();
    }

    /// The way on from what was said: the permissions' step, where this Mac's Kuvo has said
    /// something isn't allowed yet (one that hasn't answered by now is passed over: Settings › This
    /// Mac has it); then the app opens. From that step, once nothing is asked about.
    pub fn go_on(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.stage.clone() {
            Stage::Said { what: Said::Moved, boxes, first } if self.asks(cx) => {
                self.stage = Stage::Allow { boxes, first };
                self.wants_focus = true;
                cx.notify();
            }
            Stage::Said { boxes, first, .. } => self.open(boxes, first, window, cx),
            Stage::Allow { boxes, first } if !self.asks(cx) => self.open(boxes, first, window, cx),
            _ => {}
        }
    }

    /// Not Now on the permissions' step: what it asked about is put off (the box keeps it), and
    /// the app opens.
    pub fn not_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Stage::Allow { boxes, first } = self.stage.clone() else { return };
        if let Some((allow, _)) = &self.allow {
            allow.update(cx, |allow, cx| allow.not_now(cx));
        }
        self.open(boxes, first, window, cx);
    }

    /// Whether this Mac's Kuvo says macOS doesn't allow something yet that it asks about.
    fn asks(&self, cx: &App) -> bool {
        self.allow.as_ref().is_some_and(|(allow, _)| allow.read(cx).asks())
    }

    /// The app, in the window's place, the keyboard in its composer.
    fn open(&mut self, boxes: Boxes, first: BoxClient, window: &mut Window, cx: &mut Context<Self>) {
        let live = self.live;
        // The release this app carries is offered to the box on this computer, once as it opens
        // (PROTOCOL.md, "Updates"): the box takes it if it's newer than itself, and says no
        // otherwise, as it should.
        if let (Ok(installer), Some(nearest)) = (&self.opens.installer, boxes.nearest()) {
            let release = installer.carried().release.clone();
            offer(nearest, release, cx);
        }
        let home = cx.new(|cx| Home::new(boxes, first, live, window, cx));
        // And where this Mac's box is from before it took that release itself the moment its work
        // ended, the installer brings it up (once that work ends, or on Update Now).
        if let Ok(installer) = &self.opens.installer {
            let installer = installer.clone();
            home.update(cx, |home, cx| home.update_own_box(installer, cx));
        }
        let landing = home.read(cx).workspace().read(cx).landing().focus_handle(cx);
        window.focus(&landing, cx);
        self.stage = Stage::Open(home);
        self.moving = None;
        // (The window's own takes over from here, for Settings › This Mac.)
        if let Some((allow, _)) = self.allow.take() {
            allow.update(cx, |allow, cx| allow.stop_asking(cx));
        }
        self._work = None;
        cx.notify();
    }

    /// Draws a stage as it stands, with nothing run (shots, and tests of what's drawn).
    #[cfg(feature = "stand-in")]
    pub fn show(&mut self, stage: Stage, card: bool, cx: &mut Context<Self>) {
        self._work = None;
        self.stage = stage;
        self.card = card;
        cx.notify();
    }

    /// Draws the permissions' step after a move, as this Mac's Kuvo says it (shots).
    #[cfg(feature = "stand-in")]
    pub fn show_allow(&mut self, boxes: Boxes, first: BoxClient, window: &mut Window, cx: &mut Context<Self>) {
        let Some(nearest) = boxes.nearest() else { return };
        let allow = cx.new(|cx| HeadsUp::new(nearest, self.live, window, cx));
        allow.update(cx, |allow, cx| allow.read(cx));
        let seen = cx.observe(&allow, |_, _, cx| cx.notify());
        self.allow = Some((allow, seen));
        self.show(Stage::Allow { boxes, first }, true, cx);
    }

    /// Draws a move as it stands, with nothing run (shots).
    #[cfg(feature = "stand-in")]
    pub fn show_move(&mut self, stage: Stage, moving: Moving, helper: Option<&str>, cx: &mut Context<Self>) {
        self.moving = Some(moving);
        self.helper = helper.map(|helper| SharedString::from(helper.to_owned()));
        self.show(stage, true, cx);
    }

    /// The move drawn, as the box has said it.
    pub fn moving(&self) -> Option<&Moving> {
        self.moving.as_ref()
    }

    // MARK: Drawing

    /// What this computer's row says now, and whether it's still at work.
    pub fn line(&self) -> (SharedString, bool) {
        match &self.stage {
            Stage::SettingUp { verb, title, .. } => {
                let doing = if *verb == Verb::Move { "Moving your sessions" } else { "Setting up" };
                (title.as_ref().map_or_else(|| format!("{doing}…"), |title| format!("{doing} · {title}")).into(), true)
            }
            Stage::Stopped { line, .. } => (line.clone(), false),
            Stage::Said { .. } | Stage::Allow { .. } => ("Ready".into(), false),
            Stage::Asking | Stage::Starting | Stage::Open(_) => ("Starting…".into(), true),
        }
    }

    /// Why it stopped, as it's said under the row: the box's words where it gave any.
    pub fn message(&self) -> Option<SharedString> {
        match &self.stage {
            Stage::Stopped { message, .. } => Some(message.clone()),
            _ => None,
        }
    }

    /// What the window's quiet middle says while it waits.
    pub fn waiting_words(&self) -> &'static str {
        if self.setting_up && matches!(self.stage, Stage::Asking) { SETTING_UP } else { "Starting…" }
    }

    /// How far a move is, where the box says: its step, of how many.
    pub fn at(&self) -> Option<(u32, u32)> {
        match &self.stage {
            Stage::SettingUp { at, .. } => *at,
            _ => None,
        }
    }

    /// This computer's row in the Welcome card: what's happening to it, or what went wrong.
    fn row(&self, cx: &App) -> AnyElement {
        let c = &cx.theme().colors;
        let (line, working) = self.line();
        let color = if working { c.text_muted } else { c.danger };
        let trailing: AnyElement = if working { spinner("launch-working", IconSize::Nav, c.text_muted).into_any_element() } else { Icon::new(IconName::Warning).color(c.danger).into_any_element() };
        let row = div()
            .debug_selector(|| "launch-computer".into())
            .flex()
            .items_center()
            .gap(space::S3)
            .px(space::S4)
            .py(space::S3)
            .child(icon_tile(IconName::Computer, cx))
            .child(two_lines(HERE, line, color, cx))
            .child(trailing)
            .into_any_element();
        // A move's steps as a thin bar under the row: how far it is, by the box's count.
        let Some((index, of)) = self.at().filter(|(_, of)| *of > 0) else { return row };
        let done = (index.min(of) as f32 / of as f32).clamp(0., 1.);
        div()
            .flex()
            .flex_col()
            .child(row)
            .child(div().debug_selector(|| "launch-steps".into()).mx(space::S4).mb(space::S3).h(px(3.)).rounded(px(2.)).bg(c.border).child(div().h_full().w(gpui::relative(done)).rounded(px(2.)).bg(c.text_muted)))
            .into_any_element()
    }

    fn try_again(&self, cx: &mut Context<Self>) -> Button {
        Button::new("launch-again", "Try Again").primary().shortcut("↵").size(ButtonSize::Medium).on_click(cx.listener(|this, _, window, cx| this.begin(window, cx)))
    }

    /// The first run's card, its row for this computer saying how setting up goes.
    fn in_card(&self, cx: &mut Context<Self>) -> gpui::Div {
        let c = cx.theme().colors.clone();
        let mut card = vec![heading("Welcome to Kuvo", None, cx), list(vec![self.row(cx)], cx)];
        let stopped = matches!(self.stage, Stage::Stopped { .. });
        if let Stage::Stopped { message, note, .. } = &self.stage {
            card.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::S1)
                    .px(space::S1)
                    .child(div().debug_selector(|| "launch-message".into()).text_ui(TypeSize::Body).text_color(c.text).child(message.clone()))
                    .children(note.clone().map(|note| div().debug_selector(|| "launch-note".into()).text_ui(TypeSize::Small).text_color(c.text_faint).child(note)))
                    .into_any_element(),
            );
        }
        // Where the welcome's Get Started is: waiting while it works, Try Again once it stopped.
        let way_on = if stopped { self.try_again(cx) } else { Button::new("launch-next", "Get Started").primary().shortcut("↵").size(ButtonSize::Medium).loading(true).disabled(true) };
        card.push(div().flex().flex_none().items_center().pt(space::S1).child(div().flex_1()).child(way_on).into_any_element());
        card_frame(card, 0, cx)
    }

    // MARK: A move's own card

    /// One of a move's steps: how it stands, and the box's words for it.
    fn step_row(&self, place: u32, title: SharedString, state: StepState, cx: &App) -> AnyElement {
        let c = &cx.theme().colors;
        let mark: AnyElement = match state {
            StepState::Done => Icon::new(IconName::Check).color(c.success).into_any_element(),
            StepState::Now => spinner(("launch-step", place as usize), IconSize::Nav, c.text_muted).into_any_element(),
            StepState::Stopped => Icon::new(IconName::Warning).color(c.danger).into_any_element(),
            StepState::Ahead => div().size(px(5.)).rounded(radius::PILL).bg(c.border_strong).into_any_element(),
        };
        let selector = format!("launch-step-{place}-{}", match state { StepState::Done => "done", StepState::Now => "now", StepState::Stopped => "stopped", StepState::Ahead => "ahead" });
        div()
            .debug_selector(move || selector)
            .flex()
            .items_center()
            .gap(space::S3)
            .px(space::S4)
            .py(space::S2)
            .child(div().flex().flex_none().items_center().justify_center().size(px(16.)).child(mark))
            .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Small).when_else(state == StepState::Ahead, |d| d.text_color(c.text_faint), |d| d.medium().text_color(c.text)).child(title))
            .children((state == StepState::Stopped).then(|| div().flex_none().text_ui(TypeSize::Caption).text_color(c.danger).child("Stopped here")))
            .into_any_element()
    }

    /// The move's steps as the box lists them; from a box that lists none, the one it's at.
    fn steps(&self, moving: &Moving, cx: &App) -> AnyElement {
        let c = &cx.theme().colors;
        if moving.steps.is_empty() {
            let title = moving.title.clone().unwrap_or_else(|| "Moving your sessions".into());
            let state = if moving.stopped_at.is_some() { StepState::Stopped } else if moving.finishing { StepState::Done } else { StepState::Now };
            let count = moving.at.zip(moving.of).filter(|(_, of)| *of > 0).map(|(at, of)| format!("Step {at} of {of}"));
            return list(
                vec![div().flex().items_center().child(div().flex_1().min_w_0().child(self.step_row(moving.at.unwrap_or(1), title, state, cx))).children(count.filter(|_| state == StepState::Now).map(|count| div().flex_none().pr(space::S4).text_ui(TypeSize::Caption).text_color(c.text_faint).child(count))).into_any_element()],
                cx,
            );
        }
        let rows = moving.steps.iter().enumerate().map(|(ix, (_, title))| self.step_row(ix as u32 + 1, title.clone(), moving.state_of(ix as u32 + 1), cx)).collect();
        div().debug_selector(|| "launch-steps".into()).child(list(rows, cx)).into_any_element()
    }

    /// A card of its own for a move from an earlier Kuvo Code: its steps, where it is, and where
    /// and why it stopped, with Try Again.
    fn move_card(&self, moving: &Moving, cx: &mut Context<Self>) -> gpui::Div {
        let c = cx.theme().colors.clone();
        let stopped = match &self.stage {
            Stage::Stopped { line, message, note } => Some((line.clone(), message.clone(), note.clone())),
            _ => None,
        };
        let head = match &stopped {
            Some((line, ..)) => heading(line, None, cx),
            None => heading("Moving to the new Kuvo", Some(&format!("Your sessions, bots and settings move to their new place on {HERE_IN_WORDS}.")), cx),
        };
        // Why it stopped stays in view above the steps, however short the window (they scroll).
        let mut card = vec![head];
        if let Some((_, message, _)) = &stopped {
            card.push(
                div()
                    .flex()
                    .flex_none()
                    .gap(space::S3)
                    .p(space::S4)
                    .rounded(radius::POPOVER)
                    .border_1()
                    .border_color(c.border)
                    .bg(c.card)
                    .child(Icon::new(IconName::Warning).color(c.danger))
                    .child(div().debug_selector(|| "launch-message".into()).flex_1().min_w_0().text_ui(TypeSize::Small).text_color(c.text).child(message.clone()))
                    .into_any_element(),
            );
        }
        // Where it's past its steps: Kuvo being started, in the box's words.
        // And before its first step (the box asked, or getting what it needs): said too, so a
        // list of steps not begun never looks stuck.
        let starting = if stopped.is_some() {
            None
        } else if moving.finishing || matches!(self.stage, Stage::Starting) {
            Some(moving.title.clone().filter(|_| moving.finishing).unwrap_or_else(|| "Starting…".into()))
        } else if moving.at.is_none() {
            Some(moving.title.clone().map_or_else(|| "Getting ready…".into(), |title| format!("{title}…").into()))
        } else {
            None
        };
        let foot = match &stopped {
            Some((_, _, note)) => div()
                .flex()
                .flex_none()
                .items_center()
                .gap(space::S3)
                .pt(space::S1)
                .child(div().flex_1().min_w_0().children(note.clone().map(|note| div().debug_selector(|| "launch-note".into()).text_ui(TypeSize::Caption).text_color(c.text_faint).child(note))))
                .child(self.try_again(cx)),
            None => div()
                .flex()
                .flex_none()
                .items_center()
                .gap(space::S2)
                .text_ui(TypeSize::Caption)
                .text_color(c.text_faint)
                .children(starting.clone().map(|_| spinner("launch-starting", IconSize::Nav, c.text_muted)))
                .child(div().debug_selector(|| "launch-note".into()).child(starting.unwrap_or_else(|| "This takes a few minutes.".into()))),
        };
        card.push(div().id("launch-body").min_h_0().flex().flex_col().overflow_y_scroll().child(self.steps(moving, cx)).into_any_element());
        card.push(foot.into_any_element());
        card_alone(card, cx)
    }

    /// What's said before the app opens, with the way on: a move that's done, or what the box
    /// left as it is.
    fn said_card(&self, what: &Said, cx: &mut Context<Self>) -> gpui::Div {
        let c = cx.theme().colors.clone();
        let row = |icon: IconName, title: &str, detail: String, done: bool, cx: &App| {
            div()
                .flex()
                .items_center()
                .gap(space::S3)
                .px(space::S4)
                .py(space::S3)
                .child(icon_tile(icon, cx))
                .child(two_lines(title.to_owned(), detail, cx.theme().colors.text_muted, cx))
                .children(done.then(|| Icon::new(IconName::Check).color(cx.theme().colors.success)))
                .into_any_element()
        };
        let next_words = if *what == Said::Moved && self.asks(cx) { "Continue" } else { "Open Kuvo" };
        let (head, mut body, next) = match what {
            Said::Moved => {
                let mut rows = vec![row(IconName::Archive, "Moved from an earlier Kuvo Code", "Your sessions, bots and settings".into(), true, cx)];
                // What macOS names from now on, where this Mac's Kuvo says there's such a program
                // and nothing's to be allowed for it now (else the next step says it).
                let named = self.allow.as_ref().map(|(allow, _)| allow.read(cx)).filter(|allow| !allow.list().is_empty()).map(|allow| allow.program());
                let helper = self.helper.clone().or(named).filter(|_| !self.asks(cx));
                if let Some(helper) = &helper {
                    rows.push(div().debug_selector(|| "launch-helper".into()).child(row(IconName::Shield, "macOS may ask again", format!("For {helper}, which runs your agents now"), false, cx)).into_any_element());
                }
                let ready = if cfg!(target_os = "macos") { "This Mac is ready" } else { "This computer is ready" };
                (heading(ready, None, cx), vec![list(rows, cx)], next_words)
            }
            Said::Left(words) => (
                heading("Your earlier sessions stay where they are", None, cx),
                vec![
                    div()
                        .flex()
                        .flex_none()
                        .gap(space::S3)
                        .p(space::S4)
                        .rounded(radius::POPOVER)
                        .border_1()
                        .border_color(c.border)
                        .bg(c.card)
                        .child(Icon::new(IconName::Info).color(c.text_muted))
                        .child(div().debug_selector(|| "launch-message".into()).flex_1().min_w_0().text_ui(TypeSize::Small).text_color(c.text).child(words.clone()))
                        .into_any_element(),
                ],
                "Continue",
            ),
        };
        // What the move didn't carry over, each in the box's words, said this once.
        let dropped = match (what, &self.moving) {
            (Said::Moved, Some(moving)) => moving.dropped.clone(),
            _ => Vec::new(),
        };
        if !dropped.is_empty() {
            body.push(
                div()
                    .debug_selector(|| "launch-dropped".into())
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(space::S2)
                    .px(space::S1)
                    .child(div().text_ui(TypeSize::Caption).medium().text_color(c.text_faint).child("Not brought along"))
                    .children(dropped.into_iter().map(|words| div().text_ui(TypeSize::Small).text_color(c.text_muted).child(words)))
                    .into_any_element(),
            );
        }
        let way_on = Button::new("launch-next", next).primary().shortcut("↵").size(ButtonSize::Medium).on_click(cx.listener(|this, _, window, cx| this.go_on(window, cx)));
        card_alone(
            vec![head, div().id("launch-body").min_h_0().flex().flex_col().gap(space::S4).overflow_y_scroll().children(body).into_any_element(), div().flex().flex_none().items_center().pt(space::S1).child(div().flex_1()).child(way_on).into_any_element()],
            cx,
        )
    }

    /// After a move, what macOS doesn't allow this Mac's agents yet: the first run's own step.
    fn allow_card(&self, cx: &mut Context<Self>) -> gpui::Div {
        let Some(allow) = self.allow.as_ref().map(|(allow, _)| allow.clone()) else { return card_alone(Vec::new(), cx) };
        let (title, line) = allow.read(cx).heading();
        let asks = allow.read(cx).asks();
        let foot = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(space::S2)
            .pt(space::S1)
            .child(div().flex_1())
            .when(asks, |d| d.child(Button::new("launch-not-now", "Not Now").ghost().on_click(cx.listener(|this, _, window, cx| this.not_now(window, cx)))))
            .child(Button::new("launch-next", "Open Kuvo").primary().shortcut("↵").size(ButtonSize::Medium).disabled(asks).on_click(cx.listener(|this, _, window, cx| this.go_on(window, cx))))
            .into_any_element();
        card_alone(vec![heading(&title, Some(&line), cx), div().id("launch-body").min_h_0().flex().flex_col().gap(space::S4).overflow_y_scroll().child(allow).into_any_element(), foot], cx)
    }

    /// The window's quiet middle, as it waits for a box that's here: "Starting…", or why not.
    fn quiet(&self, cx: &mut Context<Self>) -> gpui::Div {
        let c = cx.theme().colors.clone();
        let words: AnyElement = match &self.stage {
            Stage::Stopped { message, note, .. } => div()
                .flex()
                .flex_col()
                .items_center()
                .gap(space::S2)
                .max_w(px(460.))
                .child(div().debug_selector(|| "launch-message".into()).text_ui(TypeSize::Body).text_color(c.text).child(message.clone()))
                .children(note.clone().map(|note| div().debug_selector(|| "launch-note".into()).text_color(c.text_muted).child(note)))
                .child(div().pt(space::S2).child(self.try_again(cx)))
                .into_any_element(),
            _ => div().flex().items_center().gap(space::S2).text_color(c.text_muted).child(spinner("launch-starting", IconSize::Nav, c.text_muted)).child(self.waiting_words()).into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.background)
            .text_ui(TypeSize::Small)
            .child(div().id("launch-title-bar").h(px(title_bar::HEIGHT)).flex_none().on_mouse_down(MouseButton::Left, |event, window, _| {
                if event.click_count == 1 {
                    window.start_window_move();
                } else if event.click_count == 2 {
                    window.titlebar_double_click();
                }
            }))
            .child(div().flex_1().flex().items_center().justify_center().px(space::S6).pb(px(96.)).child(words))
    }
}

/// Offers the box on this computer the release the app carries, off the window's thread. Nothing
/// is drawn of it: the box updates itself when nothing's at work, and says so itself.
fn offer(nearest: BoxClient, release: PathBuf, cx: &mut App) {
    cx.background_spawn(async move {
        let _ = nearest.offer_release(&release);
    })
    .detach();
}

impl Focusable for Launch {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Launch {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Stage::Open(home) = &self.stage {
            return div().size_full().child(home.clone()).into_any_element();
        }
        if std::mem::take(&mut self.wants_focus) {
            window.focus(&self.focus, cx);
        }
        let frame = match (&self.stage, self.moving.clone()) {
            (Stage::Said { what, .. }, _) => self.said_card(&what.clone(), cx),
            (Stage::Allow { .. }, _) => self.allow_card(cx),
            (_, Some(moving)) if self.card => self.move_card(&moving, cx),
            _ if self.card => self.in_card(cx),
            _ => self.quiet(cx),
        };
        frame
            .id("launch")
            .debug_selector(|| "launch".into())
            .track_focus(&self.focus)
            // Return tries again, where that's the way on.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key != "enter" || event.keystroke.modifiers.modified() {
                    return;
                }
                match this.stage {
                    Stage::Stopped { .. } => this.begin(window, cx),
                    Stage::Said { .. } | Stage::Allow { .. } => this.go_on(window, cx),
                    _ => {}
                }
            }))
            .into_any_element()
    }
}
