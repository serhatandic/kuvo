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

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Render, SharedString, Styled, Task, Window, div, px};
use kuvo_box::BoxClient;
use kuvo_box::boxes::Boxes;
use kuvo_box::install::{End, Installer, OnlyAttaches, Outcome, Progress, Verb};
use kuvo_ui::{ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TypeSize, small::spinner, theme::space};

use crate::first_run::{card_frame, heading, icon_tile, list, two_lines};
use crate::{Home, title_bar};

/// How long, and how often, a box just made or started is waited for before the window says it
/// didn't start.
const STARTS_WITHIN: Duration = Duration::from_secs(60);
const ASKED_EVERY: Duration = Duration::from_millis(500);

/// This computer, as the window names it before any box has said its name.
pub const HERE: &str = if cfg!(target_os = "macos") { "This Mac" } else { "This computer" };

const HERE_IN_WORDS: &str = if cfg!(target_os = "macos") { "this Mac" } else { "this computer" };

/// Your boxes, and the one the window opens with; or why there's none to show.
pub type Attached = Result<(Boxes, BoxClient), String>;

/// How this app sets up and reaches this computer's Kuvo.
#[derive(Clone)]
pub struct Opens {
    /// The installer the app carries, or why this build installs nothing.
    pub installer: Result<Installer, OnlyAttaches>,
    /// Reaches your boxes: the one on this computer, and the computers kept from the last run.
    pub attach: Arc<dyn Fn() -> Attached + Send + Sync>,
}

/// What the window shows.
#[derive(Clone)]
pub enum Stage {
    /// The installer is asked what this computer holds; or a box that's here is being reached.
    Asking,
    /// It's being set up (`new`) or moved (`move`). `title`: the box's words for its step.
    SettingUp { verb: Verb, title: Option<SharedString> },
    /// Made or started: its server is up in a moment.
    Starting,
    /// It went no further: `line` says what, `message` why, in the box's words where it gave
    /// any; `note` a second, quieter line.
    Stopped { line: SharedString, message: SharedString, note: Option<SharedString> },
    Open(Entity<Home>),
}

pub struct Launch {
    opens: Opens,
    live: bool,
    stage: Stage,
    /// Whether it's drawn in the first run's card (a computer being set up or moved) rather than
    /// the quiet middle of the window (one whose Kuvo is here already).
    card: bool,
    focus: FocusHandle,
    wants_focus: bool,
    _work: Option<Task<()>>,
}

