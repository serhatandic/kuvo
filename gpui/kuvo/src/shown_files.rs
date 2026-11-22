//! Files the agent shows in a conversation (`kuvo-browser_show_file`), drawn in the Flow layout:
//! single pictures large with a hairline border and caption below, several pictures in one turn as
//! an even 3-across wrapping strip, other files as compact horizontal cards (type badge, name,
//! `kind · size`, Open, and Preview for text files), missing files as calm dashed tombstones with
//! no retry, and fetch failures with Try again.
//!
//! All rendering reads only [`ViewModel`]; [`adapt`] is the single adapter from `kuvo_box`
//! groundwork facts (`ShownFile`, `FileInfo`, picture read state, and open-action status).

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Hsla, Image, InteractiveElement, IntoElement, ObjectFit, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, StyledImage as _, Window, div, img,
    prelude::FluentBuilder as _, px,
};
use kuvo_box::BoxClient;
use kuvo_box::shown_files::{FileInfo, FileKind, ShownFile};
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TypeSize,
    small::spinner,
    theme::{radius, space},
};

use crate::pictures::{self, Read as PictureRead};

/// Max size of a single shown picture in the conversation flow.
const SINGLE_MAX: (f32, f32) = (560., 420.);
/// Height of each tile in a multi-picture strip.
const STRIP_HEIGHT: f32 = 156.;
/// Max width of a compact file card or missing/failed row.
const CARD_MAX_W: f32 = 472.;

/// View-model file kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Picture,
    Pdf,
    Text,
    Other,
}

/// A decoded picture ready to draw in the flow.
#[derive(Clone, Debug)]
pub struct ReadyPicture {
    pub thumb: Arc<Image>,
    pub width: u32,
    pub height: u32,
}

/// View-model state for a shown file.
#[derive(Clone, Debug)]
pub enum State {
    /// Fetching metadata (or picture bytes) from the conversation's computer.
    Loading,
    /// Ready to display (and open / preview).
    Ready {
        picture: Option<ReadyPicture>,
        opening: bool,
        open_error: Option<String>,
    },
    /// The file no longer exists on disk. Calm dashed row, no retry.
    Missing,
    /// A fetch failure (computer offline, unreachable, refused, or unreadable).
    Failed { why: String, again: bool },
}

/// Pure view-model for one shown file.
#[derive(Clone, Debug)]
pub struct ViewModel {
    pub id: String,
    pub path: String,
    pub name: String,
    pub folder: Option<String>,
    pub caption: Option<String>,
    pub kind: Kind,
    pub size: Option<u64>,
    pub state: State,
}

/// Metadata fetch state from `BoxClient::file_info`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InfoRead {
    Loading,
    Ready(FileInfo),
    /// Older box without `kuvo/file/info`: still render the card using the guessed kind.
    Unsupported,
    /// `kuvo_box::Error::NotFound`.
    Missing,
    /// Any other error (computer offline / unreachable / refused).
    Failed(String),
}

/// Live state of an Open action on a shown file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenState {
    Opening,
    /// `open_shown_file` reported `OpenShownError::Box(Error::NotFound)`.
    Missing,
    /// `open_shown_file` failed for another reason.
    Failed(String),
}

/// Callbacks wired by the conversation view for interacting with shown files.
#[derive(Clone)]
pub struct Actions {
    pub open_picture: Rc<dyn Fn(&str, &mut Window, &mut App)>,
    pub open_file: Rc<dyn Fn(&str, &mut Window, &mut App)>,
    pub preview_file: Option<Rc<dyn Fn(&str, &mut Window, &mut App)>>,
    pub retry: Rc<dyn Fn(&str, &mut Window, &mut App)>,
    /// Opens the folder's changes in the side panel.
    pub show_changes: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
}

impl Actions {
    pub fn none() -> Self {
        Self {
            open_picture: Rc::new(|_, _, _| {}),
            open_file: Rc::new(|_, _, _| {}),
            preview_file: None,
            retry: Rc::new(|_, _, _| {}),
            show_changes: None,
        }
    }
}

