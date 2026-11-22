//! A session's actions menu (Draft B), inline rename popover, and delete confirmation popover.
//!
//! What's offered and every word come from the box (`GET kuvo/sessions/<id>/actions`,
//! `kuvo_box::session_actions`): this module only adds platform conventions ("…" when asking,
//! a separator before destructive actions, keyboard shortcut hints `⌘R` / `⌘⌫` / `⇧⌘⌫`) and
//! skips action IDs this client doesn't have a control for.

use std::{cell::Cell, collections::HashMap, rc::Rc};

use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Context, Entity, FocusHandle,
    Focusable, InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, MouseButton,
    ParentElement, Render, Styled, Subscription, Task, WeakEntity, Window, actions, div, px,
};
use kuvo_box::accounts::Confirm;
use kuvo_box::session_actions::Actions;
use kuvo_box::{BoxClient, Listing};
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, StyledType, TextField, TextFieldEvent, TypeSize,
    anchor::{Placement, Popover},
    menu::{Menu, MenuEntry, MenuItem, PopoverMenuHandle},
    theme::{radius, space},
};

use crate::workspace::Workspace;

actions!(session_menu, [RenameSession, ArchiveSession, DeleteSession]);

/// Keyboard shortcuts for the session open or walked in Sessions.
/// Ignored while a text field has focus so typing in the composer or search never triggers them.
pub fn bind_keys(cx: &mut App) {
    let context = format!("Workspace && !{}", kuvo_ui::text_field::CONTEXT);
    let context = Some(context.as_str());
    cx.bind_keys([
        KeyBinding::new("cmd-r", RenameSession, context),
        KeyBinding::new("cmd-backspace", ArchiveSession, context),
        KeyBinding::new("cmd-shift-backspace", DeleteSession, context),
    ]);
}

/// Where a session's popover (Rename or Delete confirm) anchors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PopoverAnchor {
    Row(String),
    TitleBar,
}

/// Extracts the session ID from a sidebar row key (`session-*:<id>`, `found:<id>`, `needs:<id>`).
pub fn session_from_row_key(key: &str) -> Option<&str> {
    let (prefix, id) = key.split_once(':')?;
    (prefix.starts_with("session-") || prefix == "found" || prefix == "needs").then_some(id)
}

/// Cached `GET kuvo/sessions/<id>/actions` state for a sidebar row.
/// Keeps previously fetched actions visible while stale or refreshing so hover buttons never flicker.
#[derive(Clone, Debug, Default)]
pub struct RowActions {
    pub actions: Option<Actions>,
    pub fetching: Option<u64>,
    pub stale: bool,
    pub failed: bool,
}

/// An active Rename or Delete confirm popover.
pub enum ActivePopover {
    Rename {
        session: String,
        anchor: PopoverAnchor,
        view: Entity<RenamePopover>,
    },
    Delete {
        session: String,
        anchor: PopoverAnchor,
        view: Entity<ConfirmDeletePopover>,
    },
}

impl ActivePopover {
    pub fn session(&self) -> &str {
        match self {
            Self::Rename { session, .. } | Self::Delete { session, .. } => session,
        }
    }

    pub fn anchor(&self) -> &PopoverAnchor {
        match self {
            Self::Rename { anchor, .. } | Self::Delete { anchor, .. } => anchor,
        }
    }

    pub fn is_at_row(&self, row_key: &str) -> bool {
        matches!(self.anchor(), PopoverAnchor::Row(key) if key == row_key)
    }

    pub fn is_at_title_bar(&self) -> bool {
        matches!(self.anchor(), PopoverAnchor::TitleBar)
    }
}

/// State owned by [`Workspace`] for session menus, hover buttons, and popovers.
#[derive(Default)]
pub struct SessionMenu {
    pub hovered_row: Option<String>,
    pub row_menus: HashMap<String, PopoverMenuHandle>,
    pub row_actions: HashMap<String, RowActions>,
    pub actions_epoch: u64,
    pub title_menu: PopoverMenuHandle,
    pub active_popover: Option<ActivePopover>,
    pub action_error: Option<String>,
    pub action_tasks: Vec<Task<()>>,
}

