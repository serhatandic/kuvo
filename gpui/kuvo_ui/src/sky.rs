//! The sky behind the new-session view: a calm staggered halftone, carved by soft clouds, whose
//! tones follow the time of day (dawn, day, dusk, night), blended so there's never a jump.
//!
//! Drawing only. The dot geometry is worked out once per size, theme and five-minute step of the
//! clock, then every repaint just emits the cached dots.

use std::cell::RefCell;

use gpui::{
    App, Bounds, Hsla, IntoElement, ParentElement, Styled, canvas, div, fill, hsla, linear_color_stop, linear_gradient,
    point, px, size,
};

use crate::theme::{ActiveTheme, Appearance};

/// How far apart the dots are, and how often the sky may change: every five minutes.
const PITCH: f32 = 10.;
const STEPS_PER_HOUR: f32 = 12.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeOfDay {
    Dawn,
    Day,
    Dusk,
    Night,
}

/// The two times of day blended at `hour` (local, `0.0..24.0`) and how far toward the second.
/// Each time holds for a while, and the change between two takes an hour and a half to two.
pub fn phase(hour: f32) -> (TimeOfDay, TimeOfDay, f32) {
    use TimeOfDay::*;
    // (from, to, start of the change, end of the change); between changes, `to` holds.
    const CHANGES: [(TimeOfDay, TimeOfDay, f32, f32); 4] = [
        (Night, Dawn, 4.5, 6.0),
        (Dawn, Day, 7.0, 9.0),
        (Day, Dusk, 16.5, 18.0),
        (Dusk, Night, 19.5, 21.5),
    ];
    let hour = hour.rem_euclid(24.);
    // Before the first change of the day, the night from the evening before holds.
    let mut held = Night;
    for (from, to, start, end) in CHANGES {
        if hour < start {
            break;
        }
        if hour < end {
            return (from, to, smoothstep(start, end, hour));
        }
        held = to;
    }
    (held, held, 0.)
}

/// The local hour of day, stepped to five minutes, so the sky changes at most that often.
pub fn stepped(hour: f32) -> f32 {
    (hour.rem_euclid(24.) * STEPS_PER_HOUR).floor() / STEPS_PER_HOUR
}

