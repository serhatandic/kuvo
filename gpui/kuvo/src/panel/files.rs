//! Files: the conversation's folder as its box lists it, one folder at a time as each opens
//! (`kuvo/folders` with `files=1`), folders first; a folder the box can't read says why, with Try
//! Again. Above it, "Filter files…" asks the box for the files whose path has what's typed
//! (`kuvo/files/find`), best first as it ranks them. A file picked opens in a tab of its own.

use std::collections::{HashMap, HashSet};

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div, prelude::FluentBuilder as _, px, uniform_list,
};
use kuvo_box::BoxClient;
use kuvo_box::find::{FILES_FIND, Found};
use kuvo_box::folders::{FileText, Folder, Unreadable};
use kuvo_ui::code_block::CodeBlock;
use kuvo_ui::text_field::TextFieldEvent;
use kuvo_ui::{ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TextField, TypeSize, theme::{radius, space}};

use super::changes::empty;
use super::{Load, Place, run};

/// The most lines a file shows: past it, the rest is counted.
pub const LINES_MOST: usize = 3_000;

/// What a folder's read gave.
#[derive(Clone, Debug, PartialEq)]
pub enum Read {
    Loading,
    Listed(Folder),
    Unreadable(Unreadable),
    Failed(String),
}

/// A file opened to read.
#[derive(Clone, Debug, PartialEq)]
pub enum Shown {
    Text { text: String, lines: usize },
    /// Not text (a picture, a binary), or too big: said instead.
    Unshown(String),
}

/// One line of the tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Folder { path: String, name: String, depth: usize, open: bool },
    File { path: String, name: String, depth: usize },
    /// An open folder's state in its files' place: reading, why it can't be read, or empty.
    Note { folder: String, depth: usize, read: Read },
}

/// A file picked in the tree, or among what the filter found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenFile(pub String);

pub struct FilesView {
    client: BoxClient,
    live: bool,
    place: Option<Place>,
    shown: bool,
    folders: HashMap<String, Read>,
    open: HashSet<String>,
    /// The tree's rows, worked out as folders are read, opened or closed (not on each frame).
    rows: std::rc::Rc<Vec<Row>>,
    /// The file showing in its tab, marked in the tree.
    selected: Option<String>,
    filter: Entity<TextField>,
    /// What the box found for the filter's text, and the text it was asked.
    found: Option<(String, Load<Found>)>,
    focus: FocusHandle,
    _filter: Subscription,
}

impl EventEmitter<OpenFile> for FilesView {}

impl FilesView {
    pub fn new(client: BoxClient, live: bool, cx: &mut Context<Self>) -> FilesView {
        let filter = cx.new(|cx| TextField::new(cx).bare().placeholder("Filter files…"));
        let watch = cx.subscribe(&filter, |this: &mut FilesView, _, event: &TextFieldEvent, cx| match event {
            TextFieldEvent::Changed => this.find(cx),
            TextFieldEvent::Submit(_) => {
                if let Some(first) = this.found().and_then(|found| found.files.first()).map(|file| file.path.clone()) {
                    cx.emit(OpenFile(first));
                }
            }
            TextFieldEvent::Cancel => this.filter.update(cx, |field, cx| field.set_text("", cx)),
        });
        FilesView { client, live, place: None, shown: false, folders: HashMap::new(), open: HashSet::new(), rows: Default::default(), selected: None, filter, found: None, focus: cx.focus_handle(), _filter: watch }
    }

    pub fn filter(&self) -> &Entity<TextField> {
        &self.filter
    }

    /// What the box found for the filter's text, once it's said.
    pub fn found(&self) -> Option<&Found> {
        self.found.as_ref().and_then(|(_, found)| found.ready())
    }

    pub fn set_place(&mut self, place: Option<Place>, shown: bool, cx: &mut Context<Self>) {
        let moved = self.place.as_ref().map(|place| &place.directory) != place.as_ref().map(|place| &place.directory);
        let showing = shown && !self.shown;
        self.place = place;
        self.shown = shown;
        if moved {
            self.folders.clear();
            self.open.clear();
            self.found = None;
            self.rebuild();
        }
        // As it shows (again): its open folders read again, so what the agent made since is there.
        if (moved || showing) && shown {
            self.reload(cx);
        }
        cx.notify();
    }

    /// Marks the file showing in its tab.
    pub fn set_selected(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        if self.selected != path {
            self.selected = path;
            cx.notify();
        }
    }