impl SessionMenu {
    pub fn row_menu_handle(&mut self, key: &str) -> PopoverMenuHandle {
        self.row_menus.entry(key.to_owned()).or_default().clone()
    }

    pub fn cached_actions(&self, session: &str) -> Option<&Actions> {
        self.row_actions.get(session).and_then(|entry| entry.actions.as_ref())
    }

    pub fn cache_actions(&mut self, session: &str, actions: Actions) {
        self.row_actions.insert(
            session.to_owned(),
            RowActions {
                actions: Some(actions),
                fetching: None,
                stale: false,
                failed: false,
            },
        );
    }

    pub fn begin_fetch(&mut self, session: &str) -> u64 {
        self.actions_epoch = self.actions_epoch.wrapping_add(1);
        let seq = self.actions_epoch;
        let entry = self.row_actions.entry(session.to_owned()).or_default();
        entry.fetching = Some(seq);
        entry.stale = false;
        entry.failed = false;
        seq
    }

    pub fn finish_fetch(&mut self, session: &str, seq: u64, result: kuvo_box::Result<Actions>) -> bool {
        let Some(entry) = self.row_actions.get_mut(session) else {
            return false;
        };
        if entry.fetching != Some(seq) {
            return false;
        }
        entry.fetching = None;
        match result {
            Ok(actions) => {
                entry.actions = Some(actions);
                entry.stale = false;
                entry.failed = false;
            }
            Err(_) => {
                entry.actions = None;
                entry.stale = false;
                entry.failed = true;
            }
        }
        true
    }

    pub fn invalidate_session(&mut self, session: &str) {
        if let Some(entry) = self.row_actions.get_mut(session) {
            entry.stale = true;
            entry.failed = false;
            entry.fetching = None;
        }
    }

    pub fn remove_actions(&mut self, session: &str) {
        self.row_actions.remove(session);
    }

    pub fn clear_actions(&mut self) {
        self.actions_epoch = self.actions_epoch.wrapping_add(1);
        self.row_actions.clear();
    }

    pub fn mark_changed_actions(&mut self, previous: &Listing, current: &Listing) {
        self.row_actions.retain(|id, _| current.facts(id).is_some());
        for (id, entry) in &mut self.row_actions {
            if previous.facts(id) != current.facts(id) {
                entry.stale = true;
                entry.failed = false;
                entry.fetching = None;
            }
        }
    }
}

/// Builds text-only [`MenuItem`]s from the box's [`Actions`] in the box's order.
pub fn build_menu_items(
    actions: &Actions,
    session_id: String,
    anchor: PopoverAnchor,
    workspace: WeakEntity<Workspace>,
    move_chat: bool,
) -> Vec<MenuItem> {
    let mut items = Vec::new();
    if let Some(held) = &actions.held {
        items.push(MenuItem::Header(held.clone().into()));
    }
    let held = actions.held.is_some();
    let mut lined = false;
    let mut any_above = move_chat;
    if move_chat {
        let (workspace, session, anchor) = (workspace.clone(), session_id.clone(), anchor.clone());
        items.push(MenuItem::Entry(MenuEntry::new("Move to…").disabled(held).on_select(move |window, cx| {
            workspace.update(cx, |workspace, cx| workspace.move_chat_menu(&session, &anchor, window, cx)).ok();
        })));
    }
    for action in &actions.actions {
        let supported = matches!(
            action.id.as_str(),
            "pin" | "unpin" | "rename" | "archive" | "unarchive" | "delete"
        );
        if !supported {
            continue;
        }
        if action.destructive && any_above && !lined {
            items.push(MenuItem::Separator);
            lined = true;
        }
        let asks = action.confirm.is_some() || action.id == "rename";
        let label = if asks {
            format!("{}…", action.label)
        } else {
            action.label.clone()
        };
        let mut entry = MenuEntry::new(label).disabled(held);
        if action.destructive {
            entry = entry.destructive();
        }
        if let Some(keys) = shortcut_for(&action.id) {
            entry = entry.shortcut(keys);
        }
        if !held {
            let id = action.id.clone();
            let session = session_id.clone();
            let anchor = anchor.clone();
            let confirm = action.confirm.clone();
            let workspace = workspace.clone();
            entry = entry.on_select(move |window, cx| {
                workspace
                    .update(cx, |workspace, cx| match id.as_str() {
                        "pin" => workspace.pin_session(&session, true, cx),
                        "unpin" => workspace.pin_session(&session, false, cx),
                        "archive" => workspace.archive_session(&session, true, window, cx),
                        "unarchive" => workspace.archive_session(&session, false, window, cx),
                        "rename" => workspace.start_rename(&session, anchor.clone(), window, cx),
                        "delete" => workspace.run_delete_action(
                            &session,
                            confirm.clone(),
                            anchor.clone(),
                            window,
                            cx,
                        ),
                        _ => {}
                    })
                    .ok();
            });
        }
        items.push(MenuItem::Entry(entry));
        any_above = true;
    }
    if items.is_empty() {
        items.push(MenuItem::Header("No actions available.".into()));
    }
    items
}

