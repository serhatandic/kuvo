//! Changes: the folder's changed files as the box lists them (conflicts, staged, unstaged, new),
//! followed live, each under its own header with its diff below it, the first few open and the
//! rest a click away. Stage, unstage and discard (with a confirm) send back what the box listed; a
//! commit takes what's staged, or everything when nothing is, as the box says. The branch it's on
//! opens Branches in a tab of its own.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyBinding,
    ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Subscription, Task, Window, actions, div,
    prelude::FluentBuilder as _, px,
};
use kuvo_box::follow::{Follow, Followed, Timing};
use kuvo_box::git::{ChangedFile, Committed, DiscardKind, FileDiff, GitFiles, Group, Stage};
use kuvo_box::BoxClient;
use kuvo_ui::output::{DiffLine, DiffView, parse_patch};
use kuvo_ui::small::Checkbox;
use kuvo_ui::{ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TextField, TextFieldEvent, TypeSize, theme::{radius, space}};

use super::{Load, Place, run};

actions!(changes, [Commit, Back]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("cmd-enter", Commit, Some("Changes")), KeyBinding::new("escape", Back, Some("Changes"))]);
}

/// How many of the first files show open as the tab opens; the rest open on a click.
pub const OPEN_FIRST: usize = 3;

/// A file opened to its diff.
pub struct Opened {
    pub file: ChangedFile,
    pub diff: Load<Rc<Vec<DiffLine>>>,
    /// Too big to show, or binary: said instead of its lines.
    pub unshown: Option<&'static str>,
    /// The unchanged stretches opened, by where they start.
    pub expanded: Rc<HashSet<usize>>,
}

/// What the last commit said, when it didn't simply go.
#[derive(Clone, Debug, PartialEq)]
pub enum CommitSaid {
    Failed(String),
    /// It landed, but its push didn't: Push is offered, never the commit again.
    NotPushed(String),
    Pushed,
}

/// What the Changes tab asks of the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangesEvent {
    /// Its branch was clicked: Branches, in a tab.
    ShowBranches,
}

pub struct ChangesView {
    client: BoxClient,
    live: bool,
    place: Option<Place>,
    shown: bool,
    files: Load<GitFiles>,
    /// Reading again failed after a first read: what was read stays, with this.
    stale: Option<String>,
    /// Each file showing its diff, by its key.
    opened: HashMap<String, Opened>,
    /// Files folded or opened by hand (else the first few show open), by their keys.
    chosen: HashMap<String, bool>,
    /// The file whose discard waits for its confirm.
    confirming: Option<ChangedFile>,
    /// Files with an action on its way, by their keys.
    busy: HashSet<String>,
    /// What the box said of the last action that didn't go.
    failed: Option<String>,
    message: Entity<TextField>,
    push: bool,
    committing: bool,
    said: Option<CommitSaid>,
    scroll: ScrollHandle,
    focus: FocusHandle,
    _follow: Option<Follow<Followed<GitFiles>>>,
    _follow_task: Option<Task<()>>,
    _message: Subscription,
}

impl EventEmitter<ChangesEvent> for ChangesView {}

impl ChangesView {
    pub fn new(client: BoxClient, live: bool, cx: &mut Context<Self>) -> ChangesView {
        let message = cx.new(|cx| TextField::new(cx).placeholder("Commit message").multi_line(1, 5).submits_on_enter(false));
        let subscription = cx.subscribe(&message, |this: &mut ChangesView, _, event: &TextFieldEvent, cx| {
            if matches!(event, TextFieldEvent::Changed) {
                if matches!(this.said, Some(CommitSaid::Failed(_))) {
                    this.said = None;
                }
                cx.notify();
            }
        });
        ChangesView {
            client,
            live,
            place: None,
            shown: false,
            files: Load::Loading,
            stale: None,
            opened: HashMap::new(),
            chosen: HashMap::new(),
            confirming: None,
            busy: HashSet::new(),
            failed: None,
            message,
            push: false,
            committing: false,
            said: None,
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            _follow: None,
            _follow_task: None,
            _message: subscription,
        }
    }

    pub fn files(&self) -> &Load<GitFiles> {
        &self.files
    }

    /// The file `key` names, as it shows open: its diff, or that it's on its way.
    pub fn opened(&self, key: &str) -> Option<&Opened> {
        self.opened.get(key)
    }

    pub fn message(&self) -> &Entity<TextField> {
        &self.message
    }

    pub fn said(&self) -> Option<&CommitSaid> {
        self.said.as_ref()
    }

    pub fn failed(&self) -> Option<&str> {
        self.failed.as_deref()
    }

    /// The folder, and whether the view is on screen: followed while it is, let go when it isn't.
    pub fn set_place(&mut self, place: Option<Place>, shown: bool, cx: &mut Context<Self>) {
        let moved = self.place.as_ref().map(|place| &place.directory) != place.as_ref().map(|place| &place.directory);
        if moved {
            self.files = Load::Loading;
            self.opened.clear();
            self.chosen.clear();
            self.confirming = None;
            self.failed = None;
            self.said = None;
            self.busy.clear();
        }
        let again = moved || shown != self.shown;
        self.place = place;
        self.shown = shown;
        if again {
            self.follow(cx);
        }
        cx.notify();
    }

