//! Files: the session's folder as its box lists it, one folder at a time as each opens (`kuvo/folders`
//! with `files=1`), folders first; a folder the box can't read says why, with Try Again. A file opens
//! read-only, highlighted as the conversation's code blocks are, where the box says it's text
//! (`kuvo/file/text`).

use std::collections::{HashMap, HashSet};

use gpui::{
    AnyElement, App, Context, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyBinding, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, actions, div, prelude::FluentBuilder as _, px, uniform_list,
};
use kuvo_box::BoxClient;
use kuvo_box::folders::{FileText, Folder, Unreadable};
use kuvo_ui::code_block::CodeBlock;
use kuvo_ui::{ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TypeSize, theme::{radius, space}};

use super::changes::empty;
use super::{Load, Place, run};

actions!(files, [CloseFile]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", CloseFile, Some("Files"))]);
}

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

pub struct FilesView {
    client: BoxClient,
    live: bool,
    place: Option<Place>,
    shown: bool,
    folders: HashMap<String, Read>,
    open: HashSet<String>,
    opened: Option<(String, Load<Shown>)>,
    /// The tree's rows, worked out as folders are read, opened or closed (not on each frame).
    rows: std::rc::Rc<Vec<Row>>,
    focus: FocusHandle,
    /// The editor a file opens in (Your settings › General), where it's on this Mac.
    editor: Option<crate::yours::editors::Editor>,
    _yours: gpui::Subscription,
}

impl FilesView {
    pub fn new(client: BoxClient, live: bool, cx: &mut Context<Self>) -> FilesView {
        // (The editor picked changed: the button names the new one.)
        let yours = cx.observe_global::<crate::yours::Store>(|this: &mut FilesView, cx| {
            this.editor = crate::yours::editors::picked(cx);
            cx.notify();
        });
        FilesView { client, live, place: None, shown: false, folders: HashMap::new(), open: HashSet::new(), opened: None, rows: Default::default(), focus: cx.focus_handle(), editor: None, _yours: yours }
    }

    /// The editor "Open in" names, where one's picked and on this Mac.
    pub fn editor(&self) -> Option<&crate::yours::editors::Editor> {
        self.editor.as_ref()
    }

    pub fn opened(&self) -> Option<&(String, Load<Shown>)> {
        self.opened.as_ref()
    }