/// Fetches file metadata from `client` off the UI thread, distinguishing `NotFound` from older
/// boxes and real fetch failures.
pub fn read_info(client: &BoxClient, uri: &str) -> InfoRead {
    match client.file_info(uri) {
        Ok(info) => InfoRead::Ready(info),
        Err(kuvo_box::Error::NotFound) => InfoRead::Missing,
        Err(kuvo_box::Error::Refused(refusal)) if refusal.older_box => InfoRead::Unsupported,
        Err(error) => InfoRead::Failed(error.to_string()),
    }
}

/// Single adapter from groundwork facts (`ShownFile`, `InfoRead`, picture `Read`, and `OpenState`)
/// into the view's [`ViewModel`].
pub fn adapt(
    shown: &ShownFile,
    info: Option<&InfoRead>,
    picture: Option<&PictureRead>,
    open: Option<&OpenState>,
) -> ViewModel {
    let from_kind = |kind: FileKind| match kind {
        FileKind::Picture => Kind::Picture,
        FileKind::Pdf => Kind::Pdf,
        FileKind::Text => Kind::Text,
        FileKind::Other => Kind::Other,
    };
    let (name, kind, size) = match info {
        Some(InfoRead::Ready(meta)) => {
            let name = if meta.name.is_empty() { shown.name.clone() } else { meta.name.clone() };
            (name, from_kind(meta.kind), Some(meta.size))
        }
        _ => (shown.name.clone(), from_kind(shown.kind), None),
    };
    let state = if matches!(open, Some(OpenState::Missing)) {
        State::Missing
    } else if kind == Kind::Picture {
        match picture {
            Some(PictureRead::Shown { thumb, width, height, .. }) => State::Ready {
                picture: Some(ReadyPicture { thumb: thumb.clone(), width: *width, height: *height }),
                opening: false,
                open_error: None,
            },
            Some(PictureRead::Missing) => State::Missing,
            Some(PictureRead::Failed { why, again }) => State::Failed { why: why.clone(), again: *again },
            Some(PictureRead::Reading) | None => State::Loading,
        }
    } else {
        match info {
            Some(InfoRead::Ready(_) | InfoRead::Unsupported) => State::Ready {
                picture: None,
                opening: matches!(open, Some(OpenState::Opening)),
                open_error: match open {
                    Some(OpenState::Failed(why)) => Some(why.clone()),
                    _ => None,
                },
            },
            Some(InfoRead::Missing) => State::Missing,
            Some(InfoRead::Failed(why)) => State::Failed { why: why.clone(), again: true },
            Some(InfoRead::Loading) | None => State::Loading,
        }
    };
    ViewModel {
        id: shown.id.clone(),
        path: shown.path.clone(),
        name,
        folder: parent_folder(&shown.path),
        caption: shown.caption.clone(),
        kind,
        size,
        state,
    }
}

/// Renders a turn's shown files in call order: consecutive displayable pictures form a single
/// large figure (1) or an even 3-across wrapping strip (2+); other files render as compact
/// horizontal cards; missing files as dashed tombstones; fetch failures with Try again.
pub fn render_group(files: &[ViewModel], actions: &Actions, cx: &App) -> AnyElement {
    let mut blocks: Vec<AnyElement> = Vec::new();
    let mut pics: Vec<&ViewModel> = Vec::new();

    let flush_pics = |pics: &mut Vec<&ViewModel>, blocks: &mut Vec<AnyElement>| {
        if pics.is_empty() {
            return;
        }
        let batch = std::mem::take(pics);
        if batch.len() == 1 {
            blocks.push(single_picture(batch[0], actions, cx));
        } else {
            blocks.push(picture_strip(&batch, actions, cx));
        }
    };

    for file in files {
        let is_display_picture = file.kind == Kind::Picture
            && matches!(file.state, State::Ready { picture: Some(_), .. } | State::Loading);
        if is_display_picture {
            pics.push(file);
            continue;
        }
        flush_pics(&mut pics, &mut blocks);
        let item = match &file.state {
            State::Missing => missing_row(file, cx),
            State::Failed { why, again } => failed_row(file, why, *again, actions, cx),
            State::Loading | State::Ready { .. } => file_card(file, actions, cx),
        };
        blocks.push(item);
    }
    flush_pics(&mut pics, &mut blocks);

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(space::S3)
        .children(blocks)
        .into_any_element()
}

