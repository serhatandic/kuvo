//! The bots' look (Pebble): soft solid bodies in one even family of colours, with ink eyes in a
//! deep shade of each bot's own colour. The bodies and colours go by the box's names
//! (`kuvo/bots/templates`' `looks`); one a newer box names that this doesn't know is drawn round
//! and graphite.

use std::f32::consts::{PI, TAU};
use std::sync::OnceLock;

use gpui::{Hsla, Rgba};

/// The bodies (the box's `shapes`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shape {
    Round,
    Squircle,
    Pebble,
    Gem,
    Bloom,
    Loaf,
    Capsule,
    Dome,
}

impl Shape {
    pub const ALL: [Shape; 8] = [Shape::Round, Shape::Squircle, Shape::Pebble, Shape::Gem, Shape::Bloom, Shape::Loaf, Shape::Capsule, Shape::Dome];

    /// The body the box names: round for one this doesn't know.
    pub fn of(name: &str) -> Shape {
        Shape::ALL.into_iter().find(|shape| shape.name() == name).unwrap_or(Shape::Round)
    }

    pub fn name(self) -> &'static str {
        match self {
            Shape::Round => "round",
            Shape::Squircle => "squircle",
            Shape::Pebble => "pebble",
            Shape::Gem => "gem",
            Shape::Bloom => "bloom",
            Shape::Loaf => "loaf",
            Shape::Capsule => "capsule",
            Shape::Dome => "dome",
        }
    }
}

/// How many points every outline has, so any outline morphs into any other point by point.
pub const OUTLINE: usize = 120;

fn superellipse(t: f32, n: f32, a: f32, b: f32) -> (f32, f32) {
    let (s, c) = t.sin_cos();
    let e = 2. / n;
    (a * c.signum() * c.abs().powf(e), b * s.signum() * s.abs().powf(e))
}

/// A radial outline: r(θ), θ from the top, clockwise.
pub fn radial(r: impl Fn(f32) -> f32) -> Vec<(f32, f32)> {
    (0..OUTLINE)
        .map(|i| {
            let th = TAU * i as f32 / OUTLINE as f32;
            let r = r(th);
            (r * th.sin(), -r * th.cos())
        })
        .collect()
}

/// Smooths an outline's corners by averaging each point with its neighbours.
pub fn soften(points: Vec<(f32, f32)>, passes: usize) -> Vec<(f32, f32)> {
    let mut p = points;
    let n = p.len();
    for _ in 0..passes {
        p = (0..n)
            .map(|i| {
                let a = p[(i + n - 1) % n];
                let b = p[i];
                let c = p[(i + 1) % n];
                ((a.0 + 2. * b.0 + c.0) / 4., (a.1 + 2. * b.1 + c.1) / 4.)
            })
            .collect();
    }
    p
}

impl Shape {
    /// The body's outline, about the centre, at most 1 across in each direction (worked out once).
    /// Every body covers the same area, so none looks bigger than another in a list: a round body
    /// reads larger than a hexagon in the same square, so each is scaled to the same optical size.
    pub fn outline(self) -> &'static [(f32, f32)] {
        static OUTLINES: OnceLock<Vec<Vec<(f32, f32)>>> = OnceLock::new();
        let all = OUTLINES.get_or_init(|| Shape::ALL.into_iter().map(|shape| optical(shape.make_outline())).collect());
        &all[Shape::ALL.iter().position(|shape| *shape == self).unwrap_or(0)]
    }

    fn make_outline(self) -> Vec<(f32, f32)> {
        // Every outline starts at the top and runs clockwise, so they morph into each other.
        let param = |f: &dyn Fn(f32) -> (f32, f32)| -> Vec<(f32, f32)> { (0..OUTLINE).map(|i| f(-PI / 2. + TAU * i as f32 / OUTLINE as f32)).collect() };
        match self {
            Shape::Round => param(&|t| superellipse(t, 2., 0.97, 0.97)),
            Shape::Squircle => param(&|t| superellipse(t, 4.2, 0.92, 0.92)),
            Shape::Pebble => param(&|t| {
                let (x, y) = superellipse(t, 2.25, 0.98, 0.9);
                (x * (1. + 0.09 * y), y + 0.04)
            }),
            Shape::Gem => {
                let hex = radial(|th| {
                    let k = PI / 3.;
                    let a = (th + k / 2.).rem_euclid(k) - k / 2.;
                    0.99 * (k / 2.).cos() / a.cos()
                });
                soften(hex, 7)
            }
            Shape::Bloom => radial(|th| 0.9 * (1. + 0.065 * (6. * th).cos())),
            Shape::Loaf => param(&|t| {
                let (x, y) = superellipse(t, 3.4, 1.0, 0.8);
                (x, y + 0.1)
            }),
            Shape::Capsule => param(&|t| superellipse(t, 2.7, 0.8, 1.0)),
            Shape::Dome => param(&|t| {
                let top = t.sin() < 0.;
                let (x, y) = if top { superellipse(t, 2.3, 0.98, 1.0) } else { superellipse(t, 7., 0.98, 0.72) };
                (x, y + 0.1)
            }),
        }
    }

    /// How high the eyes sit on the body: a little above its middle. Their size and spacing are
    /// the same on every body, so bots differ by silhouette and colour, never by their eyes.
    pub fn eye_height(self) -> f32 {
        match self {
            Shape::Loaf | Shape::Dome => 0.02,
            Shape::Pebble => -0.02,
            Shape::Capsule => -0.08,
            _ => -0.05,
        }
    }
}

