//! A bot's motion: how it moves in each of its states.
//!
//! Posture is a set of damped springs that each state retargets, never restarts; gestures are short
//! authored one-shots on top (a blink, a glance, the celebration). Everything random is seeded per
//! bot, so bots never move in step and the same bot always moves the same way. Nothing here reads a
//! clock: whoever draws it steps it by the time that passed, so frames can be rendered exactly.
//! It decides nothing about the bot: which state it's in is the box's (`bots.now.state`), and
//! offline the client's own (its computer can't be reached).
//!
//! Units: the body's radius is 1; y grows downwards; angles are radians.

use std::f32::consts::{PI, TAU};

/// What a bot is doing, as it's drawn: the box's state, or offline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    Idle,
    Working,
    Thinking,
    NeedsYou,
    Blocked,
    Done,
    Offline,
    Asleep,
}

impl State {
    pub const ALL: &'static [State] = &[
        State::Idle,
        State::Working,
        State::Thinking,
        State::NeedsYou,
        State::Blocked,
        State::Done,
        State::Offline,
        State::Asleep,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Working => "working",
            State::Thinking => "thinking",
            State::NeedsYou => "needs-you",
            State::Blocked => "blocked",
            State::Done => "done",
            State::Offline => "offline",
            State::Asleep => "asleep",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            State::Idle => "Idle",
            State::Working => "Working",
            State::Thinking => "Thinking",
            State::NeedsYou => "Needs you",
            State::Blocked => "Blocked",
            State::Done => "Done",
            State::Offline => "Offline",
            State::Asleep => "Asleep",
        }
    }
}

/// A symbol the body becomes for a while (thinking and blocked), the face returning after.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    None,
    /// Thinking: the body draws in to a small core and a moon circles it, passing behind.
    Orbit,
    /// Blocked: the body squares into a rounded octagon and the two eyes slide together into one bar.
    Bar,
}

impl Glyph {
    /// Whether the eyes become the symbol (else they shrink away while it shows).
    pub fn eyes_make_it(self) -> bool {
        self == Glyph::Bar
    }
}

/// A bot's temperament, from the face it was given (its saved `face`): the same states, moved a
/// little differently, so a bot keeps its character.
#[derive(Clone, Copy, Debug)]
pub struct Temperament {
    pub blink_every: f32,
    pub glance_every: f32,
    pub rest_open: f32,
    pub rest_squint: f32,
    pub rest_scale: f32,
    pub look_up: f32,
    /// How often, of its blinks, one eye blinks alone.
    pub one_eye: f32,
}

impl Temperament {
    pub fn of(face: &str) -> Self {
        let calm = Temperament { blink_every: 1.0, glance_every: 1.0, rest_open: 1.0, rest_squint: 0.06, rest_scale: 1.0, look_up: 0.05, one_eye: 0.15 };
        match face {
            "happy" => Temperament { rest_squint: 0.26, ..calm },
            "sleepy" => Temperament { rest_open: 0.72, blink_every: 1.35, glance_every: 1.4, ..calm },
            "wink" => Temperament { one_eye: 0.45, ..calm },
            "curious" => Temperament { glance_every: 0.65, look_up: 0.11, rest_scale: 1.04, ..calm },
            "bright" => Temperament { rest_scale: 1.12, rest_open: 1.05, blink_every: 0.85, ..calm },
            _ => calm,
        }
    }
}

/// A damped spring: `v += (-2ζω·v - ω²·(x - target))·dt`, stepped at most 1/240 s at a time.
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub x: f32,
    pub v: f32,
    pub target: f32,
    pub w: f32,
    pub z: f32,
}

impl Spring {
    pub fn new(x: f32, w: f32, z: f32) -> Self {
        Spring { x, v: 0., target: x, w, z }
    }

    fn step(&mut self, dt: f32) {
        let steps = (dt * 240.).ceil().max(1.) as usize;
        let h = dt / steps as f32;
        for _ in 0..steps {
            let a = -2. * self.z * self.w * self.v - self.w * self.w * (self.x - self.target);
            self.v += a * h;
            self.x += self.v * h;
        }
    }

    /// At rest, to within `eps` (and moving slower than one `eps` a frame).
    fn settled(&self, eps: f32) -> bool {
        (self.x - self.target).abs() < eps && self.v.abs() * (1. / 60.) < eps
    }

    fn snap(&mut self) {
        self.x = self.target;
        self.v = 0.;
    }
}

/// SplitMix64: a small, well-mixed generator, seeded per bot.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed ^ 0x6A09_E667_F3BC_C909)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    pub fn chance(&mut self, p: f32) -> bool {
        self.unit() < p
    }
}