fn single_picture(file: &ViewModel, actions: &Actions, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let selector = format!("picture-{}", file.id);
    let frame: AnyElement = match &file.state {
        State::Ready { picture: Some(pic), .. } => {
            let (w, h) = pictures::fitted(pic.width, pic.height, SINGLE_MAX.0, SINGLE_MAX.1);
            let id = file.id.clone();
            let open = actions.open_picture.clone();
            div()
                .id(SharedString::from(format!("shown-pic-{}", file.id)))
                .debug_selector(move || selector)
                .w(px(w))
                .h(px(h))
                .rounded(radius::CARD)
                .overflow_hidden()
                .bg(c.sunken)
                .border_1()
                .border_color(c.border)
                .cursor_pointer()
                .on_click(move |_, window, cx| open(&id, window, cx))
                .child(img(pic.thumb.clone()).size_full().object_fit(ObjectFit::Contain))
                .into_any_element()
        }
        _ => div()
            .debug_selector(move || selector)
            .w(px(280.))
            .h(px(168.))
            .rounded(radius::CARD)
            .bg(c.sunken)
            .border_1()
            .border_color(c.border)
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(space::S1_5)
            .p(space::S3)
            .child(spinner(SharedString::from(format!("shown-loading-{}", file.id)), IconSize::Nav, c.text_faint))
            .child(div().max_w_full().truncate().text_ui(TypeSize::Caption).text_color(c.text_muted).child(file.name.clone()))
            .into_any_element(),
    };
    div()
        .flex()
        .flex_col()
        .gap(space::S1_5)
        .child(frame)
        .when_some(file.caption.as_ref(), |d, caption| {
            d.child(
                div()
                    .max_w(px(SINGLE_MAX.0))
                    .text_ui(TypeSize::Small)
                    .text_color(c.text_muted)
                    .child(caption.clone()),
            )
        })
        .into_any_element()
}

fn picture_strip(items: &[&ViewModel], actions: &Actions, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let columns = if items.len() == 2 { 2 } else { 3 };
    let rows = items.chunks(columns).map(|chunk| {
        let missing_slots = columns.saturating_sub(chunk.len());
        div()
            .w_full()
            .flex()
            .gap(space::S3)
            .children(chunk.iter().map(|file| {
                let selector = format!("picture-{}", file.id);
                let frame: AnyElement = match &file.state {
                    State::Ready { picture: Some(pic), .. } => {
                        let id = file.id.clone();
                        let open = actions.open_picture.clone();
                        div()
                            .id(SharedString::from(format!("shown-pic-{}", file.id)))
                            .debug_selector(move || selector)
                            .w_full()
                            .h(px(STRIP_HEIGHT))
                            .rounded(radius::POPOVER)
                            .overflow_hidden()
                            .bg(c.sunken)
                            .border_1()
                            .border_color(c.border)
                            .cursor_pointer()
                            .on_click(move |_, window, cx| open(&id, window, cx))
                            .child(img(pic.thumb.clone()).size_full().object_fit(ObjectFit::Contain))
                            .into_any_element()
                    }
                    _ => div()
                        .debug_selector(move || selector)
                        .w_full()
                        .h(px(STRIP_HEIGHT))
                        .rounded(radius::POPOVER)
                        .bg(c.sunken)
                        .border_1()
                        .border_color(c.border)
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(space::S1_5)
                        .p(space::S2)
                        .child(spinner(SharedString::from(format!("shown-loading-{}", file.id)), IconSize::Nav, c.text_faint))
                        .child(div().max_w_full().truncate().text_ui(TypeSize::Caption).text_color(c.text_muted).child(file.name.clone()))
                        .into_any_element(),
                };
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(space::S1_5)
                    .child(frame)
                    .when_some(file.caption.as_ref(), |d, caption| {
                        d.child(
                            div()
                                .text_ui(TypeSize::Caption)
                                .text_color(c.text_muted)
                                .child(caption.clone()),
                        )
                    })
            }))
            .children((0..missing_slots).map(|_| div().flex_1().min_w_0()))
    });
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(space::S3)
        .children(rows)
        .into_any_element()
}

