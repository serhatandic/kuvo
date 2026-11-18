//! Pictures in a conversation, drawn as 0.7 drew them: a message of yours shows the pictures it
//! carried under its words, a step that made, showed or took a picture to show you shows it large
//! in the flow, its caption under it; each opens larger over the conversation, among every picture
//! of it (← and →, Escape closes). Which pictures those are is `kuvo_box::pictures`' (as
//! KuvoClient's `ConversationPictures`); their bytes come with the message, or from the computer
//! the conversation runs on (`kuvo/file`), never by a path on this one.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Image, ImageFormat, InteractiveElement, IntoElement, ObjectFit, ParentElement, SharedString, StatefulInteractiveElement, Styled,
    StyledImage as _, Window, div, img, prelude::FluentBuilder as _, px,
};
use kuvo_box::BoxClient;
use kuvo_box::pictures::{self, PICTURE_MOST, Picture, Source};
use kuvo_ui::{ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TypeSize, small::spinner, theme::{radius, space}};

/// What's been read of a picture.
#[derive(Clone, Debug)]
pub enum Read {
    Reading,
    /// `thumb`: the same, made small once as it was read (where it's large), for drawing it in the
    /// conversation; `image` whole, for the viewer.
    Shown { image: Arc<Image>, thumb: Arc<Image>, width: u32, height: u32 },
    /// It couldn't be read (`again`: trying again may bring it), or isn't a kind that shows here.
    Failed { why: String, again: bool },
}

const COULDNT: &str = "Couldn’t load this image.";

fn failed(again: bool) -> Read {
    Read::Failed { why: if again { COULDNT.into() } else { "This kind of image can’t be shown here.".into() }, again }
}

/// What a conversation has read of its pictures, by their ids, the pictures of the row being drawn
/// (found once as it changed, not at each drawing), and how one opens.
#[derive(Clone)]
pub struct Looks<'a> {
    pub read: &'a HashMap<String, Read>,
    pub shown: &'a [Picture],
    pub open: Rc<dyn Fn(&str, &mut Window, &mut App)>,
    /// Reads one that couldn't be read again, by its source's key.
    pub again: Rc<dyn Fn(&str, &mut Window, &mut App)>,
}

impl Looks<'static> {
    /// None read, and nothing opens (where a view draws no pictures yet).
    pub fn none() -> Looks<'static> {
        static EMPTY: std::sync::LazyLock<HashMap<String, Read>> = std::sync::LazyLock::new(HashMap::new);
        Looks { read: &EMPTY, shown: &[], open: Rc::new(|_, _, _| {}), again: Rc::new(|_, _, _| {}) }
    }
}

/// A picture's bytes, read where they are: in the message, or from the conversation's computer.
/// Then what it is, and how large, for drawing it. Off the UI thread.
pub fn read(client: &BoxClient, source: &Source) -> Read {
    let bytes = match source {
        Source::Inline(data) => match pictures::inline_bytes(data) {
            Some(bytes) => bytes,
            None => return failed(false),
        },
        // Yours, sent from here: read where you sent it from.
        Source::Local(path) => match std::fs::metadata(path) {
            Ok(meta) if meta.len() as usize > PICTURE_MOST => return Read::Failed { why: "This image is too large to show here.".into(), again: false },
            Ok(_) => match std::fs::read(path) {
                Ok(bytes) => bytes,
                Err(_) => return failed(true),
            },
            Err(_) => return failed(true),
        },
        Source::File(uri) => match client.file_bytes(uri, PICTURE_MOST) {
            Ok((bytes, _)) => bytes,
            // Too large to show: trying again wouldn't change that.
            Err(kuvo_box::Error::Refused(refusal)) if refusal.status == 413 => return Read::Failed { why: "This image is too large to show here.".into(), again: false },
            // Not there yet, or its computer couldn't be reached: Try Again.
            Err(_) => return failed(true),
        },
    };
    shown(bytes)
}

/// Whether two sources are the same place to read a picture from (a message's own bytes are the
/// same for as long as it has that id, so they aren't compared: a photo's are megabytes).
pub fn same_place(a: &Source, b: &Source) -> bool {
    match (a, b) {
        (Source::Inline(_), Source::Inline(_)) => true,
        (Source::File(a), Source::File(b)) => a == b,
        (Source::Local(a), Source::Local(b)) => a == b,
        _ => false,
    }
}