/// A bot's seed, from its id: per bot, so its copies on screen agree and no two bots agree.
pub fn seed_of(id: &str) -> u64 {
    id.bytes().fold(0xCBF2_9CE4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01B3))
}

/// What to draw this frame.
#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub x: f32,
    pub y: f32,
    /// The lean, about the bottom of the body.
    pub lean: f32,
    /// A turn about the body's centre (the celebration's flip).
    pub spin: f32,
    /// Squash and stretch: under 1 squashes towards the floor.
    pub sy: f32,
    pub gx: f32,
    pub gy: f32,
    /// How open each eye is (1 open, 0 shut), the lids and blinks together.
    pub open: [f32; 2],
    /// The lower lids raised (a smile in the eyes).
    pub squint: f32,
    /// The upper lids slanted: over 0 the outer corners drop (worried), under 0 the inner (intent).
    pub slant: f32,
    /// The shape shut eyes make: over 0 a ∪ (asleep), under 0 a ∩ (happy).
    pub curve: f32,
    pub scale: f32,
    /// The head turned to one side (-1 to 1): the eyes slide and the far one narrows.
    pub turn: f32,
    /// Colour (1) to grey (0).
    pub color: f32,
    pub alpha: f32,
    pub glyph: Glyph,
    /// How far into the symbol the body is (0 face, 1 symbol).
    pub glyph_amt: f32,
    /// The symbol's own clock (seconds).
    pub glyph_t: f32,
    /// Done's sparks, how far along (0 none).
    pub burst: f32,
    /// How far big gestures reach at this size (1 at 64 points and up): small copies stay in
    /// their place.
    pub reach: f32,
}

impl Pose {
    pub fn rest() -> Self {
        Pose {
            x: 0.,
            y: 0.,
            lean: 0.,
            spin: 0.,
            sy: 1.,
            gx: 0.,
            gy: -0.05,
            open: [1., 1.],
            squint: 0.06,
            slant: 0.,
            curve: 0.,
            scale: 1.,
            turn: 0.,
            color: 1.,
            alpha: 1.,
            glyph: Glyph::None,
            glyph_amt: 0.,
            glyph_t: 0.,
            burst: 0.,
            reach: 1.,
        }
    }
}

/// The springs, by what they move. Eyes are stiff and quick, the body soft and slow: the eyes lead.
#[derive(Clone, Debug)]
struct Springs {
    x: Spring,
    y: Spring,
    lean: Spring,
    sy: Spring,
    gx: Spring,
    gy: Spring,
    open: Spring,
    squint: Spring,
    slant: Spring,
    curve: Spring,
    scale: Spring,
    turn: Spring,
    color: Spring,
    alpha: Spring,
    glyph: Spring,
    /// What's left of a flip cut short, turning back about the centre.
    spin: Spring,
}

impl Springs {
    fn new() -> Self {
        let rest = Pose::rest();
        Springs {
            x: Spring::new(0., 4., 1.),
            y: Spring::new(0., 5., 1.),
            lean: Spring::new(0., 4.5, 0.85),
            sy: Spring::new(1., 14., 0.65),
            gx: Spring::new(rest.gx, 16., 1.),
            gy: Spring::new(rest.gy, 16., 1.),
            open: Spring::new(1., 12., 1.),
            squint: Spring::new(rest.squint, 11., 1.),
            slant: Spring::new(0., 11., 1.),
            curve: Spring::new(0., 11., 1.),
            scale: Spring::new(1., 12., 0.7),
            turn: Spring::new(0., 7., 0.95),
            color: Spring::new(1., 3.5, 1.),
            alpha: Spring::new(1., 3.5, 1.),
            glyph: Spring::new(0., 11., 1.),
            spin: Spring::new(0., 9., 1.),
        }
    }

    fn all(&mut self) -> [&mut Spring; 16] {
        [
            &mut self.x,
            &mut self.y,
            &mut self.lean,
            &mut self.sy,
            &mut self.gx,
            &mut self.gy,
            &mut self.open,
            &mut self.squint,
            &mut self.slant,
            &mut self.curve,
            &mut self.scale,
            &mut self.turn,
            &mut self.color,
            &mut self.alpha,
            &mut self.glyph,
            &mut self.spin,
        ]
    }
}

/// A short authored gesture on top of the springs.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Gesture {
    /// Both eyes, or one (0 left, 1 right); `double` shuts them again straight after.
    Blink { eye: Option<usize>, double: bool, slow: bool },
    /// Needs you, on entry: perks up.
    Perk,
    /// Needs you, now and then: a small lift and a double blink, "still here".
    Nudge,
    /// Working, now and then: turns to look at its other screen, and back.
    Turn { side: f32 },
    /// Blocked, now and then: a small shake of the head, stuck.
    Shake,
    /// Done: the one celebration.
    Celebrate,
}

