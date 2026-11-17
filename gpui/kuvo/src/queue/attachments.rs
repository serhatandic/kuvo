//! Attachment anchors in a description, and the same file cards in the editor and on the page.

use std::{collections::{HashMap, HashSet}, ops::Range, path::PathBuf, rc::Rc};

use gpui::{AnyElement, App, Context, InteractiveElement, IntoElement, ParentElement, SharedString, Styled, Window, div, prelude::FluentBuilder as _};
use kuvo_box::{pictures::{Picture, Source}, work_queue::Whole};
use kuvo_ui::{ActiveTheme, Button, ButtonSize, IconName, StyledType, TypeSize, markdown, theme::{radius, space}};

use super::QueueView;
use crate::pictures::{self, Looks, Read};

const DRAFT: &str = "kuvo-attachment:draft/";
const KEPT: &str = "kuvo-attachment:file/";
const LEGACY: &str = "kuvo-attachment:legacy/";

pub(crate) fn draft_marker(key: usize, name: &str) -> String {
    format!("![Attachment]({DRAFT}{key}/{})", kuvo_box::paths::segment(name))
}

fn draft_key(url: &str) -> Option<usize> {
    url.strip_prefix(DRAFT)?.split_once('/')?.0.parse().ok()
}

pub(crate) fn draft_ranges(text: &str, key: usize) -> Vec<Range<usize>> {
    markdown::image_ranges(text).into_iter().filter_map(|(range, url)| (draft_key(&url) == Some(key)).then_some(range)).collect()
}

/// Replaces the draft's stable keys with indexes into the files going with this save.
pub(super) fn for_save(body: &str, files: Vec<(usize, PathBuf)>) -> (String, Vec<PathBuf>) {
    let positions: HashMap<_, _> = files.iter().enumerate().map(|(index, (key, _))| (*key, index)).collect();
    let mut used = HashSet::new();
    let mut changes = Vec::new();
    for (range, url) in markdown::image_ranges(body) {
        if let Some(key) = draft_key(&url).filter(|key| positions.contains_key(key)) {
            used.insert(key);
            changes.push((range, key, url));
        }
    }
    // A marker deleted with the keyboard removes its file from what the draft will upload.
    let mut positions = HashMap::new();
    let mut paths = Vec::new();
    for (key, path) in files {
        if used.contains(&key) {
            positions.insert(key, paths.len());
            paths.push(path);
        }
    }
    let mut text = body.to_owned();
    for (range, key, url) in changes.into_iter().rev() {
        if let Some((_, name)) = url.strip_prefix(DRAFT).and_then(|url| url.split_once('/')) {
            text.replace_range(range, &format!("![Attachment]({DRAFT}{}/{name})", positions[&key]));
        }
    }
    (text, paths)
}

#[derive(Clone)]
pub(super) enum Origin {
    Draft(usize, PathBuf),
    Kept(String, f64),
}

#[derive(Clone)]
pub(super) struct Attachment {
    pub name: String,
    pub image: bool,
    pub size: Option<u64>,
    pub origin: Origin,
    pub anchor: Option<String>,
}

impl Attachment {
    pub(super) fn key(&self) -> String {
        match &self.origin {
            Origin::Draft(key, path) => format!("draft/{key}/{}", path.display()),
            Origin::Kept(item, at) => format!("{item}/{at}/{}", self.name),
        }
    }

    fn picture(&self) -> Picture {
        let source = match &self.origin {
            Origin::Draft(_, path) => Source::Local(path.clone()),
            Origin::Kept(_, _) => Source::File(String::new()),
        };
        Picture { id: self.key(), source, name: Some(self.name.clone()), caption: Some(self.name.clone()) }
    }
}

pub(super) fn kept(whole: &Whole) -> Vec<Attachment> {
    whole.files.iter().map(|file| Attachment {
        name: file.name.clone(),
        image: file.kind.starts_with("image/"),
        size: Some(file.size),
        origin: Origin::Kept(whole.item.id.clone(), file.at),
        anchor: file.anchor.clone(),
    }).collect()
}

pub(super) fn drafts(composer: &crate::composer::Composer) -> Vec<Attachment> {
    composer.item_attachments().map(|(key, path)| Attachment {
        name: path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "Attachment".into()),
        image: kuvo_box::parts::mime_of(path).starts_with("image/"),
        size: None,
        origin: Origin::Draft(key, path.clone()),
        anchor: None,
    }).collect()
}

