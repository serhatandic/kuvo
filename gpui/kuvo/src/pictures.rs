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
use kuvo_ui::{ActiveTheme, Icon, IconName, IconSize, StyledType, TypeSize, small::spinner, theme::{radius, space}};

/// What's been read of a picture.
#[derive(Clone, Debug)]
pub enum Read {
    Reading,
    Shown { image: Arc<Image>, width: u32, height: u32 },
    /// Why it can't be shown, in plain words.
    Failed(String),
}

/// What a conversation has read of its pictures, by their sources' keys, and how one opens.
#[derive(Clone)]
pub struct Looks<'a> {
    pub read: &'a HashMap<String, Read>,
    pub open: Rc<dyn Fn(&str, &mut Window, &mut App)>,
}

impl Looks<'static> {
    /// None read, and nothing opens (where a view draws no pictures yet).
    pub fn none() -> Looks<'static> {
        static EMPTY: std::sync::LazyLock<HashMap<String, Read>> = std::sync::LazyLock::new(HashMap::new);
        Looks { read: &EMPTY, open: Rc::new(|_, _, _| {}) }
    }
}

/// A picture's bytes, read where they are: in the message, or from the conversation's computer.
/// Then what it is, and how large, for drawing it. Off the UI thread.
pub fn read(client: &BoxClient, source: &Source) -> Read {
    let bytes = match source {
        Source::Inline(data) => match pictures::inline_bytes(data) {
            Some(bytes) => bytes,
            None => return Read::Failed("This picture couldn’t be read.".into()),
        },
        Source::File(uri) => match client.file_bytes(uri, PICTURE_MOST) {
            Ok((bytes, _)) => bytes,
            Err(kuvo_box::Error::Refused(refusal)) if refusal.status == 404 => return Read::Failed("This picture isn’t there any more.".into()),
            Err(kuvo_box::Error::Refused(refusal)) if refusal.status == 413 => return Read::Failed("This picture is too large to show here.".into()),
            Err(error) => return Read::Failed(format!("This picture couldn’t be read: {error}")),
        },
    };
    shown(bytes)
}

/// Bytes as a picture to draw, by what they are (whatever the name says).
pub fn shown(bytes: Vec<u8>) -> Read {
    let Some(format) = format_of(&bytes) else { return Read::Failed("This kind of picture can’t be shown here.".into()) };
    let size = if format == ImageFormat::Svg {
        Some((512, 512))
    } else {
        image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format().ok().and_then(|reader| reader.into_dimensions().ok())
    };
    match size {
        Some((width, height)) if width > 0 && height > 0 => Read::Shown { image: Arc::new(Image::from_bytes(format, bytes)), width, height },
        _ => Read::Failed("This kind of picture can’t be shown here.".into()),
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
    let body: AnyElement = match looks.read.get(picture.source.key()) {
        Some(Read::Shown { image, width, height }) => {
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
                .child(img(image.clone()).size_full().object_fit(ObjectFit::Contain))
                .into_any_element()
        }
        read => {
            let (w, h) = (most_w.min(240.), most_h.min(140.));
            let (lead, words): (AnyElement, String) = match read {
                Some(Read::Failed(why)) => (Icon::new(IconName::Image).size(IconSize::Card).color(c.text_faint).into_any_element(), format!("{name}: {why}")),
                _ => (spinner(SharedString::from(format!("reading-{}", picture.id)), IconSize::Nav, c.text_faint).into_any_element(), name),
            };
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
        Some(Read::Shown { image, width, height }) => {
            let (w, h) = fitted(*width, *height, most_w.max(80.), most_h.max(80.));
            div().w(px(w)).h(px(h)).child(img(image.clone()).size_full().object_fit(ObjectFit::Contain)).into_any_element()
        }
        Some(Read::Failed(why)) => div().text_ui(TypeSize::Body).text_color(gpui::white()).child(why.clone()).into_any_element(),
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
    let read: HashMap<String, Read> = in_words(step, n, text).iter().filter_map(|picture| looks.read.get(picture.source.key()).map(|read| (picture.source.key().to_owned(), read.clone()))).collect();
    let (open, step) = (looks.open.clone(), step.to_owned());
    Rc::new(move |url: &str, alt: &str, cx: &App| {
        let at = urls.iter().position(|each| each == url).unwrap_or(0);
        if let Some(mut found) = pictures::of_address(format!("{step}:{n}~{at}"), url, alt) {
            if alt.is_empty() {
                found.name = None;
            }
            let looks = Looks { read: &read, open: open.clone() };
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