    /// Follows the folder's files while on screen (the app), or reads them once (tests and shots).
    fn follow(&mut self, cx: &mut Context<Self>) {
        self._follow = None;
        self._follow_task = None;
        let Some(place) = self.place.clone().filter(|_| self.shown) else { return };
        if !self.live {
            self.read_again(cx);
            return;
        }
        let follow = self.client.follow_git_files(&place.directory, Timing::default());
        let updates = follow.updates().clone();
        self._follow = Some(follow);
        self._follow_task = Some(cx.spawn(async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                let taken = this.update(cx, |this, cx| match update {
                    Followed::Now(files) => this.take(Ok(files), cx),
                    Followed::Failed(error) => this.take(Err(error), cx),
                });
                if taken.is_err() {
                    break;
                }
            }
        }));
    }

    /// Reads the folder's files again now (after an action, so it shows at once).
    pub fn read_again(&mut self, cx: &mut Context<Self>) {
        let Some(place) = self.place.clone() else { return };
        let client = self.client.clone();
        let directory = place.directory.clone();
        let asked = directory.clone();
        run(self, self.live, cx, move || client.git_files(&asked), move |this, read, cx| {
            if this.place.as_ref().map(|place| &place.directory) == Some(&directory) {
                this.take(read, cx);
            }
        });
    }

    /// Whether the file at `at` in the box's list shows open.
    fn is_open(&self, at: usize, file: &ChangedFile) -> bool {
        self.chosen.get(&file.key()).copied().unwrap_or(at < OPEN_FIRST)
    }

    /// What the box said of the folder's files: each file showing open has its diff read (again,
    /// where the file changed since: an agent's edit), its lines kept meanwhile; a file gone from
    /// the list (committed, discarded) is let go.
    pub fn take(&mut self, read: kuvo_box::Result<GitFiles>, cx: &mut Context<Self>) {
        match read {
            Ok(files) => {
                let keys: HashSet<String> = files.files.iter().map(ChangedFile::key).collect();
                // A file that moved group (staged, unstaged) shows as it did.
                let moved: Vec<(String, String, bool)> = self
                    .chosen
                    .iter()
                    .filter(|(key, _)| !keys.contains(*key))
                    .filter_map(|(key, open)| {
                        let path = key.split_once(':').map(|(_, path)| path)?;
                        files.files.iter().find(|file| file.path == path).map(|file| (key.clone(), file.key(), *open))
                    })
                    .collect();
                for (old, new, open) in moved {
                    self.chosen.remove(&old);
                    self.chosen.entry(new).or_insert(open);
                }
                self.chosen.retain(|key, _| keys.contains(key));
                self.opened.retain(|key, _| keys.contains(key));
                // A discard waiting for its confirm is for the file as it was shown: changed or gone
                // since, it's put away (the box would refuse it too).
                if let Some(confirming) = &self.confirming
                    && !files.files.contains(confirming)
                {
                    self.confirming = None;
                }
                let wanted: Vec<ChangedFile> = files
                    .files
                    .iter()
                    .enumerate()
                    .filter(|(at, file)| self.is_open(*at, file))
                    .filter(|(_, file)| self.opened.get(&file.key()).is_none_or(|opened| &opened.file != *file))
                    .map(|(_, file)| file.clone())
                    .collect();
                self.files = Load::Ready(files);
                self.stale = None;
                for file in wanted {
                    self.load(file, cx);
                }
            }
            Err(error) => match self.files {
                Load::Ready(_) => self.stale = Some(error.to_string()),
                _ => self.files = Load::Failed(error.to_string()),
            },
        }
        cx.notify();
    }

    fn root(&self) -> Option<String> {
        self.files.ready().and_then(|files| files.root.clone())
    }

    /// Shows `file` open, its diff read as the box sends it.
    pub fn open(&mut self, file: ChangedFile, cx: &mut Context<Self>) {
        let at = self.files.ready().and_then(|files| files.files.iter().position(|listed| listed.key() == file.key()));
        if at.is_some() {
            self.chosen.insert(file.key(), true);
        }
        if !self.opened.contains_key(&file.key()) {
            self.load(file, cx);
        }
        cx.notify();
    }

    /// Folds an open file, or opens a folded one.
    pub fn toggle(&mut self, file: ChangedFile, cx: &mut Context<Self>) {
        let at = self.files.ready().and_then(|files| files.files.iter().position(|listed| listed.key() == file.key()));
        let Some(at) = at else { return };
        if self.is_open(at, &file) {
            self.chosen.insert(file.key(), false);
            cx.notify();
        } else {
            self.open(file, cx);
        }
    }

    /// Reads `file`'s diff, as the box lists it now: what showed stays until it's in, and the
    /// stretches opened stay open.
    fn load(&mut self, file: ChangedFile, cx: &mut Context<Self>) {
        let Some(root) = self.root() else { return };
        let key = file.key();
        match self.opened.get_mut(&key) {
            Some(opened) => opened.file = file.clone(),
            None => {
                self.opened.insert(key.clone(), Opened { file: file.clone(), diff: Load::Loading, unshown: None, expanded: Rc::default() });
            }
        }
        let client = self.client.clone();
        run(self, self.live, cx, move || client.file_diff(&root, &file), move |this, diff, cx| {
            let Some(opened) = this.opened.get_mut(&key) else { return };
            match diff {
                Ok(FileDiff { big: true, .. }) => {
                    opened.diff = Load::Ready(Rc::default());
                    opened.unshown = Some("Too big to show here.");
                }
                // Binary, as the box says: no lines to show.
                Ok(FileDiff { binary: true, .. }) => {
                    opened.diff = Load::Ready(Rc::default());
                    opened.unshown = Some("A binary file: no lines to show.");
                }
                Ok(FileDiff { patch, .. }) => {
                    opened.unshown = None;
                    opened.diff = Load::Ready(Rc::new(parse_patch(&patch)));
                }
                Err(error) => opened.diff = Load::Failed(error.to_string()),
            }
            cx.notify();
        });
    }

    /// Opens an unchanged stretch of `key`'s diff.
    pub fn expand(&mut self, key: &str, start: usize, cx: &mut Context<Self>) {
        if let Some(opened) = self.opened.get_mut(key) {
            let mut expanded = (*opened.expanded).clone();
            expanded.insert(start);
            opened.expanded = Rc::new(expanded);
            cx.notify();
        }
    }

    /// Stages, unstages or discards `file`, as the box listed it, then reads the list again.
    pub fn act(&mut self, action: Action, file: ChangedFile, cx: &mut Context<Self>) {
        let Some(root) = self.root() else { return };
        if action == Action::Discard {
            self.confirming = None;
        }
        let key = file.key();
        self.busy.insert(key.clone());
        self.failed = None;
        let client = self.client.clone();
        run(self, self.live, cx, move || match action {
            Action::Stage => client.stage(&root, &file),
            Action::Unstage => client.unstage(&root, &file),
            Action::Discard => client.discard(&root, &file),
        }, move |this, done, cx| {
            this.busy.remove(&key);
            if let Err(error) = done {
                this.failed = Some(error.to_string());
            }
            this.read_again(cx);
            cx.notify();
        });
        cx.notify();
    }

    /// Asks before discarding `file`.
    pub fn confirm_discard(&mut self, file: ChangedFile, cx: &mut Context<Self>) {
        self.confirming = Some(file);
        cx.notify();
    }

    pub fn cancel_discard(&mut self, cx: &mut Context<Self>) {
        self.confirming = None;
        cx.notify();
    }

    pub fn set_push(&mut self, push: bool, cx: &mut Context<Self>) {
        self.push = push;
        cx.notify();
    }

    /// Commits with the message written (and pushes, when asked), on the folder's box.
    pub fn commit(&mut self, cx: &mut Context<Self>) {
        let message = self.message.read(cx).text().trim().to_owned();
        let Some(place) = self.place.clone() else { return };
        if message.is_empty() || self.committing || !self.has_changes() {
            return;
        }
        self.committing = true;
        self.said = None;
        let (client, push) = (self.client.clone(), self.push);
        run(self, self.live, cx, move || client.commit(&place.directory, &message, push), move |this, done, cx| {
            this.committing = false;
            match done {
                Ok(Committed::Done) => {
                    this.message.update(cx, |field, cx| field.set_text("", cx));
                    this.said = None;
                }
                Ok(Committed::NotPushed(why)) => {
                    this.message.update(cx, |field, cx| field.set_text("", cx));
                    this.said = Some(CommitSaid::NotPushed(why));
                }
                Err(error) => this.said = Some(CommitSaid::Failed(error.to_string())),
            }
            this.read_again(cx);
            cx.notify();
        });
        cx.notify();
    }

    /// Pushes what's committed, after a commit whose push didn't go through.
    pub fn push(&mut self, cx: &mut Context<Self>) {
        let Some(place) = self.place.clone() else { return };
        self.committing = true;
        let client = self.client.clone();
        run(self, self.live, cx, move || client.push(&place.directory), move |this, done, cx| {
            this.committing = false;
            this.said = Some(match done {
                Ok(()) => CommitSaid::Pushed,
                Err(error) => CommitSaid::NotPushed(error.to_string()),
            });
            cx.notify();
        });
        cx.notify();
    }

    fn has_changes(&self) -> bool {
        self.files.ready().is_some_and(|files| !files.files.is_empty())
    }

    // MARK: Drawing

    /// A group's label, over its files, where the list has more than one group.
    fn group_label(group: Group, count: usize, cx: &App) -> AnyElement {
        let c = &cx.theme().colors;
        let title = match group {
            Group::Conflicted => "Conflicts",
            Group::Staged => "Staged",
            Group::Unstaged => "Unstaged",
            Group::Untracked => "New Files",
        };
        div()
            .flex()
            .items_end()
            .gap(space::S1)
            .h(px(28.))
            .pb(space::S1)
            .px(space::S3)
            .text_ui(TypeSize::Caption)
            .text_color(if group == Group::Conflicted { c.warning } else { c.text_faint })
            .child(title)
            .child(div().text_color(c.text_faint).child(count.to_string()))
            .into_any_element()
    }

    /// A file's header: folded or open, its path, and what changed (its actions on hover).
    fn header(&self, file: &ChangedFile, open: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let key = file.key();
        let group: SharedString = format!("change-{key}").into();
        let busy = self.busy.contains(&key);
        let mark = mark(file, &c);
        let lines = match (file.additions, file.deletions) {
            (Some(added), Some(removed)) if added + removed > 0 => Some(
                div()
                    .flex()
                    .flex_none()
                    .gap(space::S1)
                    .text_mono(TypeSize::Caption)
                    .when(added > 0, |d| d.child(div().text_color(c.diff_added).child(format!("+{added}"))))
                    .when(removed > 0, |d| d.child(div().text_color(c.diff_removed).child(format!("−{removed}")))),
            ),
            _ => None,
        };
        let actions = self.actions(file, cx);
        let toggled = file.clone();
        let selector = format!("change-{key}");
        div()
            .id(SharedString::from(format!("change-{key}")))
            .debug_selector(move || selector)
            .group(group.clone())
            .flex()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .items_center()
            .gap(space::S1_5)
            .h(px(34.))
            .pl(space::S2)
            .pr(space::S3)
            .border_b_1()
            .border_color(c.border)
            .cursor_pointer()
            .hover(move |s| s.bg(c.hover))
            .when(busy, |d| d.opacity(0.5))
            .on_click(cx.listener(move |this, _, window, cx| {
                // The keyboard comes along, so Escape puts a confirm away.
                window.focus(&this.focus, cx);
                this.toggle(toggled.clone(), cx);
            }))
            .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::XSmall).color(c.text_faint))
            .children(mark.map(|(letter, color)| div().w(px(12.)).flex_none().text_center().text_mono(TypeSize::Caption).text_color(color).child(letter)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_mono(TypeSize::Small)
                    .text_color(c.text)
                    .when(file.status == "deleted", |d| d.line_through())
                    .child(match &file.old_path {
                        Some(old) => format!("{old} → {}", file.path),
                        None => file.path.clone(),
                    }),
            )
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_end()
                    .min_w(px(52.))
                    .h_full()
                    .children(lines.map(|lines| div().flex().items_center().group_hover(group.clone(), |s| s.opacity(0.)).child(lines)))
                    .child(div().absolute().top_0().bottom_0().right_0().flex().items_center().gap(space::S0_5).opacity(0.).group_hover(group, |s| s.opacity(1.)).children(actions)),
            )
            .into_any_element()
    }

    /// A file's actions, as the box says them: what its stage button does (`stage`), and a discard
    /// where it has one.
    fn actions(&self, file: &ChangedFile, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let key = file.key();
        let mut actions = Vec::new();
        if file.discard.is_some() {
            let discard = file.clone();
            actions.push(
                Button::icon_only(SharedString::from(format!("discard-{key}")), IconName::Undo, "Discard Changes")
                    .ghost()
                    .size(ButtonSize::Small)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.confirm_discard(discard.clone(), cx);
                    }))
                    .into_any_element(),
            );
        }
        // (A word a newer box says that this app doesn't know: no button.)
        let (action, icon, label) = match file.stage {
            Stage::Unstage => (Action::Unstage, IconName::Minus, "Unstage"),
            Stage::Resolve => (Action::Stage, IconName::Check, "Mark as Resolved"),
            Stage::Stage => (Action::Stage, IconName::Plus, "Stage"),
            Stage::Unknown => return actions,
        };
        let acted = file.clone();
        actions.push(
            Button::icon_only(SharedString::from(format!("{}-{key}", action.word())), icon, label)
                .ghost()
                .size(ButtonSize::Small)
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.act(action, acted.clone(), cx);
                }))
                .into_any_element(),
        );
        actions
    }

    /// An open file's diff, under its header.
    fn body(&self, file: &ChangedFile, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let key = file.key();
        let Some(opened) = self.opened.get(&key) else {
            return kuvo_ui::small::LoadingLabel::new(SharedString::from(format!("diff-loading-{key}")), "Reading its changes…").compact().into_any_element();
        };
        let can_view_image = file.status != "deleted" && crate::image_viewer::is_image_path(&file.path);
        let body = match &opened.diff {
            Load::Loading => div().px(space::S4).py(space::S2).child(kuvo_ui::small::LoadingLabel::new(SharedString::from(format!("diff-loading-{key}")), "Reading its changes…").compact()).into_any_element(),
            Load::Failed(why) => {
                let again = file.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .px(space::S4)
                    .py(space::S2)
                    .text_ui(TypeSize::Caption)
                    .child(div().flex_1().min_w_0().text_color(c.text_muted).child(format!("Its changes couldn’t be read: {why}")))
                    .child(Button::new(SharedString::from(format!("diff-again-{key}")), "Try Again").ghost().size(ButtonSize::Small).on_click(cx.listener(move |this, _, _, cx| this.load(again.clone(), cx))))
                    .into_any_element()
            }
            Load::Ready(_) if opened.unshown.is_some() && can_view_image => {
                let clicked = file.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .px(space::S4)
                    .py(space::S2)
                    .child(Icon::new(IconName::Image).size(IconSize::Nav).color(c.text_muted))
                    .child(div().flex_1().text_ui(TypeSize::Caption).text_color(c.text_faint).child("A binary image"))
                    .child(
                        div().debug_selector(|| "changes-open-image".into()).child(
                            Button::new(SharedString::from(format!("changes-open-image-{key}")), "View Image")
                                .icon(IconName::ExpandEditor)
                                .ghost()
                                .size(ButtonSize::Small)
                                .on_click(cx.listener(move |this, _, window, cx| this.open_image_in_viewer(&clicked, window, cx))),
                        ),
                    )
                    .into_any_element()
            }
            Load::Ready(_) if opened.unshown.is_some() => note(opened.unshown.unwrap_or_default(), &c).into_any_element(),
            Load::Ready(lines) if lines.is_empty() => note("Nothing to show.", &c).into_any_element(),
            Load::Ready(lines) => {
                let this = cx.entity().downgrade();
                let at = key.clone();
                let language = file.path.rsplit_once('.').map(|(_, extension)| extension.to_ascii_lowercase()).unwrap_or_default();
                DiffView::new(SharedString::from(format!("diff-{key}")), lines.clone())
                    .flush()
                    .language(language)
                    .folding(opened.expanded.clone(), move |start, _, cx| {
                        this.update(cx, |this, cx| this.expand(&at, start, cx)).ok();
                    })
                    .into_any_element()
            }
        };
        div().w_full().border_b_1().border_color(c.border).child(body).into_any_element()
    }

    /// Every file, each under its header, the open ones with their diffs.
    fn list(&self, files: &GitFiles, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let groups = files.files.windows(2).filter(|pair| pair[0].group != pair[1].group).count() + 1;
        let mut rows: Vec<AnyElement> = Vec::new();
        for (at, file) in files.files.iter().enumerate() {
            if groups > 1 && (at == 0 || files.files[at - 1].group != file.group) {
                let count = files.files[at..].iter().take_while(|next| next.group == file.group).count();
                rows.push(Self::group_label(file.group, count, cx));
            }
            let open = self.is_open(at, file);
            rows.push(self.header(file, open, cx));
            if open {
                rows.push(self.body(file, cx));
            }
        }
        if files.more > 0 {
            let said = if files.more == 1 { "and 1 more new file".to_owned() } else { format!("and {} more new files", files.more) };
            rows.push(div().flex().items_center().h(px(30.)).px(space::S4).text_ui(TypeSize::Caption).text_color(c.text_faint).child(said).into_any_element());
        }
        div()
            .id("changes-list")
            .debug_selector(|| "changes-list".into())
            .track_scroll(&self.scroll)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .children(rows)
            .into_any_element()
    }

    /// The branch it's on, opening Branches.
    fn branch(&self, files: &GitFiles, cx: &mut Context<Self>) -> AnyElement {
        let label = files.branch.clone().unwrap_or_else(|| "No branch".into());
        Button::new("changes-branch", label)
            .icon(IconName::Branch)
            .ghost()
            .size(ButtonSize::Small)
            .truncate_at(px(180.))
            .tooltip("Branches")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(ChangesEvent::ShowBranches)))
            .into_any_element()
    }

    /// The commit's place at the foot: the message, what it takes, the branch, Push, and Commit.
    fn foot(&self, files: &GitFiles, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        if let Some(file) = &self.confirming {
            return self.confirm(file, cx);
        }
        let takes = commit_takes(files);
        let written = !self.message.read(cx).text().trim().is_empty();
        let said = self.said.clone().map(|said| {
            let (text, color, push) = match said {
                CommitSaid::Failed(why) => (why, c.danger, false),
                CommitSaid::NotPushed(why) => (why, c.warning, true),
                CommitSaid::Pushed => ("Pushed.".to_owned(), c.text_muted, false),
            };
            div()
                .id("commit-said")
                .debug_selector(|| "commit-said".into())
                .flex()
                .flex_col()
                .gap(space::S1)
                .max_h(px(120.))
                .overflow_y_scroll()
                .text_ui(TypeSize::Caption)
                .text_color(color)
                .child(text)
                .when(push, |d| {
                    d.child(div().child(Button::new("push-again", "Push").size(ButtonSize::Small).loading(self.committing).on_click(cx.listener(|this, _, _, cx| this.push(cx)))))
                })
        });
        let push = self.push;
        let has_files = !files.files.is_empty();
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(space::S2)
            .p(space::S3)
            .border_t_1()
            .border_color(c.border)
            .when(has_files, |d| d.child(self.message.clone()))
            .children(said)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .child(div().flex_none().ml(-space::S2).child(self.branch(files, cx)))
                    .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Caption).text_color(c.text_faint).when(has_files, |d| d.child(takes)))
                    .when(has_files, |d| {
                        d.child({
                            let this = cx.entity().downgrade();
                            Checkbox::new("commit-push", push).label("Push").on_change(move |on, _, cx| {
                                this.update(cx, |this, cx| this.set_push(on, cx)).ok();
                            })
                        })
                        .child(
                            Button::new("commit", if push { "Commit and Push" } else { "Commit" })
                                .primary()
                                .size(ButtonSize::Small)
                                .disabled(!written || self.committing)
                                .loading(self.committing)
                                .tooltip("⌘↩")
                                .on_click(cx.listener(|this, _, _, cx| this.commit(cx))),
                        )
                    }),
            )
            .into_any_element()
    }

    /// The discard's confirm, in the foot's place.
    fn confirm(&self, file: &ChangedFile, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let DiscardWords { title, detail, button, undoable } = discard_words(file);
        let file = file.clone();
        div()
            .id("discard-confirm")
            .debug_selector(|| "discard-confirm".into())
            .flex()
            .flex_col()
            .flex_none()
            .gap(space::S2)
            .p(space::S4)
            .border_t_1()
            .border_color(c.border)
            .bg(c.elevated)
            .child(div().flex().items_center().gap(space::S2).child(Icon::new(IconName::Warning).size(IconSize::Nav).color(c.warning)).child(div().medium().child(title)))
            .when(!detail.is_empty(), |d| d.child(div().text_ui(TypeSize::Caption).text_color(c.text_muted).child(detail)))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(space::S2)
                    .child(Button::new("discard-cancel", "Cancel").size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.cancel_discard(cx))))
                    .child(Button::new("discard-confirmed", button).map(|button| if undoable { button.primary() } else { button.destructive() }).size(ButtonSize::Small).on_click(cx.listener(move |this, _, _, cx| this.act(Action::Discard, file.clone(), cx)))),
            )
            .into_any_element()
    }
}