    pub fn set_place(&mut self, place: Option<Place>, shown: bool, cx: &mut Context<Self>) {
        let moved = self.place.as_ref().map(|place| &place.directory) != place.as_ref().map(|place| &place.directory);
        let showing = shown && !self.shown;
        self.place = place;
        self.shown = shown;
        if moved {
            self.folders.clear();
            self.open.clear();
            self.opened = None;
            self.rebuild();
        }
        // As it shows (again): its open folders read again, so what the agent made since is there.
        if (moved || showing) && shown {
            self.reload(cx);
        }
        cx.notify();
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

    /// Opens a file to read, as its box sends it. Image files present an option to open in the
    /// unified Stage viewer.
    pub fn open_file(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.opened = Some((path.to_owned(), Load::Loading));
        // (Looked up as a file opens: an editor installed or removed since is found.)
        if self.client.here() {
            self.editor = crate::yours::editors::picked(cx);
        }
        window.focus(&self.focus, cx);
        let client = self.client.clone();
        let asked = path.to_owned();
        let path = path.to_owned();
        run(self, self.live, cx, move || client.read_text(&asked), move |this, read, cx| {
            if this.opened.as_ref().map(|(at, _)| at) != Some(&path) {
                return;
            }
            let shown = match read {
                Ok(file) => Load::Ready(shown(file)),
                Err(error) if error.status() == Some(413) => Load::Ready(Shown::Unshown("Too big to show here.".into())),
                Err(error) => Load::Failed(error.to_string()),
            };
            this.opened = Some((path, shown));
            cx.notify();
        });
        cx.notify();
    }

    pub fn close_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.opened = None;
        window.focus(&self.focus, cx);
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

    fn row(&self, row: &Row, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let indent = |depth: usize| space::S2 + px(14. * depth as f32);
        let line = |id: String| {
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
        };
        match row {
            Row::Folder { path, name, depth, open } => {
                let toggled = path.clone();
                line(format!("tree-{path}"))
                    .pl(indent(*depth))
                    .cursor_pointer()
                    .hover(move |s| s.bg(c.hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(&toggled, cx)))
                    .child(Icon::new(if *open { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::Nav).color(c.text_faint))
                    .child(Icon::new(IconName::Folder).size(IconSize::Nav).color(c.text_muted))
                    .child(div().min_w_0().truncate().child(name.clone()))
                    .into_any_element()
            }
            Row::File { path, name, depth } => {
                let opened = path.clone();
                let selected = self.opened.as_ref().is_some_and(|(at, _)| at == path);
                let icon = if crate::image_viewer::is_image_path(name) { IconName::Image } else { IconName::File };
                line(format!("tree-{path}"))
                    .pl(indent(*depth) + px(18.))
                    .cursor_pointer()
                    .when(selected, |d| d.bg(c.selected))
                    .hover(move |s| s.bg(c.hover))
                    .on_click(cx.listener(move |this, _, window, cx| this.open_file(&opened, window, cx)))
                    .child(Icon::new(icon).size(IconSize::Nav).color(c.text_faint))
                    .child(div().min_w_0().truncate().child(name.clone()))
                    .into_any_element()
            }
            Row::Note { folder, depth, read } => {
                let note = |text: String| line(format!("note-{folder}")).pl(indent(*depth) + px(18.)).text_ui(TypeSize::Caption).text_color(c.text_faint).child(text);
                match read {
                    Read::Loading => line(format!("note-{folder}")).pl(indent(*depth) + px(18.)).child(kuvo_ui::small::LoadingLabel::new(SharedString::from(format!("tree-loading-{folder}")), "Reading…").compact()).into_any_element(),
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

    fn open_image_in_viewer(&self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        // Gather sibling image files in the same open folder so the user can step through them.
        let parent = path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
        let mut paths: Vec<String> = match self.folders.get(parent) {
            Some(Read::Listed(folder)) => folder
                .files
                .iter()
                .filter(|name| crate::image_viewer::is_image_path(name))
                .map(|name| join(parent, name))
                .collect(),
            _ => Vec::new(),
        };
        if !paths.iter().any(|p| p == path) {
            paths.push(path.to_owned());
        }
        let active = paths.iter().position(|p| p == path).unwrap_or(0);
        let here = self.client.here();
        let items: Vec<crate::image_viewer::ViewerItem> = paths
            .into_iter()
            .map(|file_path| {
                let name = file_path.rsplit('/').next().unwrap_or(&file_path).to_owned();
                let client = self.client.clone();
                let uri = crate::image_viewer::file_uri(&file_path);
                crate::image_viewer::ViewerItem {
                    id: format!("file:{file_path}"),
                    name,
                    caption: None,
                    source_tag: "Files".into(),
                    local_path: here.then(|| std::path::PathBuf::from(&file_path)),
                    read: crate::pictures::Read::Reading,
                    loader: Some(std::sync::Arc::new(move || {
                        crate::pictures::read(&client, &kuvo_box::pictures::Source::File(uri.clone()))
                    })),
                }
            })
            .collect();
        crate::image_viewer::open(
            crate::image_viewer::OpenRequest {
                items,
                active,
                return_focus: Some(self.focus.clone()),
                on_change: None,
            },
            window,
            cx,
        );
    }

    fn viewer(&self, path: &str, shown: &Load<Shown>, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let place = self.place.as_ref().map(|place| place.directory.clone()).unwrap_or_default();
        let relative = path.strip_prefix(&format!("{place}/")).unwrap_or(path).to_owned();
        let name = relative.rsplit('/').next().unwrap_or(&relative).to_owned();
        let folder = relative.rsplit_once('/').map(|(folder, _)| folder.to_owned()).unwrap_or_default();
        let is_image = crate::image_viewer::is_image_path(path);
        let body: AnyElement = match shown {
            Load::Loading => kuvo_ui::small::LoadingLabel::new("file-loading", "Reading…").fill().into_any_element(),
            Load::Failed(why) => div().p(space::S4).text_color(c.text_faint).child(format!("It couldn’t be read: {why}")).into_any_element(),
            Load::Ready(Shown::Unshown(_)) if is_image => {
                let open_path = path.to_owned();
                div()
                    .p(space::S5)
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(space::S3)
                    .rounded(radius::CARD)
                    .bg(c.sunken)
                    .border_1()
                    .border_color(c.border)
                    .child(Icon::new(IconName::Image).size(IconSize::Card).color(c.text_muted))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(2.))
                            .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child(name.clone()))
                            .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("Open full screen in the image viewer")),
                    )
                    .child(
                        div()
                            .debug_selector(|| "files-open-image".into())
                            .child(
                                Button::new("files-open-image", "View Image")
                                    .icon(IconName::ExpandEditor)
                                    .size(ButtonSize::Small)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_image_in_viewer(&open_path, window, cx);
                                    })),
                            ),
                    )
                    .into_any_element()
            }
            Load::Ready(Shown::Unshown(why)) => div().p(space::S4).text_color(c.text_faint).child(why.clone()).into_any_element(),
            Load::Ready(Shown::Text { text, lines }) => {
                text_preview(&name, text, *lines, cx)
            }
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(space::S1)
                    .h(px(40.))
                    .pl(space::S2)
                    .pr(space::S3)
                    .border_b_1()
                    .border_color(c.border)
                    .child(Button::icon_only("files-back", IconName::ChevronLeft, "Back to Files").ghost().size(ButtonSize::Small).icon_size(kuvo_ui::IconSize::Medium).on_click(cx.listener(|this, _, window, cx| this.close_file(window, cx))))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().medium().child(name))
                            .child(div().truncate().text_ui(TypeSize::Caption).text_color(c.text_faint).child(if folder.is_empty() { "Read only".to_owned() } else { format!("{folder} · Read only") })),
                    )
                    .children(self.open_in(path)),
            )
            .child(div().id("file-scroll").debug_selector(|| "file-viewer".into()).flex_1().min_h_0().overflow_y_scroll().p(space::S3).when(matches!(shown, Load::Loading), |d| d.flex().flex_col()).child(body))
            .into_any_element()
    }
}

