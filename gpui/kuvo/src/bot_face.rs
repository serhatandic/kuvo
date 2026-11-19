//! Bots drawn live, wherever they appear: a sidebar row, a conversation's header, a group chat, the
//! Work Queue, the bot's page and its look picker.
//!
//! Each bot is one creature: one [`Motion`], kept here by the bot's id, that every copy on screen
//! steps to the present and draws at its own size. So copies of one bot move together, even one
//! drawn later, and no two bots agree (the motion is seeded by the id).
//!
//! What a bot is doing is the box's (`bots.now.state`); a client only adds offline, when the bot's
//! computer can't be reached. A state the box says is handed here ([`tell`]) as it's heard: a
//! creature that's on screen then plays the change; one first drawn later starts settled in it,
//! so nothing replays as a list scrolls, the app starts or a client reconnects.
//!
//! Frames: a copy asks for the next one only while something moves by a quarter point or more at
//! its size, and at most 60 a second (on a 120 Hz display too). Otherwise it waits on one timer
//! per view for its next blink, glance or gesture, and an idle bot schedules nothing in between.
//! With macOS's Reduce Motion on, each state shows its still pose and nothing is scheduled.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AnyElement, App, EntityId, Global, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels, SharedString, StatefulInteractiveElement, Styled, Task, Window, canvas, div, px};
use kuvo_box::areas::{Bot, BotNow, BotState, Look};
use kuvo_ui::{ActiveTheme, Icon, IconName, IconSize, StyledType, TextRole, menu::Tooltip, theme::{Colors, radius}};
use kuvo_ui::bot::{Motion, Shape, State, paint_bot, seed_of};

/// The shortest time between two frames a view asks for: 60 a second, with a little slack so a
/// 60 Hz display never skips one.
const FRAME: Duration = Duration::from_micros(14_000);

/// Done shows at least through its celebration, even when it's seen at once (you were watching):
/// a dwell on the drawing, not a rule about the state.
const DONE_DWELL: f32 = 1.3;

/// How a bot is drawn for what the box says of it.
pub fn state_of(now: &BotNow) -> State {
    match now.state {
        BotState::Idle => State::Idle,
        BotState::Working => State::Working,
        BotState::Thinking => State::Thinking,
        BotState::NeedsYou => State::NeedsYou,
        BotState::Blocked => State::Blocked,
        BotState::Done => State::Done,
        BotState::Asleep => State::Asleep,
    }
}

/// The status mark shared by the full Bots sidebar (trailing column) and its compact avatar strip
/// (dedicated gutter under the face). Nothing is ever drawn over a bot's face.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SidebarMark {
    NeedsYou,
    AtWork,
    Wake { short: String, tooltip: String },
    Blocked,
    Offline,
    ChangeWaiting(String),
    ChangeRefused(String),
}

fn terse_wake(due_ms: f64, now_ms: f64) -> Option<String> {
    let ahead = due_ms - now_ms;
    if !ahead.is_finite() || ahead <= 0. {
        return None;
    }
    let minutes = (ahead / 60_000.).ceil() as u64;
    if minutes < 60 {
        Some(format!("{}m", minutes.max(1)))
    } else if minutes < 24 * 60 {
        Some(format!("{}h", minutes / 60))
    } else {
        Some(format!("{}d", minutes / (24 * 60)))
    }
}

fn hollow_dot(size: Pixels, color: Hsla) -> AnyElement {
    div()
        .flex_none()
        .size(size)
        .rounded(radius::PILL)
        .border_1()
        .border_color(color)
        .into_any_element()
}

fn halo_dot(color: Hsla, inner: Pixels, outer: Pixels) -> AnyElement {
    div()
        .flex_none()
        .size(outer)
        .rounded(radius::PILL)
        .bg(color.opacity(0.18))
        .flex()
        .items_center()
        .justify_center()
        .child(div().size(inner).rounded(radius::PILL).bg(color))
        .into_any_element()
}

impl SidebarMark {
    pub(crate) fn of(bot: &Bot, reached: bool, now: f64) -> Option<Self> {
        if !reached {
            return Some(Self::Offline);
        }
        if bot.now.asking > 0 || bot.now.state == BotState::NeedsYou {
            return Some(Self::NeedsYou);
        }
        if bot.now.state == BotState::Blocked {
            return Some(Self::Blocked);
        }
        if bot.now.working > 0 || matches!(bot.now.state, BotState::Working | BotState::Thinking) {
            return Some(Self::AtWork);
        }
        if bot.now.waking || bot.paused {
            return None;
        }
        let due = bot.wakes? * 1000.;
        let when = crate::time::next_wake(due, now)?;
        let short = terse_wake(due, now)?;
        Some(Self::Wake { short, tooltip: format!("Wakes {when}") })
    }