fn shortcut_for(id: &str) -> Option<&'static str> {
    match id {
        "rename" => Some("⌘R"),
        "archive" => Some("⌘⌫"),
        "delete" => Some("⇧⌘⌫"),
        _ => None,
    }
}

/// Builds a [`Menu`] immediately from an already-fetched `GET kuvo/sessions/<id>/actions` result.
pub fn menu_from_actions(
    result: kuvo_box::Result<Actions>,
    session_id: String,
    anchor: PopoverAnchor,
    workspace: WeakEntity<Workspace>,
    move_chat: bool,
    cx: &mut App,
) -> Entity<Menu> {
    cx.new(|cx| {
        let mut menu = Menu::new(cx).width(px(208.));
        let items = match result {
            Ok(actions) => build_menu_items(&actions, session_id, anchor, workspace, move_chat),
            Err(error) => vec![MenuItem::Header(error.to_string().into())],
        };
        menu.set_items(items, cx);
        menu
    })
}

/// Opens a box-driven [`Menu`] for `session_id`: immediately in tests/shots (`!live`), or
/// loading and updating in place when `live`.
pub fn open_actions_menu(
    client: BoxClient,
    live: bool,
    session_id: String,
    anchor: PopoverAnchor,
    workspace: WeakEntity<Workspace>,
    move_chat: bool,
    window: &mut Window,
    cx: &mut App,
) -> Entity<Menu> {
    if !live {
        let result = client.session_actions(&session_id);
        return menu_from_actions(result, session_id, anchor, workspace, move_chat, cx);
    }

    let menu = cx.new(|cx| Menu::new(cx).width(px(208.)).header("Loading…"));
    let weak_menu = menu.downgrade();
    window
        .spawn(cx, async move |cx| {
            let session = session_id.clone();
            let result = cx
                .background_executor()
                .spawn(async move { client.session_actions(&session) })
                .await;
            if let Ok(actions) = &result {
                let actions = actions.clone();
                let id = session_id.clone();
                workspace
                    .update(cx, |workspace, cx| {
                        workspace.session_menu_mut().cache_actions(&id, actions);
                        cx.notify();
                    })
                    .ok();
            }
            weak_menu
                .update(cx, |menu, cx| {
                    let items = match result {
                        Ok(actions) => build_menu_items(&actions, session_id, anchor, workspace, move_chat),
                        Err(error) => vec![MenuItem::Header(error.to_string().into())],
                    };
                    menu.set_items(items, cx);
                })
                .ok();
        })
        .detach();
    menu
}

/// A session's popover (rename, delete) under its sidebar row, at its leading edge. Make it a
/// child of the row.
pub fn row_anchored_popover(popover: AnyElement) -> AnyElement {
    Popover::new("session-popover-motion", Placement::below(), popover).into_any_element()
}