/// A discard's confirm, in words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscardWords {
    pub title: String,
    pub detail: String,
    pub button: &'static str,
    /// Drawn as an ordinary button, not a destructive one.
    pub undoable: bool,
}

/// The words of `file`'s discard confirm, by what the box says the discard comes to (`discards`) and
/// where it goes back to (`discard`); what it says can be undone isn't drawn as destructive.
pub fn discard_words(file: &ChangedFile) -> DiscardWords {
    let name = file.name().to_owned();
    let to_commit = file.discard.as_deref() == Some("head");
    let kind = file.discards.as_ref().map_or(DiscardKind::Unknown, |discards| discards.kind);
    let undoable = file.discards.as_ref().is_some_and(|discards| discards.undoable);
    let (title, detail, button) = match kind {
        DiscardKind::Trash if file.repository => (format!("Move the repository {name} to the Trash?"), "It’s a repository of its own: its commits go with it, pushed or not.".to_owned(), "Move to Trash"),
        DiscardKind::Trash => (format!("Move {name} to the Trash?"), "It’s a new file, so git keeps nothing of it.".to_owned(), "Move to Trash"),
        DiscardKind::Restore if to_commit => (format!("Restore {name}?"), "The last commit’s copy comes back; a copy on disk that differs goes to the Trash first.".to_owned(), "Restore"),
        DiscardKind::Restore => (format!("Restore {name}?"), "It comes back as it was last staged.".to_owned(), "Restore"),
        DiscardKind::UndoMove => {
            let old = file.old_path.clone().unwrap_or_default();
            (format!("Undo the move to {name}?"), format!("{old} comes back as last committed, and {name} goes to the Trash. A file written at {old} since goes to the Trash first."), "Undo Move")
        }
        DiscardKind::Changes if to_commit => (format!("Discard every change to {name}?"), "It goes back to the last commit, what’s staged too.".to_owned(), "Discard"),
        DiscardKind::Changes => (format!("Discard the changes to {name}?"), "What isn’t staged is thrown away.".to_owned(), "Discard"),
        // (A kind a newer box says that this app has no words for: asked plainly.)
        DiscardKind::Unknown => (format!("Discard the changes to {name}?"), String::new(), "Discard"),
    };
    // What the box says is lost for good, in its own sentence, after what the discard does.
    let detail = match file.discards.as_ref().and_then(|discards| discards.loses.as_deref()) {
        Some(loses) if detail.is_empty() => loses.to_owned(),
        Some(loses) => format!("{detail} {loses}"),
        None if kind == DiscardKind::Changes && !undoable => format!("{detail} This can’t be undone."),
        None => detail,
    };
    DiscardWords { title, detail, button, undoable }
}