pub(super) fn resolve<'a>(url: &str, files: &'a [Attachment]) -> Option<&'a Attachment> {
    if let Some(anchor) = url.strip_prefix(KEPT) {
        return files.iter().find(|file| file.anchor.as_deref() == Some(anchor));
    }
    if let Some(number) = url.strip_prefix(LEGACY).and_then(|number| number.parse::<usize>().ok()) {
        return files.iter().filter(|file| file.image && matches!(file.origin, Origin::Kept(..))).nth(number.checked_sub(1)?);
    }
    let key = draft_key(url)?;
    if let Some(draft) = files.iter().find(|file| matches!(file.origin, Origin::Draft(at, _) if at == key)) {
        return Some(draft);
    }
    // A computer with an older attachment implementation kept the original draft link. Match a
    // unique original name where possible. Its Files list still exposes every saved attachment.
    let (_, name) = url.strip_prefix(DRAFT)?.split_once('/')?;
    let mut named = files.iter().filter(|file| kuvo_box::paths::segment(&file.name) == name);
    let file = named.next()?;
    named.next().is_none().then_some(file)
}

pub(super) fn anchors(text: &str, files: &[Attachment]) -> Vec<(Range<usize>, String)> {
    let mut anchors: Vec<_> = markdown::image_ranges(text).into_iter().filter(|(_, url)| url.starts_with("kuvo-attachment:")).collect();
    if !text.contains("[Image ") { return anchors; }
    let protected = markdown::protected_ranges(text);
    for (start, _) in text.match_indices("[Image ") {
        let number_start = start + "[Image ".len();
        let Some((number, _)) = text[number_start..].split_once(']') else { continue };
        if number.len() > 3 || number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) { continue; }
        let end = number_start + number.len() + 1;
        if text[..start].ends_with('\\') || protected.iter().any(|range| range.start < end && start < range.end) { continue; }
        let url = format!("{LEGACY}{number}");
        if resolve(&url, files).is_some() { anchors.push((start..end, url)); }
    }
    anchors.sort_by_key(|(range, _)| range.start);
    anchors
}

pub(super) fn legacy_markdown(url: &str) -> String {
    format!("![Attachment]({url})")
}

pub(super) fn is_legacy(url: &str) -> bool { url.starts_with(LEGACY) }

#[derive(Clone)]
pub(super) struct Cards {
    read: HashMap<String, Read>,
    queue: gpui::WeakEntity<QueueView>,
}

impl Cards {
    pub fn draw(&self, file: &Attachment, width: f32, height: f32, cx: &App) -> AnyElement {
        let c = &cx.theme().colors;
        let key = file.key();
        let dismiss = match file.origin { Origin::Draft(at, _) => Some(at), _ => None };
        let size = file.size;
        let fallback = matches!(self.read.get(&key), Some(Read::Failed { .. })).then(|| file.clone());
        let file = file.clone();
        let content = if file.image {
            let picture = file.picture();
            let open = self.queue.clone();
            let retry = self.queue.clone();
            let load = file.clone();
            let looks = Looks {
                read: &self.read, shown: &[],
                open: Rc::new(move |_, window, cx| { open.update(cx, |queue, cx| queue.open_picture(file.picture(), window, cx)).ok(); }),
                again: Rc::new(move |_, _, cx| { retry.update(cx, |queue, cx| queue.load_attachment(&load, true, cx)).ok(); }),
            };
            pictures::picture(&picture, width.max(40.), height, &looks, cx)
        } else {
            let queue = self.queue.clone();
            let name = file.name.clone();
            div().flex().items_center().gap(space::S2).max_w_full().min_w_0()
                .child(Button::new(SharedString::from(format!("save-{key}")), name).icon(IconName::Paperclip)
                    .size(ButtonSize::Small).on_click(move |_, _, cx| {
                        queue.update(cx, |queue, cx| queue.save_attachment(file.clone(), cx)).ok();
                    }))
                .children(size.map(|size| div().text_ui(TypeSize::Caption).text_color(c.text_muted).child(super::size_words(size))))
                .into_any_element()
        };
        let queue = self.queue.clone();
        div().debug_selector(move || format!("queue-attachment-{key}")).flex().items_start().gap(space::S2).min_w_0()
            .child(div().min_w_0().child(content).children(fallback.map(|file| {
                let queue = self.queue.clone();
                Button::new("save-image-file", "Save image").ghost().size(ButtonSize::Small)
                    .on_click(move |_, _, cx| { queue.update(cx, |queue, cx| queue.save_attachment(file.clone(), cx)).ok(); })
            })))
            .when_some(dismiss, |row, at| row.child(Button::icon_only(("remove-attachment", at), IconName::Close, "Remove attachment")
                .ghost().size(ButtonSize::Small).on_click(move |_, _, cx| {
                    queue.update(cx, |queue, cx| queue.body_composer.update(cx, |composer, cx| composer.detach(at, cx))).ok();
                })))
            .into_any_element()
    }
}