/// A session's popover under the title bar's "•••" button (or the update chip's card under the
/// chip), lined up with its trailing edge, as it's at the bar's end. Make it a child of the button.
pub fn title_anchored_popover(popover: AnyElement) -> AnyElement {
    Popover::new("session-popover-motion", Placement::below().align_end(), popover).into_any_element()
}

/// Shared target context passed into session popovers (`RenamePopover` / `ConfirmDeletePopover`).
pub struct PopoverTarget {
    pub session: String,
    pub client: BoxClient,
    pub live: bool,
    pub return_to: Option<FocusHandle>,
    pub workspace: WeakEntity<Workspace>,
}

// BOX-NEEDED: whether an archived session can still be written to should be stated by the box in
// `GET kuvo/sessions/<id>/actions` or readiness (today the composer remains open on archived
// sessions). Also `confirm` currently carries a single `message` string rather than structured
// worktree name / uncommitted-file-count facts if a richer confirm layout is desired later.

/// Small popover under a row (or the title bar's "•••") for renaming a session.
pub struct RenamePopover {
    session: String,
    original: String,
    client: BoxClient,
    live: bool,
    field: Entity<TextField>,
    focus: FocusHandle,
    error: Option<String>,
    submitting: bool,
    restore_focus: Rc<Cell<bool>>,
    return_to: Option<FocusHandle>,
    workspace: WeakEntity<Workspace>,
    _field_sub: Subscription,
    _task: Option<Task<()>>,
}

impl Focusable for RenamePopover {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl RenamePopover {
    pub fn new(
        target: PopoverTarget,
        original: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = cx.new(|cx| {
            let mut field = TextField::new(cx).bare();
            field.set_text(original.clone(), cx);
            field.select_range(0..original.len(), cx);
            field
        });
        let field_sub = cx.subscribe_in(&field, window, |this, _, event: &TextFieldEvent, window, cx| {
            match event {
                TextFieldEvent::Changed => {
                    if this.error.take().is_some() {
                        cx.notify();
                    }
                }
                TextFieldEvent::Submit(_) => this.submit(true, window, cx),
                TextFieldEvent::Cancel => this.cancel(window, cx),
            }
        });
        let field_focus = field.focus_handle(cx);
        // The popover paints inside `deferred`, so its focus handle joins the dispatch tree after
        // the next frame; deferring focus ensures it lands cleanly on the text field.
        window.defer(cx, move |window, cx| window.focus(&field_focus, cx));
        Self {
            session: target.session,
            original,
            client: target.client,
            live: target.live,
            field,
            focus: cx.focus_handle(),
            error: None,
            submitting: false,
            restore_focus: Rc::new(Cell::new(true)),
            return_to: target.return_to,
            workspace: target.workspace,
            _field_sub: field_sub,
            _task: None,
        }
    }

    pub fn field(&self) -> &Entity<TextField> {
        &self.field
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.submitting {
            return;
        }
        let return_to = self.return_to.clone();
        self.workspace
            .update(cx, |workspace, cx| workspace.close_session_popover(return_to, window, cx))
            .ok();
    }