    pub(crate) fn tooltip(&self, name: &str) -> String {
        let words = match self {
            Self::NeedsYou => "Waiting on you",
            Self::AtWork => "At work",
            Self::Wake { tooltip, .. } => tooltip.as_str(),
            Self::Blocked => "Blocked",
            Self::Offline => "Offline",
            Self::ChangeWaiting(words) | Self::ChangeRefused(words) => words.as_str(),
        };
        format!("{name} · {words}")
    }

    pub(crate) fn detail_color(&self, colors: &Colors) -> Hsla {
        match self {
            Self::Blocked | Self::ChangeRefused(_) => colors.warning,
            Self::Offline => colors.text_faint,
            _ => colors.text_muted,
        }
    }

    /// Small status mark aligned in the trailing column on the right of a full sidebar row.
    pub(crate) fn trailing(&self, colors: &Colors) -> AnyElement {
        match self {
            Self::NeedsYou => div()
                .debug_selector(|| "bot-avatar-needs-you".into())
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .w(px(16.))
                .child(div().size(px(8.)).rounded(radius::PILL).bg(colors.accent))
                .into_any_element(),
            Self::AtWork => div()
                .debug_selector(|| "bot-avatar-at-work".into())
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .w(px(16.))
                .child(halo_dot(colors.success, px(7.), px(14.)))
                .into_any_element(),
            Self::Wake { short, .. } => div()
                .debug_selector(|| "bot-avatar-wake".into())
                .flex_none()
                .flex()
                .items_center()
                .gap(px(3.))
                .text_role(TextRole::Meta)
                .text_color(colors.text_faint)
                .child(Icon::new(IconName::Clock).size(IconSize::XSmall).color(colors.icon))
                .child(short.clone())
                .into_any_element(),
            Self::Blocked => div()
                .debug_selector(|| "bot-avatar-blocked".into())
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .w(px(16.))
                .child(Icon::new(IconName::Warning).size(IconSize::Nav).color(colors.warning))
                .into_any_element(),
            Self::Offline => div()
                .debug_selector(|| "bot-avatar-offline".into())
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .w(px(16.))
                .child(hollow_dot(px(8.), colors.text_faint))
                .into_any_element(),
            Self::ChangeWaiting(_) => div()
                .debug_selector(|| "bot-avatar-change-waiting".into())
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .w(px(16.))
                .child(Icon::new(IconName::Clock).size(IconSize::XSmall).color(colors.icon))
                .into_any_element(),
            Self::ChangeRefused(_) => div()
                .debug_selector(|| "bot-avatar-change-refused".into())
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .w(px(16.))
                .child(Icon::new(IconName::Warning).size(IconSize::Nav).color(colors.warning))
                .into_any_element(),
        }
    }

    /// Small status mark in the dedicated gutter strictly below the face in the compact strip.
    pub(crate) fn gutter(&self, colors: &Colors) -> AnyElement {
        match self {
            Self::NeedsYou => div()
                .debug_selector(|| "bot-avatar-needs-you".into())
                .flex_none()
                .size(px(6.))
                .rounded(radius::PILL)
                .bg(colors.accent)
                .into_any_element(),
            Self::AtWork => div()
                .debug_selector(|| "bot-avatar-at-work".into())
                .flex_none()
                .child(halo_dot(colors.success, px(5.), px(10.)))
                .into_any_element(),
            Self::Wake { .. } => div()
                .debug_selector(|| "bot-avatar-wake".into())
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(IconName::Clock).size(IconSize::Indicator).color(colors.icon))
                .into_any_element(),
            Self::Blocked => div()
                .debug_selector(|| "bot-avatar-blocked".into())
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(IconName::Warning).size(IconSize::Indicator).color(colors.warning))
                .into_any_element(),
            Self::Offline => div()
                .debug_selector(|| "bot-avatar-offline".into())
                .flex_none()
                .child(hollow_dot(px(6.), colors.text_faint))
                .into_any_element(),
            Self::ChangeWaiting(_) => div()
                .debug_selector(|| "bot-avatar-change-waiting".into())
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(IconName::Clock).size(IconSize::Indicator).color(colors.icon))
                .into_any_element(),
            Self::ChangeRefused(_) => div()
                .debug_selector(|| "bot-avatar-change-refused".into())
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(IconName::Warning).size(IconSize::Indicator).color(colors.warning))
                .into_any_element(),
        }
    }
}