impl Gesture {
    fn length(self) -> f32 {
        match self {
            Gesture::Blink { double: true, slow, .. } => if slow { 0.75 } else { 0.48 },
            Gesture::Blink { slow, .. } => if slow { 0.42 } else { 0.24 },
            Gesture::Perk => 0.55,
            Gesture::Nudge => 0.6,
            Gesture::Turn { .. } => 1.5,
            Gesture::Shake => 0.5,
            Gesture::Celebrate => 1.25,
        }
    }
}

/// Offsets a gesture adds to the pose at time `t` into it.
#[derive(Clone, Copy, Debug, Default)]
struct Extra {
    y: f32,
    sy: f32,
    spin: f32,
    x: f32,
    lid: [f32; 2],
    turn: f32,
    burst: f32,
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    t * t * t * (t * (t * 6. - 15.) + 10.)
}

/// How shut a lid is over a blink of length `len` at `t`: down in 40%, up in the rest.
fn lid_dip(t: f32, len: f32) -> f32 {
    let down = len * 0.38;
    if t < 0. || t > len {
        0.
    } else if t < down {
        let u = t / down;
        u * u
    } else {
        let u = (t - down) / (len - down);
        1. - smooth(u)
    }
}

fn bump(t: f32, from: f32, len: f32) -> f32 {
    if t < from || t > from + len { 0. } else { (PI * (t - from) / len).sin() }
}

impl Gesture {
    fn extra(self, t: f32) -> Extra {
        let mut e = Extra::default();
        match self {
            Gesture::Blink { eye, double, slow } => {
                let one = if slow { 0.42 } else { 0.24 };
                let mut dip = lid_dip(t, one);
                if double {
                    dip = dip.max(lid_dip(t - one - 0.02, one * 0.9));
                }
                match eye {
                    None => e.lid = [dip, dip],
                    Some(i) => e.lid[i] = dip * 0.85,
                }
            }
            Gesture::Perk => {
                e.y = -0.12 * bump(t, 0.05, 0.42);
                e.sy = 0.035 * bump(t, 0.05, 0.3) - 0.025 * bump(t, 0.0, 0.08);
                let dip = lid_dip(t, 0.22);
                e.lid = [dip, dip];
            }
            Gesture::Nudge => {
                e.y = -0.07 * bump(t, 0.0, 0.34);
                let dip = lid_dip(t - 0.08, 0.2).max(lid_dip(t - 0.32, 0.2));
                e.lid = [dip, dip];
            }
            Gesture::Turn { side } => {
                // Out, hold, back: the eyes slide off to its other screen and the head follows.
                let out = smooth(t / 0.35) * (1. - smooth((t - 1.05) / 0.4));
                e.turn = side * 0.85 * out;
            }
            Gesture::Shake => {
                let k = (-4.2 * t).exp();
                e.x = 0.05 * (TAU * 4.6 * t).sin() * k;
            }
            Gesture::Celebrate => {
                // Crouch, leap with a full flip, land with a squash, one small bounce.
                let crouch = bump(t, 0.0, 0.2);
                let air = ((t - 0.12) / 0.5).clamp(0., 1.);
                let in_air = t > 0.12 && t < 0.62;
                let rebound = bump(t, 0.7, 0.24);
                e.y = 0.04 * crouch - if in_air { 0.38 * 4. * air * (1. - air) } else { 0. } - 0.06 * rebound;
                e.sy = -0.06 * crouch + if in_air { 0.05 * (1. - air) } else { 0. } - 0.05 * bump(t, 0.6, 0.14);
                e.spin = TAU * smooth((t - 0.16) / 0.42);
                // A small burst of sparks off the top as it peaks.
                let b = (t - 0.3) / 0.75;
                if b > 0. && b < 1. {
                    e.burst = b;
                }
            }
        }
        e
    }
}

/// Time is rebased past this many seconds, so f32 clocks keep their precision for good.
const REBASE: f32 = 4096.;

/// A bot's motion: its state, springs, gestures and timers. One per bot: every copy on screen
/// draws the same motion at its own size, so a bot moves as one creature wherever it shows.
#[derive(Clone, Debug)]
pub struct Motion {
    pub state: State,
    pub temper: Temperament,
    pub reduced: bool,
    rng: Rng,
    t: f32,
    since: f32,
    springs: Springs,
    gestures: Vec<(Gesture, f32)>,
    next_blink: f32,
    next_glance: f32,
    next_gesture: f32,
    /// Working's reading: where along the line the eyes are.
    read_at: f32,
    next_read: f32,
    glyph: Glyph,
    glyph_t: f32,
}