/// Bytes as a picture to draw, by what they are (whatever the name says).
pub fn shown(bytes: Vec<u8>) -> Read {
    let Some(format) = format_of(&bytes) else { return failed(false) };
    let size = if format == ImageFormat::Svg {
        Some((512, 512))
    } else {
        image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format().ok().and_then(|reader| reader.into_dimensions().ok())
    };
    match size {
        Some((width, height)) if width > 0 && height > 0 => {
            let thumb = thumbnail(format, &bytes, width, height).map(Arc::new);
            let image = Arc::new(Image::from_bytes(format, bytes));
            Read::Shown { thumb: thumb.unwrap_or_else(|| image.clone()), image, width, height }
        }
        _ => failed(false),
    }
}

/// The longest side a picture is drawn from in the conversation: twice the largest it's drawn there
/// (an agent's picture, 560 wide), for a sharp Retina screen.
const THUMB_SIDE: u32 = 1120;

/// A large picture made small for the conversation (as a PNG), so a screenshot of a 5K screen isn't
/// decoded and kept whole for a tile of a few hundred points; none for one that's small already, or
/// that can't be made small (an SVG, an animated GIF). Off the UI thread, once, as it's read.
fn thumbnail(format: ImageFormat, bytes: &[u8], width: u32, height: u32) -> Option<Image> {
    if width.max(height) <= THUMB_SIDE || matches!(format, ImageFormat::Svg | ImageFormat::Gif) {
        return None;
    }
    use image::ImageDecoder as _;
    // Turned as the camera says it was held (a photo's EXIF), as the viewer shows it.
    let mut decoder = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?.into_decoder().ok()?;
    let orientation = decoder.orientation().ok();
    let mut decoded = image::DynamicImage::from_decoder(decoder).ok()?;
    if let Some(orientation) = orientation {
        decoded.apply_orientation(orientation);
    }
    let small = decoded.thumbnail(THUMB_SIDE, THUMB_SIDE);
    let mut out = std::io::Cursor::new(Vec::new());
    // A JPEG where nothing shows through (a photo, a screenshot): far smaller than a PNG of it.
    if small.color().has_alpha() {
        small.write_to(&mut out, image::ImageFormat::Png).ok()?;
        Some(Image::from_bytes(ImageFormat::Png, out.into_inner()))
    } else {
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).encode_image(&small.to_rgb8()).ok()?;
        Some(Image::from_bytes(ImageFormat::Jpeg, out.into_inner()))
    }
}

fn format_of(bytes: &[u8]) -> Option<ImageFormat> {
    let starts = |magic: &[u8]| bytes.starts_with(magic);
    Some(if starts(b"\x89PNG") {
        ImageFormat::Png
    } else if starts(&[0xFF, 0xD8, 0xFF]) {
        ImageFormat::Jpeg
    } else if starts(b"GIF8") {
        ImageFormat::Gif
    } else if starts(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        ImageFormat::Webp
    } else if starts(b"BM") {
        ImageFormat::Bmp
    } else if starts(b"II*\0") || starts(b"MM\0*") {
        ImageFormat::Tiff
    } else {
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
        if head.contains("<svg") { ImageFormat::Svg } else { return None }
    })
}

/// `width` × `height` fitted within `most_w` × `most_h`, never larger than it is.
pub fn fitted(width: u32, height: u32, most_w: f32, most_h: f32) -> (f32, f32) {
    let (w, h) = (width as f32, height as f32);
    let scale = (most_w / w).min(most_h / h).min(1.);
    ((w * scale).max(1.), (h * scale).max(1.))
}