/// Untouched bot face for a Bots sidebar row, with tooltip and selector (never overlays a badge).
pub(crate) fn sidebar_avatar(id: &str, name: &str, face: AnyElement, mark: Option<&SidebarMark>) -> AnyElement {
    let tooltip = mark.map_or_else(|| name.to_owned(), |mark| mark.tooltip(name));
    let selector = format!("bot-{id}-avatar");
    div().id(SharedString::from(selector.clone())).debug_selector(move || selector)
        .flex_none()
        .tooltip(move |_, cx| Tooltip::new(tooltip.clone()).build(cx))
        .child(face).into_any_element()
}

/// One bot's creature: its motion, when it was last stepped, and a state waiting for Done's
/// celebration to finish.
struct Creature {
    motion: Motion,
    /// The saved face it moves with (its temperament).
    face: String,
    last: Option<Instant>,
    pending: Option<State>,
    /// The views that drew it lately, told when its state changes.
    viewers: HashSet<EntityId>,
}

impl Creature {
    fn new(id: &str, face: &str, state: State) -> Self {
        Creature { motion: Motion::settled_in(seed_of(id), face, state), face: face.to_owned(), last: None, pending: None, viewers: HashSet::new() }
    }

    /// Stepped to `now`, by all the time that passed (a long sleep settles it).
    fn catch_up(&mut self, now: Instant, reduced: bool) {
        let dt = self.last.map_or(0., |last| now.saturating_duration_since(last).as_secs_f32());
        self.last = Some(now);
        self.motion.reduced = reduced;
        self.motion.step(dt);
        if self.pending.is_some() && (reduced || self.motion.since() >= DONE_DWELL) {
            let state = self.pending.take().unwrap_or(State::Idle);
            // Drawn as the celebration ended: the change plays. Not drawn then (no copy was on
            // screen), it's settled into, so nothing replays when one next is.
            if self.motion.since() - DONE_DWELL < 0.25 { self.motion.set_state(state) } else { self.motion.settle(state) }
        }
    }

    fn apply(&mut self, state: State) {
        // Back from offline (a reconnect, a computer reached again): settled in its state, never a
        // replay of what it did meanwhile.
        if self.motion.state == State::Offline && state != State::Offline {
            self.pending = None;
            self.motion.settle(state);
            return;
        }
        if self.motion.state == State::Done && state != State::Done && !self.motion.reduced && self.motion.since() < DONE_DWELL {
            self.pending = Some(state);
            return;
        }
        self.pending = None;
        self.motion.set_state(state);
    }

    /// When a copy at `size` next needs a frame, in seconds (0 now, infinite never).
    fn wait(&self, size: f32) -> f32 {
        let wait = self.motion.next_frame_in(size);
        match self.pending {
            Some(_) => wait.min((DONE_DWELL - self.motion.since()).max(0.)),
            None => wait,
        }
    }

    /// The state it's drawn in (Done, while a state waits on its celebration).
    fn shown(&self) -> State {
        self.motion.state
    }
}

/// Every bot's creature, the states told, and each view's frame and timer: kept as a global,
/// reached through [`faces`] (a shared cell, so drawing a face notifies no global's observers).
#[derive(Default)]
pub struct Faces {
    creatures: HashMap<String, Rc<RefCell<Creature>>>,
    told: HashMap<String, State>,
    /// When each view was last asked to draw for its bots, and those asked for a frame now.
    drawn: HashMap<EntityId, Instant>,
    asked: HashSet<EntityId>,
    /// One timer per view, for its copies' next event: when it's due, and the timer.
    timers: HashMap<EntityId, (Instant, Task<()>)>,
    /// What timers are lined up on, so copies in different views wake on the same frame.
    epoch: Option<Instant>,
    /// Frames drawn for bots and timers that woke, counted for the cost's measure and the tests.
    pub frames: u64,
    pub wakes: u64,
}

struct BotFaces(Rc<RefCell<Faces>>);

impl Global for BotFaces {}

/// The app's bots' faces.
pub fn faces(cx: &mut App) -> Rc<RefCell<Faces>> {
    if !cx.has_global::<BotFaces>() {
        cx.set_global(BotFaces(Rc::default()));
    }
    cx.global::<BotFaces>().0.clone()
}

impl Faces {
    /// The state a bot is drawn in now: the one told, held through Done's celebration.
    pub fn shown(&self, id: &str) -> Option<State> {
        match self.creatures.get(id) {
            Some(creature) => Some(creature.borrow().shown()),
            None => self.told.get(id).copied(),
        }
    }