fn file_card(file: &ViewModel, actions: &Actions, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let tint = badge_tint(file.kind, cx);
    let tag = badge_tag(&file.name, file.kind);
    let kind_name = kind_label(&file.name, file.kind);
    let loading = matches!(file.state, State::Loading);
    let (opening, open_error) = match &file.state {
        State::Ready { opening, open_error, .. } => (*opening, open_error.clone()),
        _ => (false, None),
    };

    let badge = div()
        .size(px(38.))
        .flex_none()
        .rounded(radius::NAV)
        .bg(tint.opacity(0.11))
        .border_1()
        .border_color(tint.opacity(0.24))
        .flex()
        .items_center()
        .justify_center()
        .text_ui(TypeSize::Caption)
        .medium()
        .text_color(tint)
        .child(tag);

    let subline: AnyElement = if loading {
        div()
            .flex()
            .items_center()
            .gap(space::S1)
            .text_ui(TypeSize::Caption)
            .text_color(c.text_muted)
            .child(spinner(SharedString::from(format!("shown-info-{}", file.id)), IconSize::XSmall, c.text_faint))
            .child(format!("{kind_name} · Checking…"))
            .into_any_element()
    } else {
        let meta = match file.size {
            Some(size) => format!("{kind_name} · {}", format_size(size)),
            None => kind_name.to_owned(),
        };
        div()
            .truncate()
            .text_ui(TypeSize::Caption)
            .text_color(c.text_muted)
            .child(meta)
            .into_any_element()
    };

    let action_area: Option<AnyElement> = if loading {
        None
    } else if opening {
        Some(
            div()
                .flex_none()
                .h(px(26.))
                .px(space::S2)
                .rounded(radius::NAV)
                .bg(c.surface)
                .border_1()
                .border_color(c.border)
                .flex()
                .items_center()
                .gap(space::S1)
                .text_ui(TypeSize::Caption)
                .text_color(c.text_muted)
                .child(spinner(SharedString::from(format!("shown-open-{}", file.id)), IconSize::XSmall, c.text_faint))
                .child("Opening…")
                .into_any_element(),
        )
    } else if file.kind == Kind::Text
        && let Some(preview) = actions.preview_file.clone()
    {
        let preview_path = file.path.clone();
        let open = actions.open_file.clone();
        let open_id = file.id.clone();
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(space::S1)
                .child(action_pill(
                    SharedString::from(format!("shown-preview-{}", file.id)),
                    "Preview",
                    IconName::PanelRight,
                    move |window, cx| preview(&preview_path, window, cx),
                    cx,
                ))
                .child(
                    div()
                        .id(SharedString::from(format!("shown-open-{}", file.id)))
                        .debug_selector({
                            let selector = format!("shown-open-{}", file.id);
                            move || selector
                        })
                        .size(px(26.))
                        .rounded(radius::NAV)
                        .border_1()
                        .border_color(c.border)
                        .bg(c.surface)
                        .hover(|s| s.bg(c.hover))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .tooltip(|_, cx| kuvo_ui::menu::Tooltip::new("Open").build(cx))
                        .on_click(move |_, window, cx| open(&open_id, window, cx))
                        .child(Icon::new(IconName::External).size(IconSize::XSmall).color(c.text_muted)),
                )
                .into_any_element(),
        )
    } else {
        let open = actions.open_file.clone();
        let open_id = file.id.clone();
        Some(
            div()
                .flex_none()
                .child(action_pill(
                    SharedString::from(format!("shown-open-{}", file.id)),
                    "Open",
                    IconName::External,
                    move |window, cx| open(&open_id, window, cx),
                    cx,
                ))
                .into_any_element(),
        )
    };

    let selector = format!("shown-file-{}", file.id);
    div()
        .flex()
        .flex_col()
        .gap(space::S1)
        .child(
            div()
                .debug_selector(move || selector)
                .w_full()
                .max_w(px(CARD_MAX_W))
                .px(space::S3)
                .py(space::S2)
                .rounded(radius::POPOVER)
                .bg(c.card)
                .border_1()
                .border_color(c.border)
                .shadow(cx.theme().card_shadow())
                .flex()
                .items_center()
                .gap(space::S3)
                .child(badge)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .child(
                            div()
                                .truncate()
                                .text_ui(TypeSize::Small)
                                .medium()
                                .text_color(c.text)
                                .child(file.name.clone()),
                        )
                        .child(subline),
                )
                .children(action_area),
        )
        .when_some(open_error, |d, error| {
            d.child(
                div()
                    .max_w(px(CARD_MAX_W))
                    .text_ui(TypeSize::Caption)
                    .text_color(c.danger)
                    .child(error),
            )
        })
        .when_some(file.caption.as_ref(), |d, caption| {
            d.child(
                div()
                    .max_w(px(CARD_MAX_W))
                    .text_ui(TypeSize::Small)
                    .text_color(c.text_muted)
                    .child(caption.clone()),
            )
        })
        .into_any_element()
}