    /// Reads the top folder, and each open one, again.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(place) = self.place.clone() else { return };
        let mut folders: Vec<String> = self.open.iter().cloned().collect();
        folders.sort();
        for folder in std::iter::once(place.directory).chain(folders) {
            self.read(folder, cx);
        }
    }

    fn read(&mut self, folder: String, cx: &mut Context<Self>) {
        // What showed stays while it's read again.
        self.folders.entry(folder.clone()).or_insert(Read::Loading);
        self.rebuild();
        let client = self.client.clone();
        let asked = folder.clone();
        run(self, self.live, cx, move || client.folder(&asked), move |this, read, cx| {
            let read = match read {
                Ok(listed) => Read::Listed(listed),
                Err(Ok(unreadable)) => Read::Unreadable(unreadable),
                Err(Err(error)) => Read::Failed(error.to_string()),
            };
            this.folders.insert(folder, read);
            this.rebuild();
            cx.notify();
        });
    }

    /// Asks the box for the files whose path has the filter's text.
    fn find(&mut self, cx: &mut Context<Self>) {
        let query = self.filter.read(cx).text().trim().to_owned();
        let Some(place) = self.place.clone().filter(|_| !query.is_empty()) else {
            self.found = None;
            cx.notify();
            return;
        };
        self.found = Some((query.clone(), Load::Loading));
        let client = self.client.clone();
        let asked = query.clone();
        run(self, self.live, cx, move || client.find_files(&place.directory, &asked), move |this, found, cx| {
            // (Only the answer to what's typed now.)
            if this.found.as_ref().map(|(at, _)| at) != Some(&query) {
                return;
            }
            this.found = Some((query, match found {
                Ok(found) => Load::Ready(found),
                Err(error) => Load::Failed(error.to_string()),
            }));
            cx.notify();
        });
        cx.notify();
    }

    /// Opens or closes a folder; opened the first time, it's read.
    pub fn toggle(&mut self, folder: &str, cx: &mut Context<Self>) {
        if !self.open.remove(folder) {
            self.open.insert(folder.to_owned());
            if !matches!(self.folders.get(folder), Some(Read::Listed(_))) {
                self.read(folder.to_owned(), cx);
            }
        }
        self.rebuild();
        cx.notify();
    }

    /// Reads a folder that couldn't be read again.
    pub fn retry(&mut self, folder: &str, cx: &mut Context<Self>) {
        self.folders.insert(folder.to_owned(), Read::Loading);
        self.read(folder.to_owned(), cx);
        cx.notify();
    }

    /// The tree as rows: each open folder's folders, then its files, under it.
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        if let Some(place) = &self.place {
            self.walk(&place.directory, 0, &mut rows);
        }
        self.rows = std::rc::Rc::new(rows);
    }

    fn walk(&self, folder: &str, depth: usize, rows: &mut Vec<Row>) {
        match self.folders.get(folder) {
            Some(Read::Listed(listed)) => {
                if listed.folders.is_empty() && listed.files.is_empty() {
                    rows.push(Row::Note { folder: folder.to_owned(), depth, read: Read::Listed(listed.clone()) });
                }
                for named in &listed.folders {
                    let path = named.path.clone();
                    let open = self.open.contains(&path);
                    rows.push(Row::Folder { path: path.clone(), name: named.name.clone(), depth, open });
                    if open {
                        self.walk(&path, depth + 1, rows);
                    }
                }
                for file in &listed.files {
                    rows.push(Row::File { path: file.path.clone(), name: file.name.clone(), depth });
                }
            }
            Some(read) => rows.push(Row::Note { folder: folder.to_owned(), depth, read: read.clone() }),
            None => rows.push(Row::Note { folder: folder.to_owned(), depth, read: Read::Loading }),
        }
    }

    fn line(id: String) -> gpui::Stateful<gpui::Div> {
        let selector = id.clone();
        div()
            .id(SharedString::from(id))
            .debug_selector(move || selector)
            .flex()
            .w_full()
            .items_center()
            .gap(space::S1_5)
            .h(px(26.))
            .pr(space::S2)
            .rounded(radius::NAV)
    }

    fn row(&self, row: &Row, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let glass = super::glass(cx);
        let indent = |depth: usize| space::S2 + px(14. * depth as f32);
        match row {
            Row::Folder { path, name, depth, open } => {
                let toggled = path.clone();
                Self::line(format!("tree-{path}"))
                    .pl(indent(*depth))
                    .cursor_pointer()
                    .hover(move |s| s.bg(c.hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(&toggled, cx)))
                    .child(Icon::new(if *open { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::XSmall).color(c.text_faint))
                    .child(div().min_w_0().truncate().child(name.clone()))
                    .into_any_element()
            }
            Row::File { path, name, depth } => {
                let opened = path.clone();
                let selected = self.selected.as_ref() == Some(path);
                let icon = if crate::image_viewer::is_image_path(name) { IconName::Image } else { IconName::File };
                Self::line(format!("tree-{path}"))
                    .pl(indent(*depth) + px(18.))
                    .cursor_pointer()
                    .when(selected, |d| d.bg(glass.pill).medium())
                    .when(!selected, |d| d.hover(move |s| s.bg(c.hover)))
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(OpenFile(opened.clone()))))
                    .child(Icon::new(icon).size(IconSize::XSmall).color(c.text_faint))
                    .child(div().min_w_0().truncate().child(name.clone()))
                    .into_any_element()
            }
            Row::Note { folder, depth, read } => {
                let note = |text: String| Self::line(format!("note-{folder}")).pl(indent(*depth) + px(18.)).text_ui(TypeSize::Caption).text_color(c.text_faint).child(text);
                match read {
                    Read::Loading => Self::line(format!("note-{folder}")).pl(indent(*depth) + px(18.)).child(kuvo_ui::small::LoadingLabel::new(SharedString::from(format!("tree-loading-{folder}")), "Reading…").compact()).into_any_element(),
                    Read::Listed(_) => note("Empty".into()).into_any_element(),
                    Read::Unreadable(Unreadable { reason, message }) => self.unreadable(folder, *depth, reason.clone().unwrap_or_else(|| message.clone()), cx),
                    Read::Failed(why) => self.unreadable(folder, *depth, why.clone(), cx),
                }
            }
        }
    }

    /// An open folder the box couldn't read, in its files' place: why, as the computer says it, and
    /// Try Again, so it never looks empty (one line, as every row of the tree is; the whole reason
    /// on hover).
    fn unreadable(&self, folder: &str, depth: usize, why: String, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let again = folder.to_owned();
        let selector = format!("unreadable-{folder}");
        div()
            .id(SharedString::from(format!("unreadable-{folder}")))
            .debug_selector(move || selector)
            .flex()
            .w_full()
            .items_center()
            .gap(space::S1_5)
            .h(px(26.))
            .pl(space::S2 + px(14. * depth as f32) + px(18.))
            .pr(space::S1)
            .text_ui(TypeSize::Caption)
            .tooltip({
                let said: SharedString = format!("Couldn’t read this folder: {why}").into();
                move |_, cx| kuvo_ui::menu::Tooltip::new(said.clone()).build(cx)
            })
            .child(Icon::new(IconName::Warning).size(IconSize::Nav).color(c.text_faint))
            .child(div().flex_1().min_w_0().truncate().text_color(c.text_muted).child(format!("Couldn’t read it: {why}")))
            .child(Button::new(SharedString::from(format!("retry-{folder}")), "Try Again").ghost().size(ButtonSize::Small).on_click(cx.listener(move |this, _, _, cx| this.retry(&again, cx))))
            .into_any_element()
    }

    /// "Filter files…", where the box finds files.
    fn filter_box(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.client.gate().has(FILES_FIND) {
            return None;
        }
        let c = cx.theme().colors.clone();
        Some(
            div()
                .debug_selector(|| "files-filter".into())
                .flex()
                .flex_none()
                .items_center()
                .gap(space::S1_5)
                .h(px(30.))
                .mx(space::S2)
                .mt(space::S2)
                .mb(space::S1)
                .px(space::S2)
                .rounded(radius::NAV)
                .border_1()
                .border_color(c.border)
                .bg(c.card)
                .child(Icon::new(IconName::Search).size(IconSize::XSmall).color(c.text_faint))
                .child(div().flex_1().min_w_0().text_ui(TypeSize::Small).child(self.filter.clone()))
                .into_any_element(),
        )
    }

    /// What the box found for the filter: each file with the folder it's in, best first.
    fn found_list(&self, query: &str, found: &Load<Found>, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let glass = super::glass(cx);
        match found {
            Load::Loading => div().p(space::S3).child(kuvo_ui::small::LoadingLabel::new("files-finding", "Finding…").compact()).into_any_element(),
            Load::Failed(why) => div()
                .debug_selector(|| "files-find-failed".into())
                .flex()
                .items_center()
                .gap(space::S2)
                .p(space::S3)
                .text_ui(TypeSize::Caption)
                .child(div().flex_1().min_w_0().text_color(c.text_muted).child(format!("Couldn’t search: {why}")))
                .child(Button::new("files-find-again", "Try Again").ghost().size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.find(cx))))
                .into_any_element(),
            Load::Ready(found) if found.files.is_empty() => div()
                .debug_selector(|| "files-found-none".into())
                .p(space::S3)
                .text_ui(TypeSize::Caption)
                .text_color(c.text_faint)
                .child(format!("No files match “{query}”."))
                .into_any_element(),
            Load::Ready(found) => div()
                .id("files-found")
                .debug_selector(|| "files-found".into())
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(space::S2)
                .pb(space::S2)
                .children(found.files.iter().map(|file| {
                    let opened = file.path.clone();
                    let selected = self.selected.as_ref() == Some(&file.path);
                    Self::line(format!("found-{}", file.path))
                        .pl(space::S2)
                        .cursor_pointer()
                        .when(selected, |d| d.bg(glass.pill))
                        .when(!selected, |d| d.hover(move |s| s.bg(c.hover)))
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(OpenFile(opened.clone()))))
                        .child(Icon::new(IconName::File).size(IconSize::XSmall).color(c.text_faint))
                        .child(div().flex_none().max_w(px(160.)).truncate().child(file.name.clone()))
                        .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Caption).text_color(c.text_faint).child(file.folder.clone()))
                }))
                .into_any_element(),
        }
    }
}