    /// Whether any view waits on a frame or a timer for its bots.
    pub fn scheduled(&self) -> bool {
        !self.asked.is_empty() || !self.timers.is_empty()
    }
}

/// What a bot is doing now, told as it's heard: a creature on screen plays the change at once (its
/// views are asked to draw), one not drawn yet will start in it.
pub fn tell(id: &str, state: State, cx: &mut App) {
    let now = cx.background_executor().now();
    let reduced = cx.reduce_motion();
    let creature = {
        let faces = faces(cx);
        let mut faces = faces.borrow_mut();
        if faces.told.get(id) == Some(&state) {
            return;
        }
        faces.told.insert(id.to_owned(), state);
        faces.creatures.get(id).cloned()
    };
    let Some(creature) = creature else { return };
    let viewers: Vec<EntityId> = {
        let mut creature = creature.borrow_mut();
        creature.catch_up(now, reduced);
        creature.apply(state);
        creature.viewers.drain().collect()
    };
    for view in viewers {
        cx.notify(view);
    }
}

/// The pose bot `id`'s creature is in as its copies last drew it, at `size` points (none before
/// it's drawn): for the tests and the cost's measure, which look at what's drawn.
pub fn pose_of(id: &str, size: f32, cx: &mut App) -> Option<kuvo_ui::bot::Pose> {
    let creature = faces(cx).borrow().creatures.get(id).cloned()?;
    let pose = creature.borrow().motion.pose(size);
    Some(pose)
}

/// A bot's face, live, `size` points square: the creature of the bot `id` (or of any key a picker
/// gives its options) in `look`.
pub fn bot_face(id: &str, look: &Look, size: Pixels) -> AnyElement {
    let (id, look) = (id.to_owned(), look.clone());
    let side = f32::from(size);
    div()
        .flex_none()
        .size(size)
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    let now = cx.background_executor().now();
                    let reduced = cx.reduce_motion();
                    let dark = cx.theme().appearance.is_dark();
                    let view = window.current_view();
                    let creature = {
                        let faces = faces(cx);
                        let mut faces = faces.borrow_mut();
                        let told = faces.told.get(&id).copied().unwrap_or(State::Idle);
                        faces.creatures.entry(id.clone()).or_insert_with(|| Rc::new(RefCell::new(Creature::new(&id, &look.face, told)))).clone()
                    };
                    let (pose, wait) = {
                        let mut creature = creature.borrow_mut();
                        if creature.last != Some(now) {
                            creature.catch_up(now, reduced);
                        }
                        if creature.face != look.face {
                            creature.face = look.face.clone();
                            creature.motion.set_face(&look.face);
                        }
                        creature.viewers.insert(view);
                        (creature.motion.pose(side), creature.wait(side))
                    };
                    paint_bot(bounds, Shape::of(&look.shape), &look.color, &pose, dark, window);
                    if wait == 0. {
                        ask_frame(view, window, cx);
                    } else if wait.is_finite() {
                        wake_at(view, aligned(now, wait, cx), cx);
                    }
                },
            )
            .size(size),
        )
        .into_any_element()
}

/// A still idle pose for a move row and its header.
pub fn still_bot_face(id: &str, look: &Look, size: Pixels) -> AnyElement {
    still_avatar(id, look, State::Idle, size)
}

/// A settled avatar for the compact sidebar, with no frame or motion scheduling.
pub fn still_avatar(id: &str, look: &Look, state: State, size: Pixels) -> AnyElement {
    let pose = Motion::settled_in(seed_of(id), &look.face, state).pose(f32::from(size));
    let shape = Shape::of(&look.shape);
    let color = look.color.clone();
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            paint_bot(bounds, shape, &color, &pose, cx.theme().appearance.is_dark(), window);
        },
    )
    .size(size)
    .flex_none()
    .into_any_element()
}

/// The next frame for `view`'s bots, at most 60 a second: on a faster display, frames come and go
/// without drawing until a 60th of a second has passed.
fn ask_frame(view: EntityId, window: &mut Window, cx: &mut App) {
    if !faces(cx).borrow_mut().asked.insert(view) {
        return;
    }
    window.on_next_frame(move |window, cx| next_frame(view, window, cx));
}

fn next_frame(view: EntityId, window: &mut Window, cx: &mut App) {
    let now = cx.background_executor().now();
    {
        let faces = faces(cx);
        let mut faces = faces.borrow_mut();
        if faces.drawn.get(&view).is_some_and(|drawn| now.saturating_duration_since(*drawn) < FRAME) {
            drop(faces);
            window.on_next_frame(move |window, cx| next_frame(view, window, cx));
            return;
        }
        faces.asked.remove(&view);
        faces.drawn.insert(view, now);
        faces.frames += 1;
    }
    cx.notify(view);
}