    fn submit(&mut self, restore_focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !restore_focus {
            self.restore_focus.set(false);
        }
        if self.submitting {
            return;
        }
        let next = self.field.read(cx).text().trim().to_owned();
        if next.is_empty() || next == self.original.trim() {
            let return_to = restore_focus.then(|| self.return_to.clone()).flatten();
            self.workspace
                .update(cx, |workspace, cx| {
                    if let Some(target) = return_to {
                        workspace.close_session_popover(Some(target), window, cx);
                    } else {
                        workspace.close_session_popover_no_focus(cx);
                    }
                })
                .ok();
            return;
        }

        let session = self.session.clone();
        let client = self.client.clone();
        let return_to = self.return_to.clone();
        if !self.live {
            let target = restore_focus.then_some(return_to).flatten();
            match client.rename_session(&session, &next) {
                Ok(_) => {
                    self.workspace
                        .update(cx, |workspace, cx| workspace.finish_rename(target, window, cx))
                        .ok();
                }
                Err(error) => {
                    self.error = Some(error.to_string());
                    cx.notify();
                }
            }
            return;
        }

        self.submitting = true;
        self.restore_focus.set(restore_focus);
        let restore_focus_flag = self.restore_focus.clone();
        self.error = None;
        cx.notify();
        let window_handle: AnyWindowHandle = window.window_handle();
        let workspace = self.workspace.clone();
        self._task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.rename_session(&session, &next) })
                .await;
            match result {
                Ok(_) => {
                    if let Some(target) = restore_focus_flag.get().then_some(return_to).flatten() {
                        let _ = window_handle.update(cx, |_, window, cx| {
                            workspace
                                .update(cx, |workspace, cx| workspace.finish_rename(Some(target), window, cx))
                                .ok();
                        });
                    } else {
                        workspace
                            .update(cx, |workspace, cx| workspace.finish_rename_no_focus(cx))
                            .ok();
                    }
                }
                Err(error) => {
                    this.update(cx, |this, cx| {
                        this.submitting = false;
                        this.error = Some(error.to_string());
                        cx.notify();
                    })
                    .ok();
                }
            }
        }));
    }
}

impl Render for RenamePopover {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let c = &theme.colors;
        let has_error = self.error.is_some();
        div()
            .id("rename-popover")
            .debug_selector(|| "rename-popover".into())
            .track_focus(&self.focus)
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|this, _, window, cx| this.submit(false, window, cx)))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.cancel(window, cx);
                }
            }))
            .w(px(300.))
            .flex()
            .flex_col()
            .gap(space::S2)
            .p(space::S3)
            .rounded(radius::POPOVER)
            .bg(c.elevated)
            .border_1()
            .border_color(c.border)
            .shadow(theme.elevated_shadow())
            .child(
                div()
                    .debug_selector(|| "rename-field-box".into())
                    .h(px(32.))
                    .flex()
                    .items_center()
                    .px(space::S2)
                    .rounded(radius::NAV)
                    .bg(c.surface)
                    .border_1()
                    .border_color(if has_error { c.danger } else { c.accent })
                    .child(self.field.clone()),
            )
            .children(self.error.as_ref().map(|error| kuvo_ui::Notice::said("rename-error", kuvo_ui::Tone::Error, error.clone())))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .child(
                        div()
                            .flex_1()
                            .text_ui(TypeSize::Caption)
                            .text_color(c.text_faint)
                            .child("Return to rename"),
                    )
                    .child(
                        Button::new("rename-cancel", "Cancel")
                            .size(ButtonSize::Small)
                            .disabled(self.submitting)
                            .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx))),
                    )
                    .child(
                        Button::new("rename-go", "Rename")
                            .primary()
                            .size(ButtonSize::Small)
                            .loading(self.submitting)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(true, window, cx))),
                    ),
            )
    }
}

/// Small popover under a row (or the title bar's "•••") confirming deletion in the box's words.
pub struct ConfirmDeletePopover {
    session: String,
    confirm: Confirm,
    client: BoxClient,
    live: bool,
    focus: FocusHandle,
    error: Option<String>,
    deleting: bool,
    restore_focus: Rc<Cell<bool>>,
    return_to: Option<FocusHandle>,
    workspace: WeakEntity<Workspace>,
    _task: Option<Task<()>>,
}