impl FilesView {
    /// Opens the file in the editor picked in Your settings › General, else in its own app: only
    /// a file on this computer, which this Mac's apps can open.
    // BOX-NEEDED: another computer's file would need its contents fetched to a copy here
    // (`kuvo/file`), and an editor's remote opening; Files leaves the button out there.
    fn open_in(&self, path: &str) -> Option<Button> {
        if !self.client.here() {
            return None;
        }
        let (label, tooltip) = match &self.editor {
            Some(editor) => (format!("Open in {}", editor.name), None),
            None => ("Open".to_owned(), Some("In your default text editor. Pick another in Settings › General.")),
        };
        let (path, editor) = (path.to_owned(), self.editor.as_ref().map(|editor| editor.id.clone()));
        let button = Button::new("file-open-in", label).ghost().size(ButtonSize::Small).icon_after(IconName::External).on_click(move |_, _, cx| crate::yours::editors::open(std::path::Path::new(&path), editor.as_deref(), cx));
        Some(match tooltip {
            Some(tooltip) => button.tooltip(tooltip),
            None => button,
        })
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
        let content = if let Some((path, shown)) = self.opened.clone() {
            self.viewer(&path, &shown, cx)
        } else {
            let top = self.place.as_ref().map(|place| place.directory.clone()).unwrap_or_default();
            match self.folders.get(&top) {
                // The top folder unreadable: the whole room says so.
                Some(Read::Unreadable(Unreadable { message, reason })) => empty(
                    "Couldn’t Read This Folder",
                    reason.clone().unwrap_or_else(|| message.clone()),
                    Some(Button::new("files-again", "Try Again").size(ButtonSize::Small).on_click(cx.listener(move |this, _, _, cx| this.retry(&top, cx))).into_any_element()),
                    cx,
                ),
                _ => uniform_list("files-tree", self.rows.len(), cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                    let rows = this.rows.clone();
                    range.filter_map(|ix| rows.get(ix)).map(|row| this.row(row, cx)).collect::<Vec<_>>()
                }))
                .size_full()
                .p(space::S2)
                .into_any_element(),
            }
        };
        div()
            .key_context("Files")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &CloseFile, window, cx| {
                if this.opened.is_some() {
                    this.close_file(window, cx);
                } else {
                    cx.propagate();
                }
            }))
            .flex()
            .flex_col()
            .size_full()
            .child(content)
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