impl ChangesView {
    fn open_image_in_viewer(&self, file: &ChangedFile, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root() else { return };
        let full_path = |rel: &str| {
            if rel.starts_with('/') {
                rel.to_owned()
            } else {
                format!("{}/{}", root.trim_end_matches('/'), rel)
            }
        };
        let target = full_path(&file.path);
        let mut paths: Vec<String> = self
            .files
            .ready()
            .map(|files| files.files.iter().filter(|f| f.status != "deleted" && crate::image_viewer::is_image_path(&f.path)).map(|f| full_path(&f.path)).collect())
            .unwrap_or_default();
        paths.dedup();
        if !paths.iter().any(|p| p == &target) {
            paths.push(target.clone());
        }
        let active = paths.iter().position(|p| p == &target).unwrap_or(0);
        let here = self.client.here();
        let items: Vec<crate::image_viewer::ViewerItem> = paths
            .into_iter()
            .map(|file_path| {
                let name = file_path.rsplit('/').next().unwrap_or(&file_path).to_owned();
                let client = self.client.clone();
                let uri = crate::image_viewer::file_uri(&file_path);
                crate::image_viewer::ViewerItem {
                    id: format!("change:{file_path}"),
                    name,
                    caption: None,
                    source_tag: "Changes".into(),
                    local_path: here.then(|| std::path::PathBuf::from(&file_path)),
                    read: crate::pictures::Read::Reading,
                    loader: Some(std::sync::Arc::new(move || crate::pictures::read(&client, &kuvo_box::pictures::Source::File(uri.clone())))),
                }
            })
            .collect();
        crate::image_viewer::open(crate::image_viewer::OpenRequest { items, active, return_focus: Some(self.focus.clone()), on_change: None }, window, cx);
    }
}

