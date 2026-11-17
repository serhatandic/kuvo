//! Attachment anchors in a description, and the same file cards in the editor and on the page.

use std::{collections::{HashMap, HashSet}, ops::Range, path::PathBuf, rc::Rc};

use gpui::{AnyElement, App, Context, StatefulInteractiveElement, StyledImage as _, ObjectFit, InteractiveElement, IntoElement, ParentElement, SharedString, Styled, Window, div, img, px, prelude::FluentBuilder as _};
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
    pub fn image_height(&self, file: &Attachment, width: f32, cap: f32) -> f32 {
        match self.read.get(&file.key()) {
            Some(Read::Shown { width: w, height: h, .. }) => (width * *h as f32 / (*w).max(1) as f32).clamp(32., cap),
            _ => cap.min(144.),
        }
    }

    pub fn draw(&self, file: &Attachment, width: f32, cap: f32, remove: Option<(String, bool)>, cx: &App) -> AnyElement {
        let key = file.key();
        let picture = file.picture();
        let queue = self.queue.clone();
        if let Some(Read::Shown { image, thumb, .. }) = self.read.get(&key).filter(|_| file.image) {
            let group = SharedString::from(format!("queue-image-{key}"));
            let open = queue.clone();
            let expand = queue.clone();
            let expanded_picture = picture.clone();
            let copied = image.clone();
            let selector = format!("picture-{key}");
            let removal_file = file.clone();
            let tools = div().absolute().top(px(8.)).right(px(8.)).flex().items_center().p(px(2.))
                .rounded(radius::PILL).bg(gpui::black().opacity(0.76))
                .invisible().group_hover(group.clone(), |style| style.visible())
                .child(image_action("copy-queue-image", IconName::Copy, "Copy image", move |_, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::from((*copied).clone()));
                }))
                .child(image_action("expand-queue-image", IconName::ExpandEditor, "Open image", move |window, cx| {
                    expand.update(cx, |page, cx| page.open_picture(expanded_picture.clone(), window, cx)).ok();
                }))
                .when_some(remove, |tools, (url, editor)| tools.child(image_action("remove-queue-image", IconName::Trash, "Remove from description", move |_, cx| {
                    queue.update(cx, |page, cx| page.remove_picture_anchor(&removal_file, &url, editor, cx)).ok();
                })));
            return div().id(SharedString::from(selector.clone())).debug_selector(move || selector)
                .group(group).relative().w_full().h(px(self.image_height(file, width, cap))).rounded(px(8.))
                .overflow_hidden().cursor_pointer().bg(cx.theme().colors.sunken)
                .on_click(move |_, window, cx| {
                    open.update(cx, |page, cx| page.open_picture(picture.clone(), window, cx)).ok();
                })
                .child(img(thumb.clone()).size_full().object_fit(ObjectFit::Contain))
                .child(tools).into_any_element();
        }
        if file.image {
            let mut picture = picture;
            picture.caption = None;
            let load = file.clone();
            let retry = self.queue.clone();
            let looks = Looks {
                read: &self.read, shown: &[],
                open: Rc::new(|_, _, _| {}),
                again: Rc::new(move |_, _, cx| { retry.update(cx, |page, cx| page.load_attachment(&load, true, cx)).ok(); }),
            };
            let failed = matches!(self.read.get(&key), Some(Read::Failed { .. }));
            let file = file.clone();
            return div().child(pictures::picture(&picture, width, cap, &looks, cx))
                .when(failed, |d| d.child(Button::new("save-image-file", "Save image").ghost().size(ButtonSize::Small)
                    .on_click(move |_, _, cx| { queue.update(cx, |page, cx| page.save_attachment(file.clone(), cx)).ok(); })))
                .into_any_element();
        }
        let name = file.name.clone();
        let file = file.clone();
        let size = file.size;
        let pending = if let Origin::Draft(key, _) = file.origin { Some(key) } else { None };
        let remove_queue = queue.clone();
        div().id(SharedString::from(format!("file-chip-{key}"))).flex().items_center().gap(space::S1)
            .when_some(size, |d, size| d.tooltip(move |_, cx| kuvo_ui::menu::Tooltip::new(super::size_words(size)).build(cx)))
            .child(Button::new(SharedString::from(format!("save-{key}")), name).icon(IconName::Paperclip)
                .pill().size(ButtonSize::Small).on_click(move |_, _, cx| {
                    queue.update(cx, |page, cx| page.save_attachment(file.clone(), cx)).ok();
                }))
            .when_some(pending, |row, at| row.child(Button::icon_only(("remove-attachment", at), IconName::Close, "Remove attachment")
                .ghost().size(ButtonSize::Small).on_click(move |_, _, cx| {
                    remove_queue.update(cx, |queue, cx| queue.body_composer.update(cx, |composer, cx| composer.detach(at, cx))).ok();
                })))
            .into_any_element()
    }
}

fn image_action(id: &'static str, icon: IconName, label: &'static str, action: impl Fn(&mut Window, &mut App) + 'static) -> impl IntoElement {
    div().id(id).debug_selector(move || id.to_owned()).size(px(26.)).flex().items_center().justify_center().rounded(radius::PILL)
        .cursor_pointer().hover(|d| d.bg(gpui::white().opacity(0.16)))
        .tooltip(move |_, cx| kuvo_ui::menu::Tooltip::new(label).build(cx))
        .on_click(move |_, window, cx| { cx.stop_propagation(); action(window, cx); })
        .child(kuvo_ui::Icon::new(icon).color(gpui::white().opacity(0.9)))
}

impl QueueView {
    fn remove_picture_anchor(&mut self, file: &Attachment, url: &str, editor: bool, cx: &mut Context<Self>) {
        if editor {
            if self.saving { return; }
            if let Origin::Draft(key, _) = file.origin {
                self.body_composer.update(cx, |composer, cx| composer.detach(key, cx));
                return;
            }
        }
        let mut body = if editor { self.body.read(cx).text().to_owned() }
            else if let Some(whole) = self.whole().filter(|whole| whole.item.can.edit && !whole.told) { whole.body.clone() }
            else { return };
        let files = self.whole().map(kept).unwrap_or_default();
        let Some((range, _)) = anchors(&body, &files).into_iter().find(|(_, anchor)| anchor == url) else { return };
        if editor {
            self.body.update(cx, |field, cx| {
                field.select_range(range, cx);
                field.replace_selection("", cx);
            });
        } else if let Origin::Kept(item, _) = &file.origin {
            body.replace_range(range, "");
            self.act(super::Act::Set { item: item.clone(), change: Box::new(kuvo_box::work_queue::ItemChange { body: Some(body), ..Default::default() }) }, cx);
        }
    }

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