impl Focusable for ConfirmDeletePopover {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ConfirmDeletePopover {
    pub fn new(
        target: PopoverTarget,
        confirm: Confirm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let popover_focus = focus.clone();
        window.defer(cx, move |window, cx| window.focus(&popover_focus, cx));
        Self {
            session: target.session,
            confirm,
            client: target.client,
            live: target.live,
            focus,
            error: None,
            deleting: false,
            restore_focus: Rc::new(Cell::new(true)),
            return_to: target.return_to,
            workspace: target.workspace,
            _task: None,
        }
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn cancel(&mut self, restore_focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !restore_focus {
            self.restore_focus.set(false);
        }
        if self.deleting {
            return;
        }
        let return_to = restore_focus.then(|| self.return_to.clone()).flatten();
        self.workspace
            .update(cx, |workspace, cx| {
                if let Some(target) = return_to {
                    workspace.close_session_popover(Some(target), window, cx);
                } else {
                    workspace.close_session_popover_no_focus(cx);
                }
            })
            .ok();
    }

    fn confirm_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.deleting {
            return;
        }
        let session = self.session.clone();
        let client = self.client.clone();
        let return_to = self.return_to.clone();
        if !self.live {
            match client.delete_session(&session) {
                Ok(()) => {
                    self.workspace
                        .update(cx, |workspace, cx| workspace.finish_delete(&session, return_to, window, cx))
                        .ok();
                }
                Err(error) => {
                    self.error = Some(error.to_string());
                    cx.notify();
                }
            }
            return;
        }

        self.deleting = true;
        self.restore_focus.set(true);
        let restore_focus_flag = self.restore_focus.clone();
        self.error = None;
        cx.notify();
        let window_handle: AnyWindowHandle = window.window_handle();
        let workspace = self.workspace.clone();
        self._task = Some(cx.spawn(async move |this, cx| {
            let delete_id = session.clone();
            let result = cx
                .background_executor()
                .spawn(async move { client.delete_session(&delete_id) })
                .await;
            match result {
                Ok(()) => {
                    let target = restore_focus_flag.get().then_some(return_to).flatten();
                    let _ = window_handle.update(cx, |_, window, cx| {
                        workspace
                            .update(cx, |workspace, cx| workspace.finish_delete(&session, target, window, cx))
                            .ok();
                    });
                }
                Err(error) => {
                    this.update(cx, |this, cx| {
                        this.deleting = false;
                        this.error = Some(error.to_string());
                        cx.notify();
                    })
                    .ok();
                }
            }
        }));
    }
}

impl Render for ConfirmDeletePopover {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let c = &theme.colors;
        div()
            .id("confirm-delete-popover")
            .debug_selector(|| "confirm-delete-popover".into())
            .track_focus(&self.focus)
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|this, _, window, cx| this.cancel(false, window, cx)))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.cancel(true, window, cx);
                }
            }))
            .w(px(300.))
            .flex()
            .flex_col()
            .gap(space::S2)
            .p(space::S3)
            .rounded(radius::POPOVER)
            .bg(c.elevated)
            .border_1()
            .border_color(c.border)
            .shadow(theme.elevated_shadow())
            .child(
                div()
                    .debug_selector(|| "delete-confirm-title".into())
                    .text_ui(TypeSize::Small)
                    .medium()
                    .text_color(c.text)
                    .child(self.confirm.title.clone()),
            )
            .children((!self.confirm.message.is_empty()).then(|| {
                div()
                    .debug_selector(|| "delete-confirm-message".into())
                    .text_ui(TypeSize::Small)
                    .text_color(c.text_muted)
                    .child(self.confirm.message.clone())
            }))
            .children(self.error.as_ref().map(|error| kuvo_ui::Notice::said("delete-error", kuvo_ui::Tone::Error, error.clone())))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(space::S2)
                    .pt(space::S1)
                    .child(
                        Button::new("delete-cancel", "Cancel")
                            .size(ButtonSize::Small)
                            .disabled(self.deleting)
                            .on_click(cx.listener(|this, _, window, cx| this.cancel(true, window, cx))),
                    )
                    .child(
                        Button::new("delete-go", self.confirm.label.clone())
                            .destructive()
                            .size(ButtonSize::Small)
                            .loading(self.deleting)
                            .on_click(cx.listener(|this, _, window, cx| this.confirm_delete(window, cx))),
                    ),
            )
    }
}