/// When to wake for something `wait` seconds from `now`, lined up on a shared grid of frames (a
/// 30th of a second for a slow loop's steps, a 60th otherwise), so copies of bots in different
/// views wake together and draw one frame.
fn aligned(now: Instant, wait: f32, cx: &mut App) -> Instant {
    let epoch = *faces(cx).borrow_mut().epoch.get_or_insert(now);
    let grid = if wait < 0.1 { 1. / 30. } else { 1. / 60. };
    let at = now.saturating_duration_since(epoch).as_secs_f64() + wait as f64;
    epoch + Duration::from_secs_f64((at / grid).ceil() * grid)
}

/// `view` drawn again at `due` (one timer per view: the earliest its bots need).
fn wake_at(view: EntityId, due: Instant, cx: &mut App) {
    if faces(cx).borrow().timers.get(&view).is_some_and(|(at, _)| *at <= due) {
        return;
    }
    let wait = due.saturating_duration_since(cx.background_executor().now());
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(wait).await;
        cx.update(|cx| {
            let ours = {
                let faces = faces(cx);
                let mut faces = faces.borrow_mut();
                let ours = faces.timers.get(&view).is_some_and(|(at, _)| *at == due);
                if ours {
                    // (Dropped after the borrow: dropping a task while it runs is fine.)
                    let timer = faces.timers.remove(&view);
                    faces.wakes += 1;
                    drop(faces);
                    drop(timer);
                }
                ours
            };
            if ours {
                cx.notify(view);
            }
        });
    });
    faces(cx).borrow_mut().timers.insert(view, (due, task));
}

#[cfg(test)]
mod tests {
    use super::{Bot, SidebarMark, state_of};
    use kuvo_box::areas::BotNow;
    use kuvo_ui::bot::State;

    /// The state drawn is the one the box says; a newer box's word reads as idle.
    #[test]
    fn a_bot_is_drawn_in_the_state_its_box_says() {
        let now = |json: &str| serde_json::from_str::<BotNow>(json).unwrap();
        assert_eq!(state_of(&now(r#"{"working":1,"asking":0,"thinking":1,"state":"thinking","since":1}"#)), State::Thinking);
        assert_eq!(state_of(&now(r#"{"working":0,"asking":1,"state":"needsYou","since":1}"#)), State::NeedsYou);
        assert_eq!(state_of(&now(r#"{"working":0,"asking":0,"blocked":1,"blockedBy":"limit","state":"blocked","since":1}"#)), State::Blocked);
        assert_eq!(state_of(&now(r#"{"working":0,"asking":0,"state":"dreaming","since":1}"#)), State::Idle);
        assert_eq!(state_of(&now(r#"{"working":2,"asking":0}"#)), State::Idle, "no state said: idle");
    }

    #[test]
    fn wake_badge_uses_next_time_only_when_the_bot_is_available() {
        let mut bot: Bot = serde_json::from_value(serde_json::json!({
            "id": "bot_mira", "session": "ses_mira", "name": "Mira",
            "look": {"color":"sky","shape":"round","face":"calm"}, "wakes": 180,
        })).unwrap();
        let wake = SidebarMark::of(&bot, true, 0.).unwrap();
        assert_eq!(wake, SidebarMark::Wake { short: "3m".into(), tooltip: "Wakes in 3 min".into() });
        assert_eq!(wake.tooltip(&bot.name), "Mira · Wakes in 3 min");
        assert_eq!(SidebarMark::of(&bot, false, 0.), Some(SidebarMark::Offline));
        assert_eq!(SidebarMark::of(&bot, true, 180_000.), None);
        bot.paused = true;
        assert_eq!(SidebarMark::of(&bot, true, 0.), None);
        bot.paused = false;
        bot.now.state = kuvo_box::areas::BotState::Blocked;
        assert_eq!(SidebarMark::of(&bot, true, 0.), Some(SidebarMark::Blocked));
        bot.now.state = kuvo_box::areas::BotState::Idle;
        bot.now.waking = true;
        bot.now.working = 1;
        assert_eq!(SidebarMark::of(&bot, true, 0.), Some(SidebarMark::AtWork));
        bot.now.asking = 1;
        assert_eq!(SidebarMark::of(&bot, true, 0.), Some(SidebarMark::NeedsYou));
        bot.now.working = 0;
        bot.now.asking = 0;
        assert_eq!(SidebarMark::of(&bot, true, 0.), None);
    }
}