impl QueueView {
    pub(super) fn attachment_cards(&self, cx: &Context<Self>) -> Cards {
        Cards { read: self.picture_reads.clone(), queue: cx.entity().downgrade() }
    }

    pub(super) fn load_attachment(&mut self, file: &Attachment, retry: bool, cx: &mut Context<Self>) {
        let key = file.key();
        if !file.image || (!retry && self.picture_reads.contains_key(&key)) { return; }
        let origin = file.origin.clone();
        let name = file.name.clone();
        self.picture_reads.insert(key.clone(), Read::Reading);
        let request = format!("queue-picture-{key}");
        self.run(request, move |client| match origin {
            Origin::Draft(_, path) => pictures::read(client, &Source::Local(path)),
            Origin::Kept(item, _) => match client.work_item_file(&item, &name) {
                Ok(bytes) => pictures::shown(bytes),
                Err(_) => Read::Failed { why: "Couldn’t load the image. Check the computer’s connection and try again.".into(), again: true },
            },
        }, move |this, result, _| {
            // A sheet may have been dismissed while its picture was decoding.
            if let Some(slot) = this.picture_reads.get_mut(&key) { *slot = result; }
            if this.live { this.list.remeasure(); }
        }, cx);
    }

    pub(super) fn open_picture(&mut self, mut picture: Picture, window: &mut Window, cx: &mut Context<Self>) {
        self.viewer_return = window.focused(cx);
        picture.caption = None;
        self.viewing = Some(picture);
        window.focus(&self.viewer_focus, cx);
        cx.notify();
    }

    pub(super) fn close_picture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.viewing = None;
        if let Some(focus) = self.viewer_return.take() { window.focus(&focus, cx); }
        cx.notify();
    }

    fn save_attachment(&mut self, file: Attachment, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_new_path(&std::env::temp_dir(), Some(&file.name));
        let opened = self.sheet_opened;
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = picked.await else { return };
            let saved = cx.background_executor().spawn(async move {
                let bytes = match file.origin {
                    Origin::Draft(_, path) => std::fs::read(path).map_err(|_| ()),
                    Origin::Kept(item, _) => client.work_item_file(&item, &file.name).map_err(|_| ()),
                }?;
                std::fs::write(path, bytes).map_err(|_| ())
            }).await;
            if saved.is_err() {
                this.update(cx, |this, cx| {
                    let error = "Couldn’t save the attachment. Check the file and choose a writable folder, then try again.".into();
                    if this.sheet.is_some() && this.sheet_opened == opened { this.sheet_error = Some(error); } else { this.problem = Some(error); }
                    cx.notify();
                }).ok();
            }
        }).detach();
    }
}

pub(super) fn unavailable(cx: &App) -> AnyElement {
    div().p(space::S2).rounded(radius::NAV).bg(cx.theme().colors.sunken)
        .text_ui(TypeSize::Small).text_color(cx.theme().colors.text_muted)
        .child("Attachment unavailable").into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_editor_attachment_anchors_follow_files_after_a_draft_removal() {
        let first = draft_marker(5, "Review [wide].png");
        let other = draft_marker(7, "Notes.pdf");
        let body = format!("Start\n{other}\n{first}\nEnd");
        let first_file = PathBuf::from("Review [wide].png");
        let other_file = PathBuf::from("Notes.pdf");
        let (saved, paths) = for_save(&body, vec![(4, "Removed.jpg".into()), (5, first_file.clone()), (7, other_file.clone())]);
        assert_eq!(paths, [first_file, other_file]);
        assert_eq!(saved, format!("Start\n{}\n{}\nEnd", draft_marker(1, "Notes.pdf"), draft_marker(0, "Review [wide].png")));
    }
}