/// The sky at the local `hour`, filling its parent. Put it first, absolutely positioned, under content.
pub fn sky(hour: f32, cx: &App) -> impl IntoElement {
    let hour = stepped(hour);
    let dark = cx.theme().appearance == Appearance::Dark;
    let (top, bottom) = wash(hour, dark);
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .bg(linear_gradient(180., linear_color_stop(top, 0.), linear_color_stop(bottom, 1.)))
        .child(
            canvas(
                |_bounds, _window, cx: &mut App| cx.theme().appearance == Appearance::Dark,
                move |bounds, dark, window, _cx| {
                    let width = f32::from(bounds.size.width);
                    let height = f32::from(bounds.size.height);
                    if width <= 0. || height <= 0. {
                        return;
                    }
                    let key = (width.round() as u32, height.round() as u32, dark, (hour * STEPS_PER_HOUR) as u32);
                    CACHE.with(|cache| {
                        let mut cache = cache.borrow_mut();
                        if cache.0 != Some(key) {
                            cache.1 = dots(width, height, hour, dark);
                            cache.0 = Some(key);
                        }
                        let (ox, oy) = (bounds.origin.x, bounds.origin.y);
                        for dot in &cache.1 {
                            window.paint_quad(
                                fill(Bounds::new(point(ox + px(dot.x), oy + px(dot.y)), size(px(dot.size), px(dot.size))), dot.color)
                                    .corner_radii(px(dot.size / 2.)),
                            );
                        }
                    });
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
}

type Key = (u32, u32, bool, u32);

thread_local! {
    static CACHE: RefCell<(Option<Key>, Vec<Dot>)> = const { RefCell::new((None, Vec::new())) };
}

/// One dot, in the canvas's own coordinates.
#[derive(Clone, Copy)]
struct Dot {
    x: f32,
    y: f32,
    size: f32,
    color: Hsla,
}

// Colour, mixed in RGB so blends never detour through a muddy or neon hue.

#[derive(Clone, Copy, Debug)]
struct Rgb {
    r: f32,
    g: f32,
    b: f32,
}

impl Rgb {
    const fn hex(v: u32) -> Self {
        Self { r: ((v >> 16) & 0xff) as f32 / 255., g: ((v >> 8) & 0xff) as f32 / 255., b: (v & 0xff) as f32 / 255. }
    }

    fn mix(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0., 1.);
        Self { r: self.r + (other.r - self.r) * t, g: self.g + (other.g - self.g) * t, b: self.b + (other.b - self.b) * t }
    }

    fn to_hsla(self, alpha: f32) -> Hsla {
        let (r, g, b) = (self.r.clamp(0., 1.), self.g.clamp(0., 1.), self.b.clamp(0., 1.));
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let l = (max + min) * 0.5;
        let d = max - min;
        let (h, s) = if d < 1e-6 {
            (0., 0.)
        } else {
            let s = d / (1. - (2. * l - 1.).abs()).max(1e-6);
            let h = if (max - r).abs() < 1e-6 {
                (g - b) / d + if g < b { 6. } else { 0. }
            } else if (max - g).abs() < 1e-6 {
                (b - r) / d + 2.
            } else {
                (r - g) / d + 4.
            };
            ((h / 6.).rem_euclid(1.), s.clamp(0., 1.))
        };
        hsla(h, s, l, alpha.clamp(0., 1.))
    }
}

/// The soft wash under the dots, top and bottom, blended between two times of day.
fn wash(hour: f32, dark: bool) -> (Hsla, Hsla) {
    let (a, b, t) = phase(hour);
    let ((at, ata), (ab, aba)) = wash_of(a, dark);
    let ((bt, bta), (bb, bba)) = wash_of(b, dark);
    (at.mix(bt, t).to_hsla(ata + (bta - ata) * t), ab.mix(bb, t).to_hsla(aba + (bba - aba) * t))
}

fn wash_of(time: TimeOfDay, dark: bool) -> ((Rgb, f32), (Rgb, f32)) {
    use TimeOfDay::*;
    match (time, dark) {
        (Day, false) => ((Rgb::hex(0x3F80DF), 0.12), (Rgb::hex(0x9BC2F2), 0.05)),
        (Dawn, false) => ((Rgb::hex(0x7B8EB8), 0.08), (Rgb::hex(0xDFB49A), 0.06)),
        (Dusk, false) => ((Rgb::hex(0x6C6A9A), 0.09), (Rgb::hex(0xCE9C8E), 0.06)),
        (Night, false) => ((Rgb::hex(0x4F6386), 0.08), (Rgb::hex(0x8899B4), 0.03)),
        (Day, true) => ((Rgb::hex(0x28436A), 0.12), (Rgb::hex(0x1E2A3B), 0.04)),
        (Dawn, true) => ((Rgb::hex(0x2B3752), 0.11), (Rgb::hex(0x4E3B33), 0.07)),
        (Dusk, true) => ((Rgb::hex(0x332E52), 0.12), (Rgb::hex(0x4D373A), 0.07)),
        (Night, true) => ((Rgb::hex(0x202D46), 0.13), (Rgb::hex(0x191F2C), 0.04)),
    }
}

// Noise, for the clouds.

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

fn hash2(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0x9e37_79b9);
    h ^= h >> 13;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 16;
    (h & 0xffff) as f32 / 65535.
}

fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let (sx, sy) = (fx * fx * (3. - 2. * fx), fy * fy * (3. - 2. * fy));
    let (a, b) = (hash2(x0, y0, seed), hash2(x0 + 1, y0, seed));
    let (c, d) = (hash2(x0, y0 + 1, seed), hash2(x0 + 1, y0 + 1, seed));
    let top = a + (b - a) * sx;
    let bottom = c + (d - c) * sx;
    top + (bottom - top) * sy
}

