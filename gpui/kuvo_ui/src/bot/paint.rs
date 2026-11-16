//! Paints a bot in a pose: its body, its eyes (with their lids), and a symbol where the state has
//! one. Everything is filled paths; GPUI only clips to rectangles, so nothing here clips.

use std::f32::consts::{PI, TAU};
use std::sync::OnceLock;

use gpui::{Background, Bounds, PathBuilder, Pixels, Point, Window, linear_color_stop, linear_gradient, point, px};

use super::look::{Paints, Shape, paints, radial, soften};
use super::motion::{Glyph, Pose};

/// The body's radius in a square of `size` points: room is left above for a hop.
pub fn radius(size: f32) -> f32 {
    size * 0.42
}

fn fill(window: &mut Window, points: &[Point<Pixels>], color: impl Into<Background>) {
    if points.len() < 3 {
        return;
    }
    let mut path = PathBuilder::fill();
    path.move_to(points[0]);
    for p in &points[1..] {
        path.line_to(*p);
    }
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// From the body's own units (radius 1, y down) to the window: squash towards the floor, the
/// flip about the body's middle, the lean about its foot, then the offset.
#[derive(Clone, Copy)]
struct Frame {
    center: (f32, f32),
    r: f32,
    sy: f32,
    sx: f32,
    spin: (f32, f32),
    spin_c: f32,
    lean: (f32, f32),
    offset: (f32, f32),
}

impl Frame {
    fn new(bounds: Bounds<Pixels>, pose: &Pose) -> Self {
        let size = f32::from(bounds.size.width);
        let cx = f32::from(bounds.origin.x) + size / 2.;
        let cy = f32::from(bounds.origin.y) + size / 2. + size * 0.035;
        let sy = pose.sy.clamp(0.6, 1.4);
        Frame {
            center: (cx, cy),
            r: radius(size),
            sy,
            sx: 1. + (1. - sy) * 0.4,
            spin: (pose.spin.sin(), pose.spin.cos()),
            spin_c: 1. - sy,
            lean: (pose.lean.sin(), pose.lean.cos()),
            offset: (pose.x, pose.y),
        }
    }

    fn map(&self, (x, y): (f32, f32)) -> Point<Pixels> {
        let (mut x, mut y) = (x * self.sx, 1. + (y - 1.) * self.sy);
        let (s, c) = self.spin;
        let dy = y - self.spin_c;
        (x, y) = (x * c - dy * s, self.spin_c + x * s + dy * c);
        let (s, c) = self.lean;
        let dy = y - 0.95;
        (x, y) = (x * c - dy * s, 0.95 + x * s + dy * c);
        let (x, y) = (x + self.offset.0, y + self.offset.1);
        point(px(self.center.0 + x * self.r), px(self.center.1 + y * self.r))
    }
}

/// The outline a symbol gives the body (worked out once).
fn glyph_outline(glyph: Glyph) -> Option<&'static [(f32, f32)]> {
    static ORBIT: OnceLock<Vec<(f32, f32)>> = OnceLock::new();
    static BAR: OnceLock<Vec<(f32, f32)>> = OnceLock::new();
    Some(match glyph {
        Glyph::None => return None,
        Glyph::Orbit => ORBIT.get_or_init(|| radial(|_| 0.5)),
        Glyph::Bar => BAR.get_or_init(|| {
            let k = PI / 4.;
            let oct = radial(|th| {
                let a = (th + k / 2.).rem_euclid(k) - k / 2.;
                0.95 * (k / 2.).cos() / a.cos()
            });
            soften(oct, 5)
        }),
    })
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// One eye's shape, before it's placed: its size and lids.
#[derive(Clone, Copy, Debug)]
struct Eye {
    w: f32,
    h: f32,
    /// Corner sharpness: 2 an ellipse, higher squarer.
    q: f32,
    open: f32,
    squint: f32,
    slant: f32,
    curve: f32,
    /// -1 the left eye, 1 the right: which way is outer.
    side: f32,
    /// The thinnest a shut eye gets, so it reads as a lid line, not nothing.
    min: f32,
}

const COLUMNS: usize = 36;

impl Eye {
    /// The eye's outline about its centre: along the top lid, back along the bottom one. Each
    /// column is cut by a straight upper lid (slanted per state) and a curved lower lid, and a
    /// shutting eye closes along its own lids.
    fn outline(&self) -> Vec<(f32, f32)> {
        let h = self.h * self.open.max(1.);
        let shut = (1. - self.open).clamp(0., 1.);
        let columns: Vec<(f32, f32, f32)> = (0..=COLUMNS)
            .map(|i| {
                let u = -1. + 2. * i as f32 / COLUMNS as f32;
                let half = h / 2. * (1. - u.abs().powf(self.q)).max(0.).powf(1. / self.q);
                let meet = h * (0.1 + 0.3 * self.curve * (1. - u * u));
                let top_lid = lerp(-h / 2., meet, shut) + self.slant * 0.34 * h * (self.side * u);
                let bottom_lid = lerp(h / 2. - self.squint * h * (0.64 - 0.3 * u * u), meet, shut);
                let mut top = (-half).max(top_lid);
                let mut bottom = half.min(bottom_lid);
                let thin = self.min * (1. - u * u).max(0.).powf(0.35);
                if bottom - top < thin {
                    let mid = if shut > 0.5 { meet.clamp(-half.max(thin), half.max(thin)) } else { (top + bottom) / 2. };
                    top = mid - thin / 2.;
                    bottom = mid + thin / 2.;
                }
                (u * self.w / 2., top, bottom)
            })
            .collect();
        let mut points: Vec<(f32, f32)> = columns.iter().map(|&(x, t, _)| (x, t)).collect();
        points.extend(columns.iter().rev().map(|&(x, _, b)| (x, b)));
        points
    }
}

/// Paints a bot of `shape` and `color` (the box's names) in `pose`, filling a square `bounds`.
pub fn paint_bot(bounds: Bounds<Pixels>, shape: Shape, color: &str, pose: &Pose, dark: bool, window: &mut Window) {
    let size = f32::from(bounds.size.width);
    let frame = Frame::new(bounds, pose);
    let p: Paints = paints(color, dark, pose.color, pose.alpha);
    let g = pose.glyph_amt.clamp(0., 1.);
    let glyph = if g > 0.001 { pose.glyph } else { Glyph::None };

    // The body, morphing into its symbol.
    let outline = shape.outline();
    let body: Vec<Point<Pixels>> = match glyph_outline(glyph) {
        Some(target) => outline.iter().zip(target).map(|(&(x, y), &(tx, ty))| frame.map((lerp(x, tx, g), lerp(y, ty, g)))).collect(),
        None => outline.iter().map(|&q| frame.map(q)).collect(),
    };

    // Orbit's moon passes behind the body for half its turn.
    let moon = (glyph == Glyph::Orbit).then(|| {
        let phase = TAU * pose.glyph_t / 1.9;
        let (s, c) = phase.sin_cos();
        let tilt: f32 = -0.32;
        let (x, y) = (0.92 * c, 0.34 * s);
        let at = (x * tilt.cos() - y * tilt.sin(), x * tilt.sin() + y * tilt.cos());
        (at, s < 0., 0.21 * g * if s < 0. { 0.8 } else { 1. })
    });
    let moon_points = |at: (f32, f32), r: f32| -> Vec<Point<Pixels>> {
        (0..32)
            .map(|i| {
                let a = TAU * i as f32 / 32.;
                frame.map((at.0 + r * a.cos(), at.1 + r * a.sin()))
            })
            .collect()
    };
    if let Some((at, true, r)) = moon {
        fill(window, &moon_points(at, r), p.body_bottom);
    }
    fill(window, &body, linear_gradient(180., linear_color_stop(p.body_top, 0.), linear_color_stop(p.body_bottom, 1.)));
    if let Some((at, false, r)) = moon {
        fill(window, &moon_points(at, r), p.body_top);
    }

    // Done's sparks: a fan of small dots thrown up off the body, shrinking as they go.
    if pose.burst > 0. {
        let level = Frame { spin: (0., 1.), lean: (0., 1.), ..frame };
        let b = pose.burst;
        let out = 1. - (1. - b) * (1. - b);
        for k in 0..7 {
            let a = -PI / 2. + (k as f32 - 3.) * 0.42;
            let d = 1.04 + 0.6 * out * pose.reach;
            let r = 0.085 * (1. - b).powf(0.7) * if k % 2 == 0 { 1. } else { 0.75 };
            let at = (d * a.cos(), 0.15 + d * a.sin());
            let dots: Vec<Point<Pixels>> = (0..16)
                .map(|i| {
                    let t = TAU * i as f32 / 16.;
                    level.map((at.0 + r * t.cos(), at.1 + r * t.sin()))
                })
                .collect();
            fill(window, &dots, if k % 2 == 0 { p.body_top } else { p.body_bottom });
        }
    }

    // The eyes.
    let (ey, spacing) = (shape.eye_height(), 1.);
    let (w0, h0, sep, q0) = (0.25, 0.37, 0.33, 2.);
    // Small sizes get slightly bigger eyes, as small type gets wider letters.
    let optical = 1. + 0.3 * ((44. - size) / 28.).clamp(0., 1.);
    let min = (1.15 / radius(size)).max(0.04);
    let hide = if glyph != Glyph::None && !glyph.eyes_make_it() { (1. - g * 1.9).max(0.) } else { 1. };
    let make = if glyph.eyes_make_it() { g } else { 0. };
    if hide <= 0.001 {
        return;
    }
    for (i, side) in [(-1f32), 1.].into_iter().enumerate() {
        let turn = pose.turn.clamp(-0.9, 0.9);
        let far = (-side * turn).max(0.);
        let near = (side * turn).max(0.);
        let mut cx = side * sep * spacing * (1. - 0.22 * turn.abs()) + turn * 0.34 + pose.gx;
        let mut cy = ey + pose.gy;
        let scale = pose.scale * optical * hide;
        let mut eye = Eye { w: w0 * scale * (1. - 0.5 * far + 0.08 * near), h: h0 * scale, q: q0, open: pose.open[i], squint: pose.squint, slant: pose.slant, curve: pose.curve, side, min };
        // Bar: the two eyes slide together into one.
        if make > 0. {
            cx = lerp(cx, side * 0.2, make);
            cy = lerp(cy, ey + 0.06, make);
            eye.w = lerp(eye.w, 0.52, make);
            eye.h = lerp(eye.h, 0.19, make);
            eye.q = lerp(eye.q, 6., make);
            eye.open = lerp(eye.open, 1., make);
            eye.squint *= 1. - make;
            eye.slant *= 1. - make;
            eye.curve *= 1. - make;
        }
        let points: Vec<Point<Pixels>> = eye.outline().into_iter().map(|(x, y)| frame.map((cx + x, cy + y))).collect();
        fill(window, &points, p.eye);
    }
}