impl Launch {
    /// `live`: whether the app it opens follows your boxes (the app), or shows what it's handed
    /// (tests and shots).
    pub fn new(opens: Opens, live: bool, window: &mut Window, cx: &mut Context<Self>) -> Launch {
        let mut launch = Launch { opens, live, stage: Stage::Asking, card: false, focus: cx.focus_handle(), wants_focus: true, _work: None };
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
        if matches!(self.stage, Stage::Open(_)) {
            return;
        }
        self.stage = Stage::Asking;
        self.card = false;
        cx.notify();
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
            Progress::Running { verb, verdict } => {
                // A Mac whose Kuvo is here and only needs starting again stays quiet: nothing is
                // being set up for whoever sees it.
                self.card = verdict.as_deref() != Some("ours");
                self.stage = Stage::SettingUp { verb, title: None };
            }
            Progress::Step { title, .. } => {
                if let Stage::SettingUp { title: shown, .. } = &mut self.stage {
                    *shown = title.map(SharedString::from);
                }
            }
        }
        cx.notify();
    }

    /// How the installer's run ended.
    fn ended(&mut self, outcome: Outcome, window: &mut Window, cx: &mut Context<Self>) {
        let said = outcome.message.clone().map(SharedString::from);
        match outcome.end {
            End::Attach => self.attach(None, said, None, window, cx),
            End::Started => {
                self.stage = Stage::Starting;
                cx.notify();
                self.await_box(window, cx);
            }
            // Whatever stopped it, a box that answers here is still the user's to use (an earlier
            // build's that keeps running until it's moved; one whose update couldn't be checked).
            End::NotStarted => self.attach(Some("Kuvo is set up, but isn’t running".into()), said, None, window, cx),
            End::Waits => self.attach(Some("Waiting for another move to finish".into()), said, None, window, cx),
            End::Stopped => self.attach(Some(if outcome.ran == Some(Verb::Move) { "Couldn’t move your sessions" } else { "Couldn’t set up Kuvo" }.into()), said, None, window, cx),
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
                Ok((boxes, first)) => this.open(boxes, first, window, cx),
                Err(why) => this.stop(line.unwrap_or_else(|| "Kuvo isn’t running".into()), message.unwrap_or_else(|| why.into()), note, cx),
            })
            .ok();
        }));
    }

    /// Waits for the box just made or started to answer, asking again every half second.
    fn await_box(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let attach = self.opens.attach.clone();
        self._work = Some(cx.spawn_in(window, async move |this, cx| {
            let mut waited = Duration::ZERO;
            loop {
                let asked = attach.clone();
                let reached = cx.background_spawn(async move { asked() }).await;
                let here = reached.as_ref().is_ok_and(|(boxes, _)| boxes.nearest().is_some());
                if here || waited >= STARTS_WITHIN {
                    this.update_in(cx, |this, window, cx| match reached {
                        Ok((boxes, first)) => this.open(boxes, first, window, cx),
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
        let landing = home.read(cx).workspace().read(cx).landing().focus_handle(cx);
        window.focus(&landing, cx);
        self.stage = Stage::Open(home);
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

    // MARK: Drawing

    /// What this computer's row says now, and whether it's still at work.
    pub fn line(&self) -> (SharedString, bool) {
        match &self.stage {
            Stage::SettingUp { verb, title } => {
                let doing = if *verb == Verb::Move { "Moving your sessions" } else { "Setting up Kuvo" };
                (title.as_ref().map_or_else(|| format!("{doing}…"), |title| format!("{doing} · {title}")).into(), true)
            }
            Stage::Stopped { line, .. } => (line.clone(), false),
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

    /// This computer's row in the Welcome card: what's happening to it, or what went wrong.
    fn row(&self, cx: &App) -> AnyElement {
        let c = &cx.theme().colors;
        let (line, working) = self.line();
        let color = if working { c.text_muted } else { c.danger };
        let trailing: AnyElement = if working { spinner("launch-working", IconSize::Nav, c.text_muted).into_any_element() } else { Icon::new(IconName::Warning).color(c.danger).into_any_element() };
        div()
            .debug_selector(|| "launch-computer".into())
            .flex()
            .items_center()
            .gap(space::S3)
            .px(space::S4)
            .py(space::S3)
            .child(icon_tile(IconName::Computer, cx))
            .child(two_lines(HERE, line, color, cx))
            .child(trailing)
            .into_any_element()
    }

    fn try_again(&self, cx: &mut Context<Self>) -> Button {
        Button::new("launch-again", "Try Again").primary().shortcut("↵").size(ButtonSize::Medium).on_click(cx.listener(|this, _, window, cx| this.begin(window, cx)))
    }

    /// The first run's card, its row for this computer saying how setting up goes.
    fn in_card(&self, cx: &mut Context<Self>) -> gpui::Div {
        let c = cx.theme().colors.clone();
        let mut card = vec![heading("Welcome to Kuvo Code", None, cx), list(vec![self.row(cx)], cx)];
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
            _ => div().flex().items_center().gap(space::S2).text_color(c.text_muted).child(spinner("launch-starting", IconSize::Nav, c.text_muted)).child("Starting…").into_any_element(),
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
        let frame = if self.card { self.in_card(cx) } else { self.quiet(cx) };
        frame
            .id("launch")
            .debug_selector(|| "launch".into())
            .track_focus(&self.focus)
            // Return tries again, where that's the way on.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "enter" && !event.keystroke.modifiers.modified() && matches!(this.stage, Stage::Stopped { .. }) {
                    this.begin(window, cx);
                }
            }))
            .into_any_element()
    }
}