fn fbm2(x: f32, y: f32, seed: u32) -> f32 {
    value_noise(x, y, seed) * 0.66 + value_noise(x * 2.05 + 17.1, y * 2.03 - 9.4, seed.wrapping_add(13)) * 0.34
}

fn warped_cloud(u: f32, v: f32, scale_x: f32, scale_y: f32, seed: u32) -> f32 {
    let wx = value_noise(u * scale_x * 0.72 + 2.3, v * scale_y * 0.72 + 1.1, seed.wrapping_add(7));
    let wy = value_noise(u * scale_x * 0.72 - 4.1, v * scale_y * 0.72 + 5.7, seed.wrapping_add(19));
    fbm2((u + (wx - 0.5) * 0.38) * scale_x, (v + (wy - 0.5) * 0.32) * scale_y, seed)
}

/// Softens the dots behind the chips and the composer, with no hard edge, so text stays crisp.
fn composer_veil(u: f32, v: f32) -> f32 {
    let (dx, dy) = ((u - 0.42) / 0.34, (v - 0.40) / 0.14);
    let chips = 0.30 + 0.70 * smoothstep(0.32, 1.30, (dx * dx + dy * dy).sqrt());
    let (cx, cy) = ((u - 0.50) / 0.44, (v - 0.46) / 0.15);
    let card = 0.58 + 0.42 * smoothstep(0.50, 1.40, (cx * cx + cy * cy).sqrt());
    chips.min(card)
}

// The sky itself.

/// (zenith, mid sky, horizon, cloud highlight)
fn palette(time: TimeOfDay, dark: bool) -> (Rgb, Rgb, Rgb, Rgb) {
    use TimeOfDay::*;
    match (time, dark) {
        (Day, false) => (Rgb::hex(0x2560C4), Rgb::hex(0x4984DD), Rgb::hex(0x7EAEEB), Rgb::hex(0xB6D3F7)),
        (Dawn, false) => (Rgb::hex(0x6B7FA8), Rgb::hex(0xA48C9C), Rgb::hex(0xC9A490), Rgb::hex(0xDFCBBF)),
        (Dusk, false) => (Rgb::hex(0x5E5E92), Rgb::hex(0x92768D), Rgb::hex(0xBE9184), Rgb::hex(0xD6B8B0)),
        (Night, false) => (Rgb::hex(0x4E6184), Rgb::hex(0x6D7FA0), Rgb::hex(0x98A7BF), Rgb::hex(0xBCC6D8)),
        (Day, true) => (Rgb::hex(0x4F6D96), Rgb::hex(0x5B7698), Rgb::hex(0x6B829E), Rgb::hex(0x7E92AA)),
        (Dawn, true) => (Rgb::hex(0x576682), Rgb::hex(0x726470), Rgb::hex(0x887066), Rgb::hex(0x937D73)),
        (Dusk, true) => (Rgb::hex(0x5A5580), Rgb::hex(0x745E6E), Rgb::hex(0x866862), Rgb::hex(0x927670)),
        (Night, true) => (Rgb::hex(0x465878), Rgb::hex(0x526480), Rgb::hex(0x61718A), Rgb::hex(0x708098)),
    }
}

/// The strongest a dot gets: day in light is the ceiling the user picked; the rest stay under it.
fn max_alpha(time: TimeOfDay, dark: bool) -> f32 {
    use TimeOfDay::*;
    match (time, dark) {
        (Day, false) => 0.54,
        (Dawn, false) => 0.45,
        (Dusk, false) => 0.44,
        (Night, false) => 0.42,
        (Day, true) => 0.36,
        (Dawn, true) => 0.34,
        (Dusk, true) => 0.35,
        (Night, true) => 0.32,
    }
}