/// Whether the eyes show (not shut for the state, not hidden in a symbol).
fn eyes_show(state: State, glyph: Glyph, since: f32) -> bool {
    !matches!(state, State::Offline | State::Asleep) && !(glyph != Glyph::None && since > 0.22)
}

impl Motion {
    pub fn new(seed: u64, face: &str) -> Self {
        let mut rng = Rng::new(seed);
        let temper = Temperament::of(face);
        let first_blink = rng.range(1.0, 6.0);
        let first_glance = rng.range(3.0, 9.0);
        let mut m = Motion {
            state: State::Idle,
            temper,
            reduced: false,
            rng,
            t: 0.,
            since: 100.,
            springs: Springs::new(),
            gestures: Vec::new(),
            next_blink: first_blink,
            next_glance: first_glance,
            next_gesture: f32::INFINITY,
            read_at: 0.,
            next_read: f32::INFINITY,
            glyph: Glyph::None,
            glyph_t: 0.,
        };
        m.retarget();
        m.springs.all().into_iter().for_each(|s| s.snap());
        m
    }

    /// Starts in `state` at rest, as a bot already in it is first drawn: no entry gesture, ever, on
    /// first drawing (a list scrolled, the app relaunched, a client reconnected).
    pub fn settled_in(seed: u64, face: &str, state: State) -> Self {
        let mut m = Motion::new(seed, face);
        m.state = state;
        m.retarget();
        m.settle_entry();
        m.springs.all().into_iter().for_each(|s| s.snap());
        m.glyph = m.state_glyph();
        m.plan_state_timers();
        m
    }

    pub fn now(&self) -> f32 {
        self.t
    }

    /// How long it's been in its state, in seconds (100 for one it was first drawn in).
    pub fn since(&self) -> f32 {
        self.since
    }

    /// Whether a gesture is under way (a blink, a glance's turn, the celebration).
    pub fn gesturing(&self) -> bool {
        !self.gestures.is_empty()
    }

    /// Puts the state's entry behind it: where it would be long after the change.
    fn settle_entry(&mut self) {
        self.since = 100.;
        if self.state == State::Done {
            self.springs.open.target = 1.;
            self.springs.curve.target = 0.;
        }
        if self.state_glyph() != Glyph::None {
            self.springs.glyph.target = 1.;
        }
    }

    /// Changes state, as seen live: the acknowledging gesture, then the new posture.
    pub fn set_state(&mut self, state: State) {
        if state == self.state {
            return;
        }
        let from = self.state;
        // One-shots don't hold the new state up: what's left of them goes into the springs.
        self.cut_gestures();
        self.state = state;
        self.since = 0.;
        self.retarget();
        if self.reduced {
            self.settle_entry();
            self.springs.all().into_iter().for_each(|s| s.snap());
            self.glyph = self.state_glyph();
            return;
        }
        let blinking = self.gestures.iter().any(|(g, _)| matches!(g, Gesture::Blink { .. }));
        match state {
            State::Done => self.gestures.push((Gesture::Celebrate, self.t)),
            State::NeedsYou => self.gestures.push((Gesture::Perk, self.t)),
            State::Offline | State::Asleep => {}
            // A blink under way is the acknowledgement; one isn't started on top of it.
            _ if blinking => {}
            _ => self.gestures.push((Gesture::Blink { eye: None, double: false, slow: false }, self.t)),
        }
        if from == State::Thinking || from == State::Blocked {
            // The face comes back from the symbol a little quicker than it went.
            self.springs.glyph.w = 13.;
        }
        self.plan_state_timers();
    }

    fn plan_state_timers(&mut self) {
        let t = self.t;
        let tm = self.temper;
        self.next_blink = t + self.rng.range(1.2, 3.0) * tm.blink_every;
        self.next_glance = match self.state {
            State::Idle | State::Done | State::Thinking => t + self.rng.range(2.5, 6.0) * tm.glance_every,
            _ => f32::INFINITY,
        };
        self.next_gesture = match self.state {
            State::Working => t + self.rng.range(2.5, 5.0),
            State::NeedsYou => t + self.rng.range(4.0, 6.0),
            State::Blocked => t + self.rng.range(2.5, 4.0),
            _ => f32::INFINITY,
        };
        self.read_at = 0.;
        self.next_read = if self.state == State::Working { t + 0.6 } else { f32::INFINITY };
    }

