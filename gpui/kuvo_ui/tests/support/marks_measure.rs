//! Measures a mark's artwork the way it's seen: drawn once, then the pixels counted. Shared by the
//! test that holds every bundled mark to the standard and the tool that sets each file's frame
//! (`examples/normalize_marks.rs`).

use resvg::{tiny_skia, usvg};

/// What a mark's artwork covers, in the units of the file's own view box.
#[derive(Clone, Copy, Debug)]
pub struct Ink {
    /// The smallest box round everything drawn: left, top, width, height.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// How much of that box is drawn on (0 to 1).
    pub fill: f32,
    /// The file's view box: left, top, width, height.
    pub view: [f32; 4],
}

/// Alpha above which a pixel is part of the mark.
const SEEN: u8 = 24;

/// The `<svg …>` tag's attributes.
fn root_tag(svg: &str) -> (usize, usize) {
    let start = svg.find("<svg").expect("a mark is an SVG");
    (start, start + svg[start..].find('>').expect("an SVG's tag ends"))
}

/// An attribute's value on the root tag.
pub fn attribute(svg: &str, name: &str) -> Option<String> {
    let (start, end) = root_tag(svg);
    let tag = &svg[start..end];
    let at = tag.find(&format!(" {name}=\""))? + name.len() + 3;
    Some(tag[at..].split('"').next()?.to_owned())
}

/// The file's view box.
pub fn view_box(svg: &str) -> [f32; 4] {
    let numbers: Vec<f32> = attribute(svg, "viewBox").expect("a mark has a view box").split([' ', ',']).filter(|s| !s.is_empty()).map(|s| s.parse().unwrap()).collect();
    [numbers[0], numbers[1], numbers[2], numbers[3]]
}

/// The file with the root's size and view box set (and its other attributes kept).
pub fn with_frame(svg: &str, size: &str, view: [f32; 4]) -> String {
    let (start, end) = root_tag(svg);
    let mut tag = svg[start..end].to_owned();
    for name in ["width", "height", "viewBox"] {
        while let Some(at) = tag.find(&format!(" {name}=\"")) {
            let close = at + name.len() + 3 + tag[at + name.len() + 3..].find('"').unwrap() + 1;
            tag.replace_range(at..close, "");
        }
    }
    let view = view.map(|n| format!("{}", (n * 1000.).round() / 1000.));
    format!("{}{} width=\"{size}\" height=\"{size}\" viewBox=\"{}\"{}", &svg[..start], tag.trim_end_matches(char::is_whitespace), view.join(" "), &svg[end..])
        .replacen("\"  ", "\" ", 1)
}

/// The artwork's bounds and how much of them it fills, drawn `px` across along the file's view
/// box, with room round it so artwork outside the view box is counted too.
pub fn measure(svg: &str, px: u32) -> Ink {
    let view = view_box(svg);
    let [vx, vy, vw, vh] = view;
    let wide = [vx - vw, vy - vh, vw * 3., vh * 3.];
    let tree = usvg::Tree::from_data(with_frame(svg, "1", wide).replace("width=\"1\" height=\"1\"", &format!("width=\"{}\" height=\"{}\"", wide[2], wide[3])).as_bytes(), &usvg::Options::default()).expect("a mark is an SVG");
    let scale = px as f32 / vw.max(vh);
    let (width, height) = ((wide[2] * scale).ceil() as u32, (wide[3] * scale).ceil() as u32);
    let mut pixmap = tiny_skia::Pixmap::new(width, height).unwrap();
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
    let mut covered = 0.0f64;
    for (ix, pixel) in pixmap.pixels().iter().enumerate() {
        covered += pixel.alpha() as f64 / 255.;
        if pixel.alpha() >= SEEN {
            let (x, y) = (ix as u32 % width, ix as u32 / width);
            left = left.min(x);
            right = right.max(x + 1);
            top = top.min(y);
            bottom = bottom.max(y + 1);
        }
    }
    assert!(right > left && bottom > top, "a mark draws something");
    let (w, h) = ((right - left) as f32, (bottom - top) as f32);
    Ink { x: wide[0] + left as f32 / scale, y: wide[1] + top as f32 / scale, w: w / scale, h: h / scale, fill: (covered as f32 / (w * h)).min(1.), view }
}

/// A mark's optical size, as a side: the geometric mean of the sides of its box and of the ink in
/// it. A round mark covers less of its box than a square one, so it's drawn a little larger to look
/// as big; a thin outline is held back by its box.
pub fn optical_side(ink: &Ink) -> f32 {
    (ink.w * ink.h).sqrt() * ink.fill.sqrt().sqrt()
}

/// The standard every mark's file is framed to. The frame is a square, the size the mark is drawn at
/// (a slot); the mark is centred in it.
///
/// - its optical size is [`OPTICAL`] of the frame,
/// - but its box never grows past [`LONGEST`] of it.
pub const OPTICAL: f32 = 0.76;
pub const LONGEST: f32 = 1.0;

/// The side of the square frame that gives `ink` the standard's size.
pub fn frame_side(ink: &Ink) -> f32 {
    (optical_side(ink) / OPTICAL).max(ink.w.max(ink.h) / LONGEST)
}