/// The area every body covers: a disc of radius 0.86 (in the body's own units).
const OPTICAL_AREA: f32 = PI * 0.86 * 0.86;

/// `points` scaled about the centre to cover [`OPTICAL_AREA`], never past the square they're in.
fn optical(points: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    let n = points.len();
    let area = (0..n).map(|i| {
        let (a, b) = (points[i], points[(i + 1) % n]);
        a.0 * b.1 - b.0 * a.1
    }).sum::<f32>().abs() / 2.;
    let extent = points.iter().fold(0f32, |most, (x, y)| most.max(x.abs()).max(y.abs()));
    let k = (OPTICAL_AREA / area).sqrt().min(0.99 / extent);
    points.into_iter().map(|(x, y)| (x * k, y * k)).collect()
}

/// OKLCH to sRGB, so the palette's colours are even in lightness by construction.
pub fn oklch(l: f32, c: f32, h: f32, alpha: f32) -> Hsla {
    let h = h.to_radians();
    let (a, b) = (c * h.cos(), c * h.sin());
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    let r = 4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_94 * s3;
    let g = -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_38 * s3;
    let bl = -0.004_196_086_3 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3;
    let enc = |x: f32| {
        let x = x.clamp(0., 1.);
        if x <= 0.003_130_8 { 12.92 * x } else { 1.055 * x.powf(1. / 2.4) - 0.055 }
    };
    Rgba { r: enc(r), g: enc(g), b: enc(bl), a: alpha }.into()
}

/// The colours' hues, by the box's names (graphite, and one this doesn't know, have none).
pub fn hue(name: &str) -> Option<f32> {
    Some(match name {
        "mint" => 172.,
        "sky" => 238.,
        "indigo" => 272.,
        "violet" => 300.,
        "pink" => 350.,
        "coral" => 28.,
        "orange" => 58.,
        "amber" => 86.,
        "lime" => 128.,
        _ => return None,
    })
}

/// What a bot is painted with, for a colour and the appearance. `colored` 0 greys it (offline);
/// `alpha` fades it.
#[derive(Clone, Copy, Debug)]
pub struct Paints {
    pub body_top: Hsla,
    pub body_bottom: Hsla,
    pub eye: Hsla,
}

pub fn paints(color: &str, dark: bool, colored: f32, alpha: f32) -> Paints {
    let (h, chroma_scale, grey) = match hue(color) {
        Some(h) => (h, 1., false),
        None => (255., 0.12, true),
    };
    let k = colored.clamp(0., 1.);
    // Offline greys towards a neutral mid tone.
    let mix = |l: f32, c: f32, off_l: f32| (l * k + off_l * (1. - k), c * chroma_scale * k);
    // Muted, at one lightness, so a list of them sits calmly beside a quiet interface; a little
    // darker in dark mode, so none glows. The eyes are a soft deep shade of the bot's own colour,
    // not black.
    let (l, c) = if dark { (0.68, 0.072) } else { (0.81, 0.078) };
    let l = if grey { l - 0.08 } else { l };
    let off = if dark { 0.46 } else { 0.8 };
    let (lt, ct) = mix(l + 0.02, c, off);
    let (lb, cb) = mix(l - 0.03, c * 1.06, off - 0.03);
    let (le, ce) = if dark { mix(0.31, 0.04, 0.3) } else { mix(0.41, 0.055, 0.58) };
    Paints { body_top: oklch(lt, ct, h, alpha), body_bottom: oklch(lb, cb, h, alpha), eye: oklch(le, ce, h, alpha) }
}

/// A colour's swatch, as a picker shows it: the body's own colour.
pub fn swatch(color: &str, dark: bool) -> Hsla {
    paints(color, dark, 1., 1.).body_top
}