/// The read-only text viewer, also used for recorded conversation outputs.
pub(crate) fn text_preview(name: &str, text: &str, lines: usize, cx: &App) -> AnyElement {
    let faint = cx.theme().colors.text_faint;
    let cut = text.split_inclusive('\n').take(LINES_MOST).collect::<String>();
    let language = name.rsplit_once('.').map(|(_, extension)| extension.to_ascii_lowercase()).unwrap_or_default();
    div()
        .flex()
        .flex_col()
        .gap(space::S2)
        .child(CodeBlock::new("file-code", cut).language(language).line_numbers(true))
        .when(lines > LINES_MOST, |d| d.child(div().text_ui(TypeSize::Caption).text_color(faint).child(format!("The first {LINES_MOST} of {lines} lines."))))
        .into_any_element()
}

/// A file as shown: its text where the box says it's text, else that it isn't.
pub fn shown(file: FileText) -> Shown {
    if !file.text {
        return Shown::Unshown("Not a text file: nothing to show here.".into());
    }
    let lines = file.content.lines().count();
    Shown::Text { text: file.content, lines }
}

impl Focusable for FilesView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for FilesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let top = self.place.as_ref().map(|place| place.directory.clone()).unwrap_or_default();
        let content = match (self.found.clone(), self.folders.get(&top)) {
            (Some((query, found)), _) => self.found_list(&query, &found, cx),
            // The top folder unreadable: the whole room says so.
            (None, Some(Read::Unreadable(Unreadable { message, reason }))) => empty(
                "Couldn’t Read This Folder",
                reason.clone().unwrap_or_else(|| message.clone()),
                Some(Button::new("files-again", "Try Again").size(ButtonSize::Small).on_click(cx.listener(move |this, _, _, cx| this.retry(&top, cx))).into_any_element()),
                cx,
            ),
            (None, _) => uniform_list("files-tree", self.rows.len(), cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                let rows = this.rows.clone();
                range.filter_map(|ix| rows.get(ix)).map(|row| this.row(row, cx)).collect::<Vec<_>>()
            }))
            .flex_1()
            .min_h_0()
            .px(space::S2)
            .pb(space::S2)
            .into_any_element(),
        };
        div()
            .key_context("Files")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .children(self.filter_box(cx))
            .child(div().flex().flex_col().flex_1().min_h_0().child(content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_shows_and_what_the_box_says_isnt_text_says_so() {
        assert_eq!(shown(FileText { text: true, content: "one\ntwo\n".into() }), Shown::Text { text: "one\ntwo\n".into(), lines: 2 });
        assert!(matches!(shown(FileText { text: false, content: String::new() }), Shown::Unshown(_)));
    }
}