/// How full a dot is at `(u, v)` (`0..1` across and down), and its colour.
fn sample(u: f32, v: f32, time: TimeOfDay, dark: bool) -> (f32, Rgb) {
    let (zenith, mid, horizon, highlight) = palette(time, dark);
    let between = |tone: f32| {
        if tone < 0.5 { zenith.mix(mid, smoothstep(0., 0.5, tone)) } else { mid.mix(horizon, smoothstep(0.5, 1., tone)) }
    };

    // Day in light: the picked baseline, as drafted.
    if time == TimeOfDay::Day && !dark {
        let c1 = warped_cloud(u * 1.02 - v * 0.16, v * 1.08, 3.0, 3.4, 211);
        let c2 = fbm2(u * 5.6 + 2.4, v * 6.0 - 1.2, 223);
        let cloud = smoothstep(0.34, 0.74, c1 * 0.72 + c2 * 0.28);
        let (dx, dy) = (u - 0.24, (v - 0.14) * 1.2);
        let glow = (-((dx * dx + dy * dy) * 2.2)).exp() * 0.65;
        let color = between(v).mix(highlight, (cloud * 0.58 + glow * 0.55).min(0.90));
        let field = 0.72 * smoothstep(1.05, 0., v) + 0.24;
        let raw = (field - cloud * 0.56 + cloud * (1. - cloud) * 4. * 0.34).clamp(0.10, 0.96);
        return ((raw * composer_veil(u, v)).clamp(0., 1.), color);
    }

    let (cloud, field, tone, soft) = match time {
        TimeOfDay::Dawn => {
            let c1 = warped_cloud(u * 0.96 + v * 0.12, v * 1.12, 2.8, 3.8, 101);
            let c2 = fbm2(u * 5.2 + 1.3, v * 6.4 - 0.8, 109);
            let cloud = smoothstep(0.32, 0.76, c1 * 0.72 + c2 * 0.28);
            (cloud, 0.64 * smoothstep(1.02, 0., v) + 0.18, (v * 0.82 + u * 0.22).clamp(0., 1.), cloud * 0.42)
        }
        TimeOfDay::Day => {
            let c1 = warped_cloud(u * 1.02 - v * 0.16, v * 1.08, 3.0, 3.4, 211);
            let c2 = fbm2(u * 5.6 + 2.4, v * 6.0 - 1.2, 223);
            let cloud = smoothstep(0.34, 0.74, c1 * 0.72 + c2 * 0.28);
            (cloud, 0.68 * smoothstep(1.04, 0., v) + 0.20, v, cloud * 0.36)
        }
        TimeOfDay::Dusk => {
            let c1 = warped_cloud(u * 0.90, v * 1.18, 2.5, 4.4, 307);
            let c2 = fbm2(u * 4.8 - 1.5, v * 7.2 + 0.9, 313);
            let cloud = smoothstep(0.32, 0.75, c1 * 0.70 + c2 * 0.30);
            (cloud, 0.66 * smoothstep(1.02, 0., v) + 0.18, (v * 0.84 + (1. - u) * 0.20).clamp(0., 1.), cloud * 0.40)
        }
        TimeOfDay::Night => {
            let c1 = warped_cloud(u * 0.94 - v * 0.08, v * 1.10, 2.6, 3.6, 409);
            let c2 = fbm2(u * 5.0 + 1.1, v * 5.8, 419);
            let cloud = smoothstep(0.36, 0.78, c1 * 0.70 + c2 * 0.30);
            (cloud, 0.60 * smoothstep(0.98, 0., v) + 0.16, v, cloud * 0.30)
        }
    };
    let color = between(tone).mix(highlight, soft.min(0.55));
    let raw = (field - cloud * 0.54 + cloud * (1. - cloud) * 4. * 0.24).clamp(0.08, 0.92);
    ((raw * composer_veil(u, v)).clamp(0., 1.), color)
}