fn missing_row(file: &ViewModel, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let icon = if file.kind == Kind::Picture { IconName::Image } else { IconName::File };
    let subline = match &file.folder {
        Some(folder) => format!("No longer on disk · {folder}/"),
        None => "No longer on disk".to_owned(),
    };
    let selector = format!("shown-missing-{}", file.id);
    div()
        .flex()
        .flex_col()
        .gap(space::S1)
        .child(
            div()
                .debug_selector(move || selector)
                .w_full()
                .max_w(px(CARD_MAX_W))
                .px(space::S3)
                .py(space::S2)
                .rounded(radius::POPOVER)
                .bg(c.sunken)
                .border_1()
                .border_dashed()
                .border_color(c.border_strong)
                .flex()
                .items_center()
                .gap(space::S2)
                .child(
                    div()
                        .size(px(32.))
                        .flex_none()
                        .rounded(radius::NAV)
                        .bg(c.surface)
                        .border_1()
                        .border_color(c.border)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(icon).size(IconSize::Nav).color(c.text_faint)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .truncate()
                                .text_ui(TypeSize::Small)
                                .text_color(c.text_muted)
                                .child(file.name.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_ui(TypeSize::Caption)
                                .text_color(c.text_faint)
                                .child(subline),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .px(space::S2)
                        .py(px(2.))
                        .rounded(radius::PILL)
                        .bg(c.surface)
                        .border_1()
                        .border_color(c.border)
                        .text_ui(TypeSize::Caption)
                        .text_color(c.text_faint)
                        .child("Missing"),
                ),
        )
        .when_some(file.caption.as_ref(), |d, caption| {
            d.child(
                div()
                    .max_w(px(CARD_MAX_W))
                    .text_ui(TypeSize::Small)
                    .text_color(c.text_faint)
                    .child(caption.clone()),
            )
        })
        .into_any_element()
}

fn failed_row(file: &ViewModel, why: &str, again: bool, actions: &Actions, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let icon = if file.kind == Kind::Picture { IconName::Image } else { IconName::File };
    let selector = format!("shown-failed-{}", file.id);
    let retry_id = file.id.clone();
    let retry = actions.retry.clone();
    div()
        .flex()
        .flex_col()
        .gap(space::S1)
        .child(
            div()
                .debug_selector(move || selector)
                .w_full()
                .max_w(px(CARD_MAX_W))
                .px(space::S3)
                .py(space::S2)
                .rounded(radius::POPOVER)
                .bg(c.sunken)
                .border_1()
                .border_color(c.border)
                .flex()
                .items_center()
                .gap(space::S2)
                .child(
                    div()
                        .size(px(32.))
                        .flex_none()
                        .rounded(radius::NAV)
                        .bg(c.surface)
                        .border_1()
                        .border_color(c.border)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(icon).size(IconSize::Nav).color(c.text_faint)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .truncate()
                                .text_ui(TypeSize::Small)
                                .text_color(c.text_muted)
                                .child(file.name.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_ui(TypeSize::Caption)
                                .text_color(c.text_faint)
                                .child(why.to_owned()),
                        ),
                )
                .when(again, |d| {
                    let retry_selector = format!("shown-again-{}", file.id);
                    d.child(
                        div()
                            .flex_none()
                            .debug_selector(move || retry_selector)
                            .child(
                                Button::new(SharedString::from(format!("shown-again-{}", file.id)), "Try again")
                                    .size(ButtonSize::Small)
                                    .on_click(move |_, window, cx| retry(&retry_id, window, cx)),
                            ),
                    )
                }),
        )
        .when_some(file.caption.as_ref(), |d, caption| {
            d.child(
                div()
                    .max_w(px(CARD_MAX_W))
                    .text_ui(TypeSize::Small)
                    .text_color(c.text_faint)
                    .child(caption.clone()),
            )
        })
        .into_any_element()
}

fn action_pill(
    id: SharedString,
    label: &'static str,
    icon: IconName,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let c = &cx.theme().colors;
    let selector = id.to_string();
    div()
        .id(id)
        .debug_selector(move || selector)
        .h(px(26.))
        .px(space::S2)
        .rounded(radius::NAV)
        .bg(c.surface)
        .border_1()
        .border_color(c.border)
        .hover(|s| s.bg(c.hover))
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(space::S1)
        .text_ui(TypeSize::Caption)
        .medium()
        .text_color(c.text)
        .on_click(move |_, window, cx| on_click(window, cx))
        .child(label)
        .child(Icon::new(icon).size(IconSize::XSmall).color(c.text_muted))
        .into_any_element()
}

fn badge_tint(kind: Kind, cx: &App) -> Hsla {
    let c = &cx.theme().colors;
    match kind {
        Kind::Pdf => c.danger,
        Kind::Text => c.accent,
        Kind::Picture | Kind::Other => c.text_muted,
    }
}

fn extension_of(name: &str) -> Option<String> {
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty()).then(|| ext.to_ascii_lowercase())
}