/// What a commit takes, in words, from what the box says: what it takes (`commits`), how many files
/// that is (`takes`) and how many of them are staged outside this folder (`stagedElsewhere`). Nothing
/// is counted here.
pub fn commit_takes(files: &GitFiles) -> String {
    let (takes, elsewhere) = (files.takes, files.staged_elsewhere);
    if !files.commits_staged() {
        return if takes == 1 { "Commits the one changed file.".to_owned() } else { format!("Commits all {takes} changed files.") };
    }
    match (takes, elsewhere) {
        // (`stagedElsewhere` counts a move's two sides, so it can pass `takes`.)
        (1, 0) => "Commits the staged file.".to_owned(),
        (takes, 0) => format!("Commits the {takes} staged files."),
        (1, _) => "Commits the file staged outside this folder.".to_owned(),
        (takes, elsewhere) if elsewhere >= takes => format!("Commits the {takes} files staged outside this folder."),
        (takes, elsewhere) => format!("Commits the {takes} staged files, {elsewhere} of them outside this folder."),
    }
}

/// What can be done to a file from its header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Stage,
    Unstage,
    Discard,
}

impl Action {
    fn word(self) -> &'static str {
        match self {
            Action::Stage => "stage",
            Action::Unstage => "unstage",
            Action::Discard => "discard",
        }
    }
}