/// The staggered halftone screen at `hour`, blending the two times of day around it dot by dot.
fn dots(width: f32, height: f32, hour: f32, dark: bool) -> Vec<Dot> {
    let (a, b, t) = phase(hour);
    let alpha_ceiling = max_alpha(a, dark) + (max_alpha(b, dark) - max_alpha(a, dark)) * t;
    let row_step = PITCH * 0.866;
    let rows = (height / row_step) as i32 + 2;
    let columns = (width / PITCH) as i32 + 2;
    let mut out = Vec::with_capacity((rows * columns) as usize);
    for row in 0..rows {
        let y = row as f32 * row_step;
        let stagger = if row & 1 == 1 { PITCH * 0.5 } else { 0. };
        let v = (y / height).clamp(0., 1.);
        for column in -1..columns {
            let x = column as f32 * PITCH + stagger;
            if x < -PITCH || x > width + PITCH {
                continue;
            }
            let u = (x / width).clamp(0., 1.);
            let (density, color) = if t <= 0. || a == b {
                sample(u, v, a, dark)
            } else if t >= 1. {
                sample(u, v, b, dark)
            } else {
                let (da, ca) = sample(u, v, a, dark);
                let (db, cb) = sample(u, v, b, dark);
                (da + (db - da) * t, ca.mix(cb, t))
            };
            if density > 0.11 {
                let radius = 0.85 + density.powf(1.20) * 3.35;
                out.push(Dot {
                    x: x - radius,
                    y: y - radius,
                    size: radius * 2.,
                    color: color.to_hsla((0.18 + density * 0.82) * alpha_ceiling),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use TimeOfDay::*;

    /// What the sky shows at `hour`, as a weight for each time of day.
    fn weights(hour: f32) -> [f32; 4] {
        let (a, b, t) = phase(hour);
        let mut w = [0.; 4];
        let index = |time| match time {
            Dawn => 0,
            Day => 1,
            Dusk => 2,
            Night => 3,
        };
        w[index(a)] += 1. - t;
        w[index(b)] += t;
        w
    }

    #[test]
    fn each_time_of_day_holds_around_its_hours() {
        assert_eq!(weights(2.0), [0., 0., 0., 1.]);
        assert_eq!(weights(6.5), [1., 0., 0., 0.]);
        assert_eq!(weights(12.0), [0., 1., 0., 0.]);
        assert_eq!(weights(19.0), [0., 0., 1., 0.]);
        assert_eq!(weights(23.0), [0., 0., 0., 1.]);
        assert_eq!(weights(24.0), weights(0.0));
    }

    #[test]
    fn the_sky_blends_between_times_with_no_jump() {
        // Halfway through the morning change, dawn and day share the sky.
        let morning = weights(8.0);
        assert!((morning[0] - 0.5).abs() < 1e-4 && (morning[1] - 0.5).abs() < 1e-4);
        // Minute by minute round the clock, no time of day moves by more than a little.
        let mut last = weights(0.);
        for minute in 1..=24 * 60 {
            let now = weights(minute as f32 / 60.);
            for (was, is) in last.iter().zip(now) {
                assert!((was - is).abs() < 0.03, "a jump at minute {minute}");
            }
            last = now;
        }
    }

    #[test]
    fn the_sky_changes_at_most_every_five_minutes() {
        assert_eq!(stepped(8.0 + 4.9 / 60.), 8.0);
        assert_eq!(stepped(8.0 + 5.0 / 60.), 8.0 + 5.0 / 60.);
    }

    #[test]
    fn the_sky_paints_fewer_dots_than_the_dotted_backdrop() {
        // The new-session pane at the standard window size.
        let (width, height) = (961., 775.);
        let old = crate::backdrop::dots_painted(width, height);
        for hour in [2., 6.5, 8., 12., 19., 23.] {
            for dark in [false, true] {
                let new = dots(width, height, hour, dark).len();
                assert!(new * 2 < old, "{new} dots against the backdrop's {old} at {hour}");
            }
        }
    }
}