    /// Folds what's left of the gestures under way into the springs (blinks carry on).
    fn cut_gestures(&mut self) {
        let now = self.t;
        let (blinks, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.gestures).into_iter().partition(|(g, _)| matches!(g, Gesture::Blink { .. }));
        self.gestures = blinks;
        for (g, start) in rest {
            let e = g.extra(now - start);
            self.springs.y.x += e.y;
            self.springs.sy.x += e.sy;
            self.springs.x.x += e.x;
            self.springs.turn.x += e.turn;
            // A flip under way turns back about the centre, the short way round.
            self.springs.spin.x += (e.spin + PI).rem_euclid(TAU) - PI;
        }
    }

    /// The symbol this state shows, if any.
    pub fn state_glyph(&self) -> Glyph {
        match self.state {
            State::Thinking => Glyph::Orbit,
            State::Blocked => Glyph::Bar,
            _ => Glyph::None,
        }
    }

    /// Each state's posture: what the springs head for.
    fn retarget(&mut self) {
        let tm = self.temper;
        let s = &mut self.springs;
        let gx;
        let gy;
        let (x, y, lean, sy, open, squint, slant, curve, scale, color, alpha);
        match self.state {
            State::Idle => {
                (x, y, lean, sy) = (0., 0., 0., 1.);
                (open, squint, slant, curve, scale) = (tm.rest_open, tm.rest_squint, 0., 0., tm.rest_scale);
                (gx, gy) = (s.gx.target.clamp(-0.14, 0.14), -tm.look_up);
                (color, alpha) = (1., 1.);
            }
            State::Working => {
                // Narrowed, intent eyes on its work: reads apart from idle even standing still.
                (x, y, lean, sy) = (0.03, 0.035, 0.07, 0.985);
                (open, squint, slant, curve, scale) = (0.6, 0.04, -0.12, 0., 1.0);
                (gx, gy) = (0., 0.14);
                (color, alpha) = (1., 1.);
            }
            State::Thinking => {
                (x, y, lean, sy) = (0., -0.01, -0.06, 1.0);
                (open, squint, slant, curve, scale) = (0.9, 0.12, 0.06, 0., 0.96);
                (gx, gy) = (0.15, -0.17);
                (color, alpha) = (1., 1.);
            }
            State::NeedsYou => {
                (x, y, lean, sy) = (0., -0.07, 0., 1.02);
                (open, squint, slant, curve, scale) = (1.08, 0., 0.08, 0., 1.22);
                (gx, gy) = (0., 0.02);
                (color, alpha) = (1., 1.);
            }
            State::Blocked => {
                (x, y, lean, sy) = (0., 0.05, 0., 0.97);
                (open, squint, slant, curve, scale) = (0.74, 0., 0.55, 0., 0.96);
                (gx, gy) = (0., 0.08);
                (color, alpha) = (1., 1.);
            }
            State::Done => {
                // Smiling eyes: the lower lids high, a dome over each.
                (x, y, lean, sy) = (0., 0., 0., 1.);
                (open, squint, slant, curve, scale) = (0., 0.62, 0., -1., 1.06);
                (gx, gy) = (0., -0.04);
                (color, alpha) = (1., 1.);
            }
            State::Offline => {
                (x, y, lean, sy) = (0., 0.04, 0., 0.98);
                (open, squint, slant, curve, scale) = (0., 0., 0., 0., 1.0);
                (gx, gy) = (0., 0.04);
                (color, alpha) = (0., 0.6);
            }
            State::Asleep => {
                (x, y, lean, sy) = (0., 0.05, -0.13, 0.965);
                (open, squint, slant, curve, scale) = (0., 0., 0., 0.85, 1.0);
                (gx, gy) = (-0.04, 0.07);
                (color, alpha) = (1., 1.);
            }
        }
        s.x.target = x;
        s.y.target = y;
        s.lean.target = lean;
        s.sy.target = sy;
        s.open.target = open;
        s.squint.target = squint;
        s.slant.target = slant;
        s.curve.target = curve;
        s.scale.target = scale;
        s.gx.target = gx;
        s.gy.target = gy;
        s.color.target = color;
        s.alpha.target = alpha;
        s.turn.target = 0.;
        s.spin.target = 0.;
        // Asleep's eyes drift shut; everything else moves at its usual pace.
        s.open.w = if self.state == State::Asleep { 3.2 } else { 12. };
        s.glyph.w = 11.;
        // Done's eyes start shut and happy and open into the smile (see `step`); a symbol comes in
        // once the eyes have reacted (see `step`).
    }

