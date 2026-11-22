//! A file tab: one file, read-only, highlighted as the conversation's code blocks are, where the
//! box says it's text (`kuvo/file/text`); its place in the folder above it, and the folder's files
//! beside it (hidden and shown by the button at its top right).

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use kuvo_box::BoxClient;
use kuvo_ui::code_block::CodeBlock;
use kuvo_ui::{ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TypeSize, theme::{radius, space}};

use super::files::{FilesView, LINES_MOST, Shown, shown};
use super::{Load, Place, run};

/// The tree's width beside a file.
pub const TREE_WIDTH: gpui::Pixels = px(220.);

pub struct FileView {
    client: BoxClient,
    live: bool,
    path: String,
    place: Option<Place>,
    on_screen: bool,
    read: Load<Shown>,
    /// The folder's files, shared with its other file tabs.
    tree: Entity<FilesView>,
    tree_shown: bool,
    /// The editor a file opens in (Your settings › General), where it's on this Mac.
    editor: Option<crate::yours::editors::Editor>,
    focus: FocusHandle,
    _yours: Subscription,
}

impl FileView {
    pub fn new(client: BoxClient, live: bool, path: String, tree: Entity<FilesView>, cx: &mut Context<Self>) -> FileView {
        // (The editor picked changed: the button names the new one.)
        let yours = cx.observe_global::<crate::yours::Store>(|this: &mut FileView, cx| {
            this.editor = crate::yours::editors::picked(cx);
            cx.notify();
        });
        let editor = if client.here() { crate::yours::editors::picked(cx) } else { None };
        FileView { client, live, path, place: None, on_screen: false, read: Load::Loading, tree, tree_shown: true, editor, focus: cx.focus_handle(), _yours: yours }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn read(&self) -> &Load<Shown> {
        &self.read
    }

    pub fn tree_shown(&self) -> bool {
        self.tree_shown
    }

    /// The editor "Open" names, where one's picked and on this Mac.
    pub fn editor(&self) -> Option<&crate::yours::editors::Editor> {
        self.editor.as_ref()
    }

    /// Its conversation's folder, and whether it's the tab on screen: as it shows (again), the file
    /// is read again, so what the agent changed since is there.
    pub fn set_place(&mut self, place: Option<Place>, on_screen: bool, cx: &mut Context<Self>) {
        let showing = on_screen && !self.on_screen;
        self.place = place;
        self.on_screen = on_screen;
        if showing {
            self.reload(cx);
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let asked = self.path.clone();
        run(self, self.live, cx, move || client.read_text(&asked), move |this, read, cx| {
            this.read = match read {
                Ok(file) => Load::Ready(shown(file)),
                Err(error) if error.status() == Some(413) => Load::Ready(Shown::Unshown("Too big to show here.".into())),
                Err(error) => Load::Failed(error.to_string()),
            };
            cx.notify();
        });
    }

    pub fn toggle_tree(&mut self, cx: &mut Context<Self>) {
        self.tree_shown = !self.tree_shown;
        cx.notify();
    }

    /// Where the file is: its folder's name, then each folder down to it, then its name.
    fn crumbs(&self) -> Vec<String> {
        let place = self.place.as_ref().map(|place| place.directory.trim_end_matches('/').to_owned());
        match place.as_deref().and_then(|place| self.path.strip_prefix(&format!("{place}/")).map(|inside| (place, inside))) {
            Some((place, inside)) => std::iter::once(place.rsplit('/').next().unwrap_or(place).to_owned()).chain(inside.split('/').map(str::to_owned)).collect(),
            // A file outside the folder: the end of its path.
            None => {
                let parts: Vec<&str> = self.path.trim_start_matches('/').split('/').collect();
                parts[parts.len().saturating_sub(3)..].iter().map(|part| (*part).to_owned()).collect()
            }
        }
    }

    /// Opens the file in the editor picked in Your settings › General, else in its own app: only
    /// a file on this computer, which this Mac's apps can open.
    // BOX-NEEDED: another computer's file would need its contents fetched to a copy here
    // (`kuvo/file`), and an editor's remote opening; a file tab leaves the button out there.
    fn open_in(&self) -> Option<Button> {
        if !self.client.here() {
            return None;
        }
        let (label, tooltip) = match &self.editor {
            Some(editor) => (format!("Open in {}", editor.name), None),
            None => ("Open".to_owned(), Some("In your default text editor. Pick another in Settings › General.")),
        };
        let (path, editor) = (self.path.clone(), self.editor.as_ref().map(|editor| editor.id.clone()));
        let button = Button::new("file-open-in", label).ghost().size(ButtonSize::Small).icon_after(IconName::External).on_click(move |_, _, cx| crate::yours::editors::open(std::path::Path::new(&path), editor.as_deref(), cx));
        Some(match tooltip {
            Some(tooltip) => button.tooltip(tooltip),
            None => button,
        })
    }

    fn open_image_in_viewer(&self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.path.clone();
        let client = self.client.clone();
        let uri = crate::image_viewer::file_uri(&path);
        let item = crate::image_viewer::ViewerItem {
            id: format!("file:{path}"),
            name: path.rsplit('/').next().unwrap_or(&path).to_owned(),
            caption: None,
            source_tag: "Files".into(),
            local_path: self.client.here().then(|| std::path::PathBuf::from(&path)),
            read: crate::pictures::Read::Reading,
            loader: Some(std::sync::Arc::new(move || crate::pictures::read(&client, &kuvo_box::pictures::Source::File(uri.clone())))),
        };
        crate::image_viewer::open(crate::image_viewer::OpenRequest { items: vec![item], active: 0, return_focus: Some(self.focus.clone()), on_change: None }, window, cx);
    }

    fn bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let crumbs = self.crumbs();
        let last = crumbs.len().saturating_sub(1);
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(space::S2)
            .h(px(36.))
            .pl(space::S3)
            .pr(space::S2)
            .border_b_1()
            .border_color(c.border)
            .child(
                div()
                    .id("file-crumbs")
                    .debug_selector(|| "file-crumbs".into())
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(space::S1_5)
                    .overflow_hidden()
                    .text_ui(TypeSize::Small)
                    .children(crumbs.into_iter().enumerate().flat_map(|(at, crumb)| {
                        let sep = (at > 0).then(|| div().flex_none().text_color(c.text_faint).child("/").into_any_element());
                        let word = if at == last {
                            div().min_w_0().truncate().medium().text_color(c.text).child(crumb).into_any_element()
                        } else {
                            div().flex_shrink_1().min_w(px(16.)).truncate().text_color(c.text_muted).child(crumb).into_any_element()
                        };
                        sep.into_iter().chain(std::iter::once(word))
                    })),
            )
            .children(self.open_in())
            .child(
                Button::icon_only("file-tree-toggle", IconName::PanelRight, if self.tree_shown { "Hide Files" } else { "Show Files" })
                    .ghost()
                    .size(ButtonSize::Small)
                    .selected(self.tree_shown)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_tree(cx))),
            )
            .into_any_element()
    }

    fn content(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let name = self.path.rsplit('/').next().unwrap_or(&self.path).to_owned();
        match &self.read {
            Load::Loading => kuvo_ui::small::LoadingLabel::new("file-loading", "Reading…").fill().into_any_element(),
            Load::Failed(why) => super::changes::empty("Couldn’t Read This File", why.clone(), Some(Button::new("file-again", "Try Again").size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.reload(cx))).into_any_element()), cx),
            Load::Ready(Shown::Unshown(_)) if crate::image_viewer::is_image_path(&self.path) => div()
                .flex()
                .flex_col()
                .flex_1()
                .items_center()
                .justify_center()
                .gap(space::S3)
                .p(space::S5)
                .child(Icon::new(IconName::Image).size(IconSize::Card).color(c.text_muted))
                .child(div().text_ui(TypeSize::Small).medium().child(name))
                .child(
                    div().debug_selector(|| "files-open-image".into()).child(
                        Button::new("files-open-image", "View Image")
                            .icon(IconName::ExpandEditor)
                            .size(ButtonSize::Small)
                            .on_click(cx.listener(|this, _, window, cx| this.open_image_in_viewer(window, cx))),
                    ),
                )
                .into_any_element(),
            Load::Ready(Shown::Unshown(why)) => super::changes::empty("Nothing to Show Here", why.clone(), None, cx),
            Load::Ready(Shown::Text { text, lines }) => {
                let cut = text.split_inclusive('\n').take(LINES_MOST).collect::<String>();
                let language = name.rsplit_once('.').map(|(_, extension)| extension.to_ascii_lowercase()).unwrap_or_default();
                div()
                    .id("file-scroll")
                    .debug_selector(|| "file-viewer".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(CodeBlock::new("file-code", cut).language(language).line_numbers(true).flush())
                    .when(*lines > LINES_MOST, |d| d.child(div().px(space::S4).pb(space::S4).text_ui(TypeSize::Caption).text_color(c.text_faint).child(format!("The first {LINES_MOST} of {lines} lines."))))
                    .into_any_element()
            }
        }
    }
}

impl Focusable for FileView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for FileView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        let path: SharedString = self.path.clone().into();
        self.tree.update(cx, |tree, cx| tree.set_selected(Some(path.to_string()), cx));
        div()
            .track_focus(&self.focus)
            .key_context("File")
            .size_full()
            .flex()
            .flex_col()
            .child(self.bar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(div().flex().flex_col().flex_1().min_w_0().child(self.content(cx)))
                    .when(self.tree_shown, |d| {
                        d.child(
                            div()
                                .debug_selector(|| "file-tree".into())
                                .w(TREE_WIDTH)
                                .flex_none()
                                .h_full()
                                .border_l_1()
                                .border_color(c.border)
                                .rounded(radius::SMALL)
                                .child(self.tree.clone()),
                        )
                    }),
            )
    }
}