/// A file's mark beside its path, where it's more than changed: a conflict, or added, deleted or
/// moved, as a letter in its colour.
fn mark(file: &ChangedFile, c: &kuvo_ui::theme::Colors) -> Option<(&'static str, gpui::Hsla)> {
    if file.group == Group::Conflicted {
        return Some(("!", c.warning));
    }
    match file.status.as_str() {
        "added" | "untracked" => Some(("A", c.diff_added)),
        "deleted" => Some(("D", c.diff_removed)),
        "renamed" => Some(("R", c.accent)),
        _ => None,
    }
}

/// A quiet line in a view's place.
fn note(text: impl Into<SharedString>, c: &kuvo_ui::theme::Colors) -> impl IntoElement {
    div().px(space::S4).py(space::S2).text_ui(TypeSize::Caption).text_color(c.text_faint).child(text.into())
}

/// A view's whole room saying something: a title and a line under it, and an action.
pub(crate) fn empty(title: impl Into<SharedString>, detail: impl Into<SharedString>, action: Option<AnyElement>, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .flex()
        .flex_col()
        .flex_1()
        .items_center()
        .justify_center()
        .gap(space::S1)
        .px(space::S5)
        .text_center()
        .child(div().text_ui(TypeSize::Small).medium().text_color(c.text_muted).child(title.into()))
        .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(detail.into()))
        .children(action.map(|action| div().pt(space::S2).child(action)))
        .into_any_element()
}