    /// Steps the motion on by `dt` seconds: the clocks by all of it, however long it slept.
    pub fn step(&mut self, dt: f32) {
        let dt = dt.max(0.);
        self.t += dt;
        self.since = (self.since + dt).min(1000.);
        self.glyph_t += dt;
        if self.t > REBASE {
            self.rebase();
        }
        if self.reduced {
            self.gestures.clear();
        }
        let t = self.t;
        if !self.reduced {
            self.events(t);
        }
        // Done: the eyes stay shut and happy through the celebration, then open into the smile.
        if self.state == State::Done {
            let happy = self.since < 1.3 && !self.reduced;
            self.springs.open.target = if happy { 0. } else { 1. };
            self.springs.curve.target = if happy { -1. } else { 0. };
        }
        // A symbol comes in once the eyes have reacted; from one symbol to another it goes
        // through the face.
        let wanted = self.state_glyph();
        if wanted == Glyph::None {
            self.springs.glyph.target = 0.;
        } else if self.glyph == Glyph::None || self.glyph == wanted || self.springs.glyph.x < 0.02 {
            self.glyph = wanted;
            self.springs.glyph.target = if self.since < 0.22 && !self.reduced { 0. } else { 1. };
        } else {
            self.springs.glyph.target = 0.;
        }
        // Springs: a long sleep has settled them anyway.
        let long = dt > 1.;
        for s in self.springs.all() {
            if self.reduced || long { s.snap() } else { s.step(dt) }
        }
        let now = self.t;
        self.gestures.retain(|(g, start)| now - start < g.length());
        if wanted == Glyph::None && self.springs.glyph.x < 0.002 {
            self.glyph = Glyph::None;
        }
    }

    fn rebase(&mut self) {
        self.t -= REBASE;
        for at in [&mut self.next_blink, &mut self.next_glance, &mut self.next_gesture, &mut self.next_read] {
            *at -= REBASE;
        }
        for (_, start) in self.gestures.iter_mut() {
            *start -= REBASE;
        }
        if self.glyph == Glyph::None {
            self.glyph_t = 0.;
        }
    }

    fn events(&mut self, t: f32) {
        let tm = self.temper;
        let blinking = self.gestures.iter().any(|(g, _)| matches!(g, Gesture::Blink { .. } | Gesture::Perk | Gesture::Nudge | Gesture::Celebrate));
        let eyes_open = eyes_show(self.state, self.state_glyph(), self.since);
        if t >= self.next_blink {
            let (lo, hi) = match self.state {
                State::Idle | State::Done => (5.0, 12.0),
                State::Working => (2.8, 5.5),
                State::Thinking => (3.0, 6.0),
                State::NeedsYou => (4.0, 7.0),
                State::Blocked => (5.0, 8.0),
                _ => (8.0, 12.0),
            };
            self.next_blink = t + self.rng.range(lo, hi) * tm.blink_every;
            if eyes_open && !blinking {
                let quiet = matches!(self.state, State::Idle | State::Done);
                let eye = (quiet && self.rng.chance(tm.one_eye)).then(|| if self.rng.chance(0.5) { 0 } else { 1 });
                let double = eye.is_none() && self.rng.chance(0.12);
                let slow = self.state == State::Blocked;
                self.gestures.push((Gesture::Blink { eye, double, slow }, t));
            }
        }
        if t >= self.next_glance {
            self.next_glance = match self.state {
                State::Idle | State::Done => t + self.rng.range(8.0, 16.0) * tm.glance_every,
                State::Thinking => t + self.rng.range(2.2, 4.0) * tm.glance_every,
                _ => f32::INFINITY,
            };
            let s = &mut self.springs;
            match self.state {
                State::Idle | State::Done => {
                    // Looks up and to one side, across, or back to you; never where it already is.
                    let at_home = s.gx.target.abs() <= 0.02;
                    let side = if s.gx.target > 0.02 { -1. } else if s.gx.target < -0.02 || self.rng.chance(0.5) { 1. } else { -1. };
                    let home = !at_home && self.rng.chance(0.3);
                    s.gx.target = if home { 0. } else { side * self.rng.range(0.08, 0.15) };
                    s.gy.target = -tm.look_up - if home { 0. } else { self.rng.range(-0.02, 0.07) };
                    // The head turns a touch with the eyes, on its slower springs.
                    s.lean.target = s.gx.target * 0.12;
                    s.x.target = s.gx.target * 0.08;
                }
                State::Thinking => {
                    s.gx.target = if s.gx.target > 0. { -self.rng.range(0.08, 0.16) } else { self.rng.range(0.1, 0.17) };
                    s.gy.target = -self.rng.range(0.13, 0.2);
                    s.lean.target = -s.gx.target * 0.35;
                }
                _ => {}
            }
        }
        if t >= self.next_gesture {
            match self.state {
                State::Working => {
                    let side = if self.rng.chance(0.5) { 1. } else { -1. };
                    self.gestures.push((Gesture::Turn { side }, t));
                    self.next_gesture = t + self.rng.range(7.0, 12.0);
                }
                State::NeedsYou => {
                    self.gestures.push((Gesture::Nudge, t));
                    self.next_gesture = t + self.rng.range(6.0, 9.0);
                }
                State::Blocked => {
                    self.gestures.push((Gesture::Shake, t));
                    self.next_gesture = t + self.rng.range(3.5, 5.5);
                }
                _ => self.next_gesture = f32::INFINITY,
            }
        }
        // Working reads: the eyes step along a line, pause, and sweep back to start the next, the
        // body settling a little lower as they do.
        if t >= self.next_read && self.state == State::Working {
            let s = &mut self.springs;
            if self.read_at >= 3. {
                self.read_at = 0.;
                s.gx.target = -0.13;
                s.gy.target = 0.14;
                s.y.target = 0.05;
                self.next_read = t + self.rng.range(1.2, 2.4);
            } else {
                self.read_at += 1.;
                s.gx.target = -0.13 + 0.09 * self.read_at;
                s.gy.target = 0.14 + 0.012 * self.read_at;
                s.y.target = 0.035;
                self.next_read = t + self.rng.range(0.6, 1.1);
            }
        }
    }