/// Short uppercase extension tag for the card's left badge (`PDF`, `MD`, `JSON`, `TXT`…).
pub fn badge_tag(name: &str, kind: Kind) -> String {
    if kind == Kind::Pdf {
        return "PDF".into();
    }
    if let Some(ext) = extension_of(name)
        && ext.len() <= 4
        && ext.chars().all(|ch| ch.is_ascii_alphanumeric())
    {
        return ext.to_ascii_uppercase();
    }
    match kind {
        Kind::Picture => "IMG".into(),
        Kind::Pdf => "PDF".into(),
        Kind::Text => "TXT".into(),
        Kind::Other => "FILE".into(),
    }
}

/// Human-readable file kind for the card's `kind · size` subtitle.
pub fn kind_label(name: &str, kind: Kind) -> &'static str {
    match kind {
        Kind::Picture => "Image",
        Kind::Pdf => "PDF",
        Kind::Text => match extension_of(name).as_deref() {
            Some("md" | "markdown") => "Markdown",
            Some("json") => "JSON",
            Some("csv") => "CSV",
            Some("yaml" | "yml") => "YAML",
            Some("html" | "htm") => "HTML",
            Some("css") => "CSS",
            Some("xml") => "XML",
            Some("toml") => "TOML",
            Some("log") => "Log",
            _ => "Text",
        },
        Kind::Other => match extension_of(name).as_deref() {
            Some("mp4" | "mov" | "webm" | "mkv") => "Video",
            Some("mp3" | "wav" | "m4a" | "flac" | "ogg") => "Audio",
            Some("zip" | "tar" | "gz" | "tgz" | "dmg" | "pkg") => "Archive",
            _ => "File",
        },
    }
}

/// Human-readable file size (`840 B`, `18 KB`, `2.4 MB`, `1.2 GB`).
pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if b < MB {
        let kb = b / KB;
        if kb >= 10.0 { format!("{:.0} KB", kb) } else { format!("{:.1} KB", kb) }
    } else if b < GB {
        let mb = b / MB;
        if mb >= 10.0 { format!("{:.0} MB", mb) } else { format!("{:.1} MB", mb) }
    } else {
        format!("{:.1} GB", b / GB)
    }
}