impl Focusable for ChangesView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ChangesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        let content: AnyElement = match &self.files {
            Load::Loading => kuvo_ui::small::LoadingLabel::new("changes-loading", "Reading changes…").fill().into_any_element(),
            Load::Failed(why) => empty(
                "Couldn’t Read the Changes",
                why.clone(),
                Some(Button::new("changes-again", "Try Again").size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.read_again(cx))).into_any_element()),
                cx,
            ),
            Load::Ready(files) if !files.repository => empty("Not a Git Repository", "This folder isn’t in one, so there are no changes to show.", None, cx),
            Load::Ready(files) => {
                let files = files.clone();
                let failed = self.failed.clone().or_else(|| self.stale.clone().map(|why| format!("The list couldn’t be read again: {why}")));
                let body = if files.files.is_empty() {
                    empty("No Changes", "Everything in this folder is committed.", None, cx)
                } else {
                    self.list(&files, cx)
                };
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .children(files.underway.as_deref().map(|underway| {
                        let said = match underway {
                            "rebasing" => "A rebase is under way here: finish it, or stop it, in a terminal.",
                            "cherry-picking" => "A cherry-pick is under way here: finish it, or stop it, in a terminal.",
                            "reverting" => "A revert is under way here: finish it, or stop it, in a terminal.",
                            _ => "A merge is under way here: resolve its conflicts, then commit.",
                        };
                        div()
                            .id("changes-underway")
                            .debug_selector(|| "changes-underway".into())
                            .flex()
                            .items_center()
                            .gap(space::S2)
                            .mx(space::S3)
                            .my(space::S2)
                            .px(space::S3)
                            .py(space::S2)
                            .rounded(radius::NAV)
                            .bg(c.sunken)
                            .text_ui(TypeSize::Caption)
                            .text_color(c.text_muted)
                            .child(Icon::new(IconName::Warning).size(IconSize::Nav).color(c.warning))
                            .child(said)
                    }))
                    .children(failed.map(|why| {
                        div().id("changes-failed").debug_selector(|| "changes-failed".into()).mx(space::S3).my(space::S2).px(space::S3).py(space::S2).rounded(radius::NAV).bg(c.sunken).text_ui(TypeSize::Caption).text_color(c.danger).child(why)
                    }))
                    .child(body)
                    .child(self.foot(&files, cx))
                    .into_any_element()
            }
        };
        div()
            .key_context("Changes")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Commit, _, cx| this.commit(cx)))
            .on_action(cx.listener(|this, _: &Back, _, cx| {
                if this.confirming.take().is_some() {
                    cx.notify();
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

    fn listed(json: &str) -> GitFiles {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_commit_is_said_to_take_as_many_files_as_the_box_counts() {
        let file = |group: &str| format!(r#"{{"path":"app/{group}.ts","group":"{group}","status":"modified","scope":"unstaged"}}"#);
        // Only the folder's own file changed, another folder's staged: it takes that one.
        let elsewhere = listed(&format!(r#"{{"repository":true,"commits":"staged","stagedElsewhere":1,"takes":1,"files":[{}]}}"#, file("unstaged")));
        assert_eq!(commit_takes(&elsewhere), "Commits the file staged outside this folder.");
        let both = listed(&format!(r#"{{"repository":true,"commits":"staged","stagedElsewhere":2,"takes":3,"files":[{}]}}"#, file("staged")));
        assert_eq!(commit_takes(&both), "Commits the 3 staged files, 2 of them outside this folder.");
        let here = listed(&format!(r#"{{"repository":true,"commits":"staged","stagedElsewhere":0,"takes":1,"files":[{}]}}"#, file("staged")));
        assert_eq!(commit_takes(&here), "Commits the staged file.");
        let all = listed(&format!(r#"{{"repository":true,"commits":"all","stagedElsewhere":0,"more":4,"takes":5,"files":[{}]}}"#, file("unstaged")));
        assert_eq!(commit_takes(&all), "Commits all 5 changed files.");
        // The count is the box's: the rows here are never counted in its place.
        let said = listed(&format!(r#"{{"repository":true,"commits":"staged","stagedElsewhere":0,"takes":7,"files":[{}]}}"#, file("staged")));
        assert_eq!(commit_takes(&said), "Commits the 7 staged files.");
        let outside = listed(&format!(r#"{{"repository":true,"commits":"staged","stagedElsewhere":4,"takes":4,"files":[{}]}}"#, file("unstaged")));
        assert_eq!(commit_takes(&outside), "Commits the 4 files staged outside this folder.");
        // A move staged outside is one file to commit, though both its sides are outside.
        let moved = listed(&format!(r#"{{"repository":true,"commits":"staged","stagedElsewhere":4,"takes":2,"files":[{}]}}"#, file("unstaged")));
        assert_eq!(commit_takes(&moved), "Commits the 2 files staged outside this folder.");
    }

    #[test]
    fn a_discards_confirm_is_worded_by_what_the_box_says_it_comes_to() {
        let words = |more: &str| discard_words(&serde_json::from_str(&format!(r#"{{"path":"app/Form.tsx","group":"unstaged","scope":"unstaged",{more}}}"#)).unwrap());
        // The kind is the box's, whatever the file's status would have suggested.
        let restored = words(r#""status":"modified","discard":"head","discards":{"kind":"restore","undoable":true}"#);
        assert_eq!((restored.title.as_str(), restored.button, restored.undoable), ("Restore Form.tsx?", "Restore", true));
        assert!(restored.detail.starts_with("The last commit’s copy comes back"));
        let staged = words(r#""status":"deleted","discard":"unstaged","discards":{"kind":"restore","undoable":true}"#);
        assert_eq!(staged.detail, "It comes back as it was last staged.");
        let moved = words(r#""status":"renamed","oldPath":"app/Old.tsx","discard":"head","discards":{"kind":"undoMove","undoable":true}"#);
        assert_eq!((moved.title.as_str(), moved.button, moved.undoable), ("Undo the move to Form.tsx?", "Undo Move", true));
        assert!(moved.detail.starts_with("app/Old.tsx comes back as last committed"));
        let trashed = words(r#""status":"added","discard":"trash","repository":true,"discards":{"kind":"trash","undoable":true}"#);
        assert_eq!((trashed.title.as_str(), trashed.button), ("Move the repository Form.tsx to the Trash?", "Move to Trash"));
        // Whether it's drawn as destructive is the box's word too: a deletion it doesn't call a
        // restore is a plain, destructive discard.
        let thrown = words(r#""status":"deleted","discard":"head","discards":{"kind":"changes","undoable":false}"#);
        assert_eq!((thrown.title.as_str(), thrown.button, thrown.undoable), ("Discard every change to Form.tsx?", "Discard", false));
        // What a discard loses for good is the box's sentence, said after what it does.
        let lost = words(r#""status":"added","discard":"trash","discards":{"kind":"trash","undoable":false,"loses":"What you staged of it isn’t kept anywhere, so this can’t be undone."}"#);
        assert_eq!(lost.detail, "It’s a new file, so git keeps nothing of it. What you staged of it isn’t kept anywhere, so this can’t be undone.");
        assert!(!lost.undoable);
        let unstaged = words(r#""status":"modified","discard":"unstaged","discards":{"kind":"changes","undoable":false}"#);
        assert_eq!((unstaged.title.as_str(), unstaged.undoable), ("Discard the changes to Form.tsx?", false));
        let reversible = words(r#""status":"modified","discard":"unstaged","discards":{"kind":"changes","undoable":true}"#);
        assert_eq!(reversible.detail, "What isn’t staged is thrown away.");
        // A kind a newer box says: asked plainly, drawn as the box says.
        let newer = words(r#""status":"modified","discard":"head","discards":{"kind":"shelve","undoable":true}"#);
        assert_eq!((newer.title.as_str(), newer.detail.as_str(), newer.button, newer.undoable), ("Discard the changes to Form.tsx?", "", "Discard", true));
    }
}