    /// The pose to draw now, at `size` points: big hops and sparks are scaled down where they'd
    /// leave a small copy's place, and asleep breathes only where it's big enough to see.
    pub fn pose(&self, size: f32) -> Pose {
        let s = &self.springs;
        let mut e = Extra::default();
        for (g, start) in &self.gestures {
            let x = g.extra(self.t - start);
            e.y += x.y;
            e.sy += x.sy;
            e.spin += x.spin;
            e.x += x.x;
            e.turn += x.turn;
            e.burst = e.burst.max(x.burst);
            e.lid[0] = e.lid[0].max(x.lid[0]);
            e.lid[1] = e.lid[1].max(x.lid[1]);
        }
        let reach = (size / 64.).clamp(0.45, 1.);
        let breath = if self.breathes(size) { 0.018 * (TAU * self.t / 4.6).sin() } else { 0. };
        let open = s.open.x.max(0.);
        Pose {
            x: s.x.x + e.x,
            y: s.y.x + e.y * reach - breath * 0.5,
            lean: s.lean.x,
            spin: e.spin + s.spin.x,
            sy: s.sy.x + e.sy + breath,
            gx: s.gx.x,
            gy: s.gy.x,
            open: [open * (1. - e.lid[0]), open * (1. - e.lid[1])],
            squint: s.squint.x,
            slant: s.slant.x,
            curve: s.curve.x,
            scale: s.scale.x,
            turn: s.turn.x + e.turn,
            color: s.color.x.clamp(0., 1.),
            alpha: s.alpha.x.clamp(0., 1.),
            glyph: self.glyph,
            glyph_amt: s.glyph.x.clamp(0., 1.05),
            glyph_t: self.glyph_t,
            burst: e.burst,
            reach,
        }
    }

    fn breathes(&self, size: f32) -> bool {
        self.state == State::Asleep && !self.reduced && size >= 48.
    }

    /// When a copy drawn at `size` points next needs a frame, in seconds: 0 for the next one (a
    /// spring moving more than a quarter point there, a gesture under way), a short wait for a
    /// loop that moves slowly at this size (the orbit at 30 fps when small, the breath at 4), or
    /// the time to its next blink, glance or gesture. Infinite when nothing will ever move.
    pub fn next_frame_in(&self, size: f32) -> f32 {
        if self.reduced {
            return f32::INFINITY;
        }
        let eps = 0.25 / (size * 0.42);
        let mut springs = self.springs.clone();
        if !self.gestures.is_empty() || springs.all().into_iter().any(|s| !s.settled(eps)) {
            return 0.;
        }
        let looping = self.glyph != Glyph::None && self.springs.glyph.x > 0.01 && self.glyph == Glyph::Orbit;
        let mut wait = self.next_event_in();
        if looping {
            wait = wait.min(if size < 40. { 1. / 30. } else { 0. });
        }
        if self.breathes(size) {
            wait = wait.min(0.25);
        }
        wait
    }

    /// Seconds until the next timed event (a blink, a glance, a gesture, a step of reading).
    pub fn next_event_in(&self) -> f32 {
        if self.reduced {
            return f32::INFINITY;
        }
        let blink = if eyes_show(self.state, self.state_glyph(), self.since) { self.next_blink } else { f32::INFINITY };
        (blink.min(self.next_glance).min(self.next_gesture).min(self.next_read) - self.t).max(0.)
    }
}