/// A picture in a conversation, fitted within `most_w` × `most_h`: drawn once read, a quiet tile
/// while it's read or where it can't be (saying why), and its caption under it. Click opens it.
pub fn picture(picture: &Picture, most_w: f32, most_h: f32, looks: &Looks, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let id = picture.id.clone();
    let open = looks.open.clone();
    let name = picture.name.clone().unwrap_or_else(|| "Picture".into());
    let selector = format!("picture-{}", picture.id);
    let body: AnyElement = match looks.read.get(picture.id.as_str()) {
        Some(Read::Shown { thumb, width, height, .. }) => {
            let (w, h) = fitted(*width, *height, most_w, most_h);
            div()
                .id(SharedString::from(format!("picture-{}", picture.id)))
                .debug_selector(move || selector)
                .w(px(w))
                .h(px(h))
                .rounded(radius::CARD)
                .overflow_hidden()
                .border_1()
                .border_color(c.border)
                .cursor_pointer()
                .on_click(move |_, window, cx| open(&id, window, cx))
                .child(img(thumb.clone()).size_full().object_fit(ObjectFit::Contain))
                .into_any_element()
        }
        read => {
            let (w, h) = (most_w.min(240.), most_h.min(140.));
            let (lead, words, again): (AnyElement, String, bool) = match read {
                // (Named, as a file that can't be drawn here is still what was sent: a HEIC photo.)
                Some(Read::Failed { why, again }) => (Icon::new(IconName::Image).size(IconSize::Card).color(c.text_faint).into_any_element(), picture.name.as_ref().map_or_else(|| why.clone(), |name| format!("{name}\n{why}")), *again),
                _ => (spinner(SharedString::from(format!("reading-{}", picture.id)), IconSize::Nav, c.text_faint).into_any_element(), name, false),
            };
            let (retry, key) = (looks.again.clone(), picture.id.as_str().to_owned());
            let again_selector = format!("picture-again-{}", picture.id);
            div()
                .debug_selector(move || selector)
                .w(px(w))
                .h(px(h))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(space::S1_5)
                .p(space::S3)
                .rounded(radius::CARD)
                .bg(c.sunken)
                .border_1()
                .border_color(c.border)
                .child(lead)
                .child(div().text_ui(TypeSize::Caption).text_color(c.text_muted).text_center().line_clamp(3).child(words))
                .when(again, |d| {
                    d.child(div().debug_selector(move || again_selector).child(Button::new(SharedString::from(format!("picture-again-{}", picture.id)), "Try Again").size(ButtonSize::Small).on_click(move |_, window, cx| retry(&key, window, cx))))
                })
                .into_any_element()
        }
    };
    div()
        .flex()
        .flex_col()
        .gap(space::S1)
        .child(body)
        .children(picture.caption.clone().map(|caption| div().max_w(px(most_w)).text_ui(TypeSize::Small).text_color(c.text_muted).child(caption)))
        .into_any_element()
}

/// Several pictures in a row that wraps, as 0.7 drew them: one large, several smaller.
pub fn pictures(list: &[Picture], one: (f32, f32), many: (f32, f32), looks: &Looks, cx: &App) -> AnyElement {
    let (most_w, most_h) = if list.len() == 1 { one } else { many };
    div().flex().flex_wrap().gap(space::S2).children(list.iter().map(|each| picture(each, most_w, most_h, looks, cx))).into_any_element()
}

/// The picture opened over the conversation, fitted to the window: its name and where it is among
/// the conversation's (`at` of `count`), its caption, ← and → to the one before and after, and
/// Close (or Escape, or a click beside it).
pub fn viewer(picture: &Picture, at: usize, count: usize, read: Option<&Read>, window: &Window) -> impl IntoElement {
    let size = window.viewport_size();
    let (most_w, most_h) = (f32::from(size.width) - 96., f32::from(size.height) - 160.);
    let shown: AnyElement = match read {
        Some(Read::Shown { image, width, height, .. }) => {
            let (w, h) = fitted(*width, *height, most_w.max(80.), most_h.max(80.));
            div().w(px(w)).h(px(h)).child(img(image.clone()).size_full().object_fit(ObjectFit::Contain)).into_any_element()
        }
        Some(Read::Failed { why, .. }) => div().text_ui(TypeSize::Body).text_color(gpui::white()).child(why.clone()).into_any_element(),
        _ => spinner("viewer-reading", IconSize::Card, gpui::white()).into_any_element(),
    };
    let name = picture.name.clone().unwrap_or_else(|| "Picture".into());
    div()
        .debug_selector(|| "picture-viewer".into())
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(space::S3)
        .bg(gpui::black().opacity(0.82))
        .child(
            div()
                .flex()
                .items_center()
                .gap(space::S2)
                .text_ui(TypeSize::Small)
                .text_color(gpui::white().opacity(0.85))
                .child(name)
                .when(count > 1, |d| d.child(div().text_color(gpui::white().opacity(0.55)).child(format!("{} of {count}", at + 1)))),
        )
        .child(shown)
        .children(picture.caption.clone().map(|caption| div().max_w(px(most_w.max(200.))).text_ui(TypeSize::Small).text_color(gpui::white().opacity(0.8)).text_center().child(caption)))
}