/// Up to two parent directory components of `path` (e.g. `"/Users/sam/docs/release/a.pdf"` ->
/// `"docs/release"`, `"/tmp/a.pdf"` -> `"tmp"`).
pub fn parent_folder(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    let (parent, _) = trimmed.rsplit_once('/')?;
    let parts: Vec<&str> = parent.split('/').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        return None;
    }
    let keep = parts.len().min(2);
    Some(parts[parts.len() - keep..].join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuvo_box::shown_files::ShownFile;

    fn sample_shown(path: &str, kind: FileKind, caption: Option<&str>) -> ShownFile {
        let name = path.rsplit('/').next().unwrap_or(path).to_owned();
        ShownFile {
            id: "msg_1:0-shown".into(),
            uri: kuvo_box::pictures::file_uri(path),
            path: path.into(),
            name,
            caption: caption.map(str::to_owned),
            kind,
            computer: None,
            session: None,
        }
    }

    #[test]
    fn adapt_maps_document_states_loading_ready_missing_failed_and_older_box() {
        let pdf = sample_shown("/Users/sam/docs/release/Q4-launch-report.pdf", FileKind::Pdf, Some("Summary"));

        let loading = adapt(&pdf, Some(&InfoRead::Loading), None, None);
        assert!(matches!(loading.state, State::Loading));
        assert_eq!(loading.kind, Kind::Pdf);
        assert_eq!(loading.size, None);
        assert_eq!(loading.folder.as_deref(), Some("docs/release"));

        let info = FileInfo {
            name: "Q4-launch-report.pdf".into(),
            size: 2_516_582,
            kind: FileKind::Pdf,
            mime: "application/pdf".into(),
        };
        let ready = adapt(&pdf, Some(&InfoRead::Ready(info.clone())), None, None);
        assert!(matches!(ready.state, State::Ready { picture: None, opening: false, open_error: None }));
        assert_eq!(ready.size, Some(2_516_582));

        let missing = adapt(&pdf, Some(&InfoRead::Missing), None, None);
        assert!(matches!(missing.state, State::Missing));

        let open_missing = adapt(&pdf, Some(&InfoRead::Ready(info.clone())), None, Some(&OpenState::Missing));
        assert!(matches!(open_missing.state, State::Missing));

        let failed = adapt(&pdf, Some(&InfoRead::Failed("The computer is offline.".into())), None, None);
        assert!(matches!(failed.state, State::Failed { again: true, .. }));

        let older = adapt(&pdf, Some(&InfoRead::Unsupported), None, None);
        assert!(matches!(older.state, State::Ready { picture: None, .. }));
        assert_eq!(older.size, None);
    }

    #[test]
    fn adapt_maps_picture_missing_vs_failed_and_ready() {
        let pic = sample_shown("/tmp/old-wireframe.png", FileKind::Picture, Some("Hero"));

        let missing = adapt(&pic, None, Some(&PictureRead::Missing), None);
        assert!(matches!(missing.state, State::Missing));
        assert_eq!(missing.folder.as_deref(), Some("tmp"));

        let unreachable = adapt(&pic, None, Some(&PictureRead::Failed { why: "Offline".into(), again: true }), None);
        assert!(matches!(unreachable.state, State::Failed { again: true, .. }));

        let too_big = adapt(&pic, None, Some(&PictureRead::Failed { why: "Too large".into(), again: false }), None);
        assert!(matches!(too_big.state, State::Failed { again: false, .. }));
    }

    #[test]
    fn formatting_helpers_produce_clean_labels() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1_536), "1.5 KB");
        assert_eq!(format_size(18_432), "18 KB");
        assert_eq!(format_size(2_516_582), "2.4 MB");
        assert_eq!(badge_tag("Q4-launch-report.pdf", Kind::Pdf), "PDF");
        assert_eq!(badge_tag("migration-plan.md", Kind::Text), "MD");
        assert_eq!(badge_tag("archive.tar.gz", Kind::Other), "GZ");
        assert_eq!(badge_tag("README", Kind::Text), "TXT");
        assert_eq!(kind_label("migration-plan.md", Kind::Text), "Markdown");
        assert_eq!(kind_label("Q4-launch-report.pdf", Kind::Pdf), "PDF");
        assert_eq!(parent_folder("/Users/sam/docs/release/a.pdf").as_deref(), Some("docs/release"));
        assert_eq!(parent_folder("/tmp/a.pdf").as_deref(), Some("tmp"));
        assert_eq!(parent_folder("/a.pdf"), None);
    }
}