#[cfg(test)]
mod tests {
    use super::{Glyph, Motion, State, seed_of};

    fn settled(state: State) -> Motion {
        Motion::settled_in(seed_of("bot_ada"), "calm", state)
    }

    /// Each state has a still pose of its own that reads without motion: what a bot shows with
    /// Reduce Motion on, and between its gestures.
    #[test]
    fn every_state_has_a_still_pose_of_its_own() {
        let pose = |state| settled(state).pose(28.);
        let (idle, working) = (pose(State::Idle), pose(State::Working));
        assert!(working.open[0] < idle.open[0] - 0.3 && working.gy > idle.gy + 0.1, "working: narrowed eyes, down at its work");
        assert!(working.lean > 0.05, "working leans in");
        assert_eq!((pose(State::Thinking).glyph, pose(State::Thinking).glyph_amt), (Glyph::Orbit, 1.), "thinking: the orbit");
        assert_eq!((pose(State::Blocked).glyph, pose(State::Blocked).glyph_amt), (Glyph::Bar, 1.), "blocked: the bar");
        assert!(pose(State::NeedsYou).scale > 1.15, "needs you: eyes bigger, at you");
        assert!(pose(State::Done).squint > 0.5 && pose(State::Done).open[0] > 0.9, "done: smiling eyes, settled");
        assert!(pose(State::Offline).color < 0.01 && pose(State::Offline).alpha < 0.7 && pose(State::Offline).open[0] < 0.01, "offline: grey, faded, eyes shut");
        assert!(pose(State::Asleep).open[0] < 0.01 && pose(State::Asleep).curve > 0.5, "asleep: eyes shut as ∪");
    }

    /// A bot first drawn in a state starts settled in it: no entry gesture on mount, ever. One that
    /// changes while it's drawn plays the change once: Done's celebration, a blink for the others.
    #[test]
    fn a_change_plays_once_and_a_first_drawing_plays_nothing() {
        let done = settled(State::Done);
        assert!(!done.gesturing() && done.next_frame_in(28.) > 0., "drawn in Done: already smiling, nothing playing");
        let mut seen = settled(State::Working);
        seen.set_state(State::Done);
        assert!(seen.gesturing() && seen.next_frame_in(28.) == 0., "seen finishing: the celebration plays");
        seen.step(2.);
        seen.step(0.);
        assert!(!seen.gesturing(), "and plays once: done after it");
        assert!(seen.next_frame_in(28.) > 1., "then it rests till its next blink or glance");
    }

    /// At rest, an idle bot needs no frame until its next one-shot, seconds away.
    #[test]
    fn an_idle_bot_needs_no_frames_between_its_one_shots() {
        let mut idle = settled(State::Idle);
        let mut frames = 0;
        let mut t = 0.;
        while t < 60. {
            let wait = idle.next_frame_in(28.);
            let dt = if wait == 0. { 1. / 60. } else { wait.max(1. / 60.) };
            if wait == 0. {
                frames += 1;
            }
            idle.step(dt);
            t += dt;
        }
        assert!(frames > 0, "it blinks and glances");
        assert!(frames < 60 * 6, "frames only during one-shots: {frames} in a minute");
    }

    /// With Reduce Motion on: the still pose, no gesture, nothing scheduled; a change swaps poses.
    #[test]
    fn reduced_motion_holds_still() {
        let mut motion = settled(State::Idle);
        motion.reduced = true;
        assert!(motion.next_frame_in(28.).is_infinite());
        motion.set_state(State::Thinking);
        motion.step(0.);
        assert!(!motion.gesturing() && motion.next_frame_in(28.).is_infinite());
        assert_eq!(motion.pose(28.).glyph_amt, 1., "the symbol at once");
    }

    /// A one-shot never holds up the new state: Done's celebration cut by a change shows the new
    /// state's posture at once.
    #[test]
    fn a_new_state_never_waits_for_a_one_shot() {
        let mut motion = settled(State::Working);
        motion.set_state(State::Done);
        motion.step(0.3);
        motion.set_state(State::NeedsYou);
        motion.step(0.6);
        let pose = motion.pose(28.);
        assert!(pose.scale > 1.1 && pose.spin.abs() < 0.3, "needs you's eyes, the flip turned back");
    }

    /// Bots never move in step: their seeds differ, and so do their next one-shots.
    #[test]
    fn two_bots_never_blink_together() {
        let (a, b) = (Motion::settled_in(seed_of("bot_a"), "calm", State::Idle), Motion::settled_in(seed_of("bot_b"), "calm", State::Idle));
        assert_ne!(a.next_event_in(), b.next_event_in());
    }
}