/// The pictures written in a step's words (`![alt](url)`), block `n` of step `step`, that can be
/// read: in the reply itself, or a file on the conversation's computer.
pub fn in_words(step: &str, n: usize, text: &str) -> Vec<Picture> {
    kuvo_ui::markdown::images(text).iter().enumerate().filter_map(|(at, url)| pictures::of_address(format!("{step}:{n}~{at}"), url, "")).collect()
}

/// What draws the pictures in a step's words (block `n` of `step`): each read one as a picture,
/// fitted to `most`; a web address as a row that opens it in your browser; anything else as its words.
pub fn words_pictures(step: &str, n: usize, text: &str, most: (f32, f32), looks: &Looks) -> kuvo_ui::markdown::Pictures {
    let urls = kuvo_ui::markdown::images(text);
    let read: HashMap<String, Read> = in_words(step, n, text).iter().filter_map(|picture| looks.read.get(picture.id.as_str()).map(|read| (picture.id.clone(), read.clone()))).collect();
    let (open, again, step) = (looks.open.clone(), looks.again.clone(), step.to_owned());
    Rc::new(move |url: &str, alt: &str, cx: &App| {
        let at = urls.iter().position(|each| each == url).unwrap_or(0);
        if let Some(mut found) = pictures::of_address(format!("{step}:{n}~{at}"), url, alt) {
            if alt.is_empty() {
                found.name = None;
            }
            let looks = Looks { read: &read, shown: &[], open: open.clone(), again: again.clone() };
            return picture(&found, most.0, most.1, &looks, cx);
        }
        let c = &cx.theme().colors;
        let words = if alt.is_empty() { url.to_owned() } else { alt.to_owned() };
        if url.starts_with("https://") || url.starts_with("http://") {
            let address = url.to_owned();
            return div()
                .id(SharedString::from(format!("web-picture-{step}-{n}-{at}")))
                .flex()
                .items_center()
                .gap(space::S1_5)
                .text_ui(TypeSize::Small)
                .text_color(c.accent)
                .cursor_pointer()
                .child(Icon::new(IconName::Image).size(IconSize::Nav).color(c.accent))
                .child(words)
                .on_click(move |_, _, cx| cx.open_url(&address))
                .into_any_element();
        }
        div().text_color(c.text_muted).child(words).into_any_element()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32, alpha: bool) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        if alpha {
            image::RgbaImage::new(width, height).write_to(&mut out, image::ImageFormat::Png).unwrap();
        } else {
            image::RgbImage::new(width, height).write_to(&mut out, image::ImageFormat::Png).unwrap();
        }
        out.into_inner()
    }

    /// A screenshot of a large screen is drawn in the conversation from a small copy made as it's
    /// read (its shape kept), and opens whole; a small one is drawn as it is.
    #[test]
    fn a_large_picture_is_drawn_from_a_small_copy_and_opens_whole() {
        let Read::Shown { image, thumb, width, height } = shown(png(2880, 1800, false)) else { panic!("read") };
        assert_eq!((width, height), (2880, 1800));
        assert_eq!(image.format(), ImageFormat::Png);
        assert_eq!(thumb.format(), ImageFormat::Jpeg, "nothing shows through it");
        let small = image::load_from_memory(thumb.bytes()).unwrap();
        assert_eq!((small.width(), small.height()), (1120, 700));
        let Read::Shown { thumb, .. } = shown(png(3000, 600, true)) else { panic!("read") };
        assert_eq!(thumb.format(), ImageFormat::Png, "one that shows through keeps it");
        let Read::Shown { image, thumb, .. } = shown(png(400, 300, false)) else { panic!("read") };
        assert!(Arc::ptr_eq(&image, &thumb), "small already");
    }
}
