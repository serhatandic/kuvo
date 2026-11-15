//! Renders Kuvo's screens offscreen against the stand-in box, in light and dark, to
//! `gpui/shots/screens-<screen>-{light,dark}.png`. No window opens, nothing real is reached.
//!
//! `cargo run -p kuvo --features shots --bin shots [-- <screen>...]`

use std::{path::PathBuf, sync::Arc};

use gpui::{AppContext as _, Context, Focusable as _, HeadlessAppContext, Window, px, size};
use kuvo::area::Area;
use kuvo::panel::Tab;
use kuvo_box::git::Group;
use kuvo::first_run::{FirstRun, Step};
use kuvo::stand_in::first_run::Scenario;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::BoxState;
use kuvo::bots::{Act as BotAct, BotsView};
use kuvo::groups::{Act as GroupAct, GroupsView};
use kuvo::queue::{Act as QueueAct, Field, Filter, QueueView, Target};
use kuvo::settings::{SettingsView, sign_in::SignIn};
use kuvo::{Workspace, bind_keys};
use kuvo_box::accounts::Side;
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Assets, Theme};

/// The moment the stand-in's sessions are seen at, so the times read the same in every shot.
const NOW: f64 = 1_794_650_000_000.0;

struct Screen {
    slug: &'static str,
    width: f32,
    height: f32,
    /// The session open (none: the landing view).
    session: Option<&'static str>,
    /// What's typed in the composer.
    typed: Option<&'static str>,
    empty: bool,
    /// Tool steps opened (a step's id, and which of its blocks), and outputs shown whole.
    opens: &'static [(&'static str, usize)],
    /// What's typed in the sidebar's search, and how many rows the keyboard walks down.
    search: Option<&'static str>,
    walks: usize,
    /// Files attached in the composer (from `gpui/`).
    attach: &'static [&'static str],
    /// What the side panel shows (none: it's closed).
    panel: Option<Panel>,
}

type Panel = fn(&mut Workspace, &mut Window, &mut Context<Workspace>);

const SCREENS: &[Screen] = &[
    Screen { slug: "held-box-sessions", width: 1280., height: 820., session: None, typed: None, empty: true, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "held-box", width: 1280., height: 820., session: None, typed: None, empty: true, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_none) },
    Screen { slug: "conversation", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    // Selecting text (`kuvo_ui/src/selection.rs`): across a paragraph, inside a code block, across
    // messages, the right-click menu over a selection, and a drag in three steps.
    Screen { slug: "select-paragraph", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(select_paragraph) },
    Screen { slug: "select-code", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(select_code) },
    Screen { slug: "select-across", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(select_across) },
    Screen { slug: "select-menu", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(select_paragraph) },
    Screen { slug: "select-drag-1", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "select-drag-2", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "select-drag-3", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "landing", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "asking", width: 1280., height: 820., session: Some("ses_limits"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    // What the box asks itself: a command outside the sandbox, an item from the vault.
    Screen { slug: "asking-sandbox", width: 1280., height: 820., session: Some("ses_limits"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "asking-vault", width: 1280., height: 820., session: Some("ses_limits"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "question", width: 1280., height: 820., session: Some("ses_cms"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "slash", width: 1280., height: 820., session: Some("ses_recipes"), typed: Some("/"), empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "narrow", width: 860., height: 760., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "steps", width: 1280., height: 900., session: Some("ses_signup"), typed: None, empty: false, opens: &[("msg_s2", 2), ("msg_s5", 0), ("msg_s5", 1)], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "search", width: 1280., height: 820., session: Some("ses_limits"), typed: None, empty: false, opens: &[], search: Some("redis"), walks: 1, attach: &[], panel: None },
    Screen { slug: "walking", width: 1280., height: 820., session: Some("ses_limits"), typed: None, empty: false, opens: &[], search: None, walks: 3, attach: &[], panel: None },
    Screen { slug: "attached", width: 1280., height: 820., session: Some("ses_dark"), typed: Some("This is how the settings look in dark mode now: the cards blend into the page."), empty: false, opens: &[], search: None, walks: 0, attach: &["shots/theme-dark.png", "kuvo_ui/src/theme.rs"], panel: None },
    Screen { slug: "files", width: 1280., height: 820., session: Some("ses_lighthouse"), typed: Some("Look at @Form"), empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "agents", width: 1280., height: 820., session: Some("ses_lighthouse"), typed: Some("Ask &"), empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "empty", width: 1280., height: 820., session: None, typed: None, empty: true, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "changes", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(changes) },
    Screen { slug: "diff", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(diff) },
    Screen { slug: "discard", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(discard) },
    Screen { slug: "tree", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(tree) },
    Screen { slug: "file", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(file) },
    Screen { slug: "branches", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(branches) },
    Screen { slug: "worktree", width: 1280., height: 820., session: Some("ses_dark"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(branches) },
    Screen { slug: "pull-request", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(pull_request) },
    Screen { slug: "panel-narrow", width: 980., height: 760., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(changes) },
    // The rail's areas (`kuvo/src/rail.rs`, `areas.rs`): Sessions with what needs you on top, each
    // other area with a row picked, a narrow window folded and opened, and the first run.
    Screen { slug: "area-sessions", width: 1280., height: 820., session: Some("ses_limits"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "area-bots", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bots) },
    Screen { slug: "area-queue", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue) },
    Screen { slug: "area-computers", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computers) },
    Screen { slug: "area-settings", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(usage) },
    Screen { slug: "area-narrow", width: 760., height: 760., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "area-narrow-open", width: 760., height: 760., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sessions_out) },
    Screen { slug: "area-first-run", width: 1280., height: 820., session: None, typed: None, empty: true, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    // Bots (`kuvo/src/bots`): a bot's page, its conversation, New Bot, each sheet, a bot waiting
    // for another computer, a narrow window and the first run.
    Screen { slug: "bot-page", width: 1280., height: 1180., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bots) },
    Screen { slug: "bot-quiet", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_quiet) },
    Screen { slug: "bot-chat", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_chat) },
    Screen { slug: "bot-new", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_new) },
    Screen { slug: "bot-new-refused", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_new_refused) },
    Screen { slug: "bot-rename", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_rename) },
    Screen { slug: "bot-job", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_job) },
    Screen { slug: "bot-look", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_look) },
    Screen { slug: "bot-delete", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_delete) },
    Screen { slug: "bot-held", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_held) },
    Screen { slug: "bot-elsewhere", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_elsewhere) },
    Screen { slug: "bot-elsewhere-delete", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_elsewhere_delete) },
    Screen { slug: "bot-waiting", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_waiting) },
    Screen { slug: "bot-none", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_none) },
    Screen { slug: "bot-narrow", width: 760., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_narrow) },
    Screen { slug: "bot-first-run", width: 1280., height: 820., session: None, typed: None, empty: true, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_none) },
    // Group chats (`kuvo/src/groups`): one mid-discussion, "&" offering its bots, one paused with
    // a wrap-up and a bot that left, an empty one, its editor, New Group and what the box turns down.
    Screen { slug: "group-chat", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(group_chat) },
    Screen { slug: "group-mention", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(group_mention) },
    Screen { slug: "group-wrap-up", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(group_wrap_up) },
    Screen { slug: "group-empty", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(group_empty) },
    Screen { slug: "group-edit", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(group_edit) },
    Screen { slug: "group-delete", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(group_delete) },
    Screen { slug: "group-new", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(group_new) },
    Screen { slug: "group-new-refused", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(group_new_refused) },
    Screen { slug: "group-narrow", width: 760., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(group_chat) },
    Screen { slug: "group-first-run", width: 1280., height: 820., session: None, typed: None, empty: true, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_none) },
    // The Work Queue (`kuvo/src/queue`): the queue under each scope, filtered and searched, an
    // item's page and its chips' menus, New Item with each of its menus, and the labels' sheets.
    Screen { slug: "queue", width: 1280., height: 1100., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_all) },
    Screen { slug: "queue-status", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_status) },
    Screen { slug: "queue-label", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_label) },
    Screen { slug: "queue-done", width: 1280., height: 1100., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_done) },
    Screen { slug: "queue-filtered", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_filtered) },
    Screen { slug: "queue-search", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_search) },
    Screen { slug: "queue-item", width: 1280., height: 1500., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_item) },
    Screen { slug: "queue-item-status", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_item_status) },
    Screen { slug: "queue-item-labels", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_item_labels) },
    Screen { slug: "queue-item-elsewhere", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_item_elsewhere) },
    Screen { slug: "queue-item-done", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_item_done) },
    Screen { slug: "queue-new", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_new) },
    Screen { slug: "queue-new-status", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_new_status) },
    Screen { slug: "queue-new-priority", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_new_priority) },
    Screen { slug: "queue-new-assignee", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_new_assignee) },
    Screen { slug: "queue-new-due", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_new_due) },
    Screen { slug: "queue-new-labels", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_new_labels) },
    Screen { slug: "queue-new-no-labels", width: 1280., height: 820., session: None, typed: None, empty: true, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_new_labels) },
    Screen { slug: "queue-new-many-labels", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_new_labels) },
    Screen { slug: "queue-edit", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_edit) },
    Screen { slug: "queue-label-new", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_label_new) },
    Screen { slug: "queue-label-edit", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_label_edit) },
    Screen { slug: "queue-label-remove", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_label_remove) },
    Screen { slug: "queue-refused", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_refused) },
    Screen { slug: "queue-first-run", width: 1280., height: 820., session: None, typed: None, empty: true, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_all) },
    Screen { slug: "queue-narrow", width: 760., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_narrow) },
    Screen { slug: "queue-item-narrow", width: 760., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_item_narrow) },
    Screen { slug: "queue-new-narrow", width: 760., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_new_narrow) },
    // Computers (`kuvo/src/computers.rs`): all of them, only this Mac, a long list, a computer's
    // page (this one, one at work, one away, one older, one shared), Add a Machine, a narrow window.
    Screen { slug: "computers", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computers_all) },
    Screen { slug: "computers-alone", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computers_all) },
    Screen { slug: "computers-many", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computers_all) },
    Screen { slug: "computer-this", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_this) },
    Screen { slug: "computer-page", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computers) },
    Screen { slug: "computer-blocked", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computers) },
    Screen { slug: "computer-away", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_away) },
    Screen { slug: "computer-older", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_older) },
    Screen { slug: "computer-shared", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_shared) },
    Screen { slug: "computer-add", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_add) },
    Screen { slug: "computer-narrow", width: 760., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_narrow) },
    // Settings (`kuvo/src/settings`): each section, the search, a section not in this app yet, and
    // each step of signing in.
    Screen { slug: "settings-row-storage", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(row_storage) },
    Screen { slug: "settings-row-storage-confirm", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(row_storage_confirm) },
    Screen { slug: "settings-row-worktrees", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(row_worktrees) },
    Screen { slug: "settings-row-git", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(row_git) },
    Screen { slug: "settings-row-archived", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(row_archived) },
    Screen { slug: "settings-a-yours", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(yours) },
    Screen { slug: "settings-a-yours-later", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(yours_later) },
    Screen { slug: "settings-a-general", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(general) },
    Screen { slug: "settings-a-accounts", width: 1280., height: 1400., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(accounts) },
    Screen { slug: "settings-a-usage", width: 1280., height: 1400., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(usage) },
    Screen { slug: "settings-a-models", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(models) },
    Screen { slug: "settings-a-permissions", width: 1280., height: 1640., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(permissions) },
    Screen { slug: "settings-a-permissions-services", width: 1280., height: 1640., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(permissions) },
    Screen { slug: "settings-a-browser", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(browser) },
    Screen { slug: "settings-a-later", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(elsewhere) },
    Screen { slug: "settings-a-machine", width: 1280., height: 1640., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(machine) },
    Screen { slug: "settings-a-picker", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(picker) },
    Screen { slug: "settings-a-away", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(away) },
    Screen { slug: "settings-a-older", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(older) },
    Screen { slug: "settings-a-unreached", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(machine) },
    Screen { slug: "settings-a-search-yours", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(search_yours) },
    Screen { slug: "settings-a-search-computer", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(search_computer) },
    Screen { slug: "settings-a-stale", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(stale) },
    Screen { slug: "settings-a-kinds", width: 1280., height: 1500., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(kinds) },
    Screen { slug: "settings-a-first-run", width: 1280., height: 820., session: None, typed: None, empty: true, opens: &[], search: None, walks: 0, attach: &[], panel: Some(usage) },
    Screen { slug: "settings-a-narrow", width: 760., height: 760., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(browser) },
    Screen { slug: "sign-in-choose", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sign_in_choose) },
    Screen { slug: "sign-in-key", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sign_in_key) },
    Screen { slug: "sign-in-form", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sign_in_form) },
    Screen { slug: "sign-in-browser", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sign_in_browser) },
    Screen { slug: "sign-in-device", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sign_in_device) },
    Screen { slug: "sign-in-paste", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sign_in_paste) },
    Screen { slug: "sign-in-done", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sign_in_done) },
    Screen { slug: "sign-in-failed", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sign_in_failed) },
    Screen { slug: "sign-in-disconnect", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(disconnect) },
    // The session menu (Draft B: row hover buttons, right-click & title-bar menus, popovers,
    // pinned ordering, archived filter, and held sessions).
    Screen { slug: "session-menu-row-hover", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(session_menu_row_hover) },
    Screen { slug: "session-menu-row-menu", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "session-menu-title-menu", width: 1280., height: 820., session: Some("ses_dark"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(session_menu_title_menu) },
    Screen { slug: "session-menu-rename", width: 1280., height: 820., session: Some("ses_dark"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(session_menu_rename) },
    Screen { slug: "session-menu-delete-worktree", width: 1280., height: 820., session: Some("ses_dark"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(session_menu_delete_worktree) },
    Screen { slug: "session-menu-pinned", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(session_menu_pinned) },
    Screen { slug: "session-menu-archived", width: 1280., height: 820., session: Some("ses_dark"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(session_menu_archived) },
    Screen { slug: "session-menu-held", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
];

/// An area's row, picked: what its page shows.
fn pick(workspace: &mut Workspace, area: Area, key: &str, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_area(area, window, cx);
    if let Some(view) = workspace.area_view(area).cloned() {
        view.update(cx, |view, cx| view.select(Some(key.to_owned()), cx));
    }
}

fn bots(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Bots, "bot:bot_mira", window, cx);
}

/// Bots' page view.
fn bots_page(workspace: &Workspace, cx: &Context<Workspace>) -> gpui::Entity<BotsView> {
    workspace.area_view(Area::Bots).and_then(|view| view.read(cx).page().cloned()).expect("Bots")
}
/// The group chats' view.
fn groups_view(workspace: &Workspace, cx: &Context<Workspace>) -> gpui::Entity<GroupsView> {
    workspace.area_view(Area::Bots).and_then(|view| view.read(cx).groups().cloned()).expect("Bots")
}
fn group_chat(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Bots, "group:grp_launch", window, cx);
}
/// "&" in a chat's composer: its bots.
fn group_mention(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Bots, "group:grp_planning", window, cx);
    let chat = groups_view(workspace, cx).read(cx).chat("grp_planning").cloned().expect("the chat");
    let field = chat.read(cx).composer().read(cx).field().clone();
    field.update(cx, |field, cx| field.set_text("What’s on this week? &", cx));
    window.focus(&field.focus_handle(cx), cx);
}
fn group_wrap_up(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Bots, "group:grp_design", window, cx);
}
fn group_empty(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Bots, "group:grp_planning", window, cx);
}
fn group_edit(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Bots, "edit-group:grp_launch", window, cx);
}
fn group_delete(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    group_edit(workspace, window, cx);
    groups_view(workspace, cx).update(cx, |view, cx| view.act(GroupAct::Delete, cx));
}
/// New Group, named, two bots picked.
fn group_new(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Bots, "new-group", window, cx);
    let view = groups_view(workspace, cx);
    let field = view.read(cx).name_field().clone();
    field.update(cx, |field, cx| field.set_text("Support desk", cx));
    view.update(cx, |view, cx| {
        view.act(GroupAct::Toggle("bot_inbox".into()), cx);
        view.act(GroupAct::Toggle("bot_mira".into()), cx);
    });
}
/// New Group with one bot: the box's words.
fn group_new_refused(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Bots, "new-group", window, cx);
    let view = groups_view(workspace, cx);
    let field = view.read(cx).name_field().clone();
    field.update(cx, |field, cx| field.set_text("Support desk", cx));
    view.update(cx, |view, cx| {
        view.act(GroupAct::Toggle("bot_inbox".into()), cx);
        view.act(GroupAct::Save, cx);
    });
}
/// A bot with nothing under way and no sessions.
fn bot_quiet(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Bots, "bot:bot_release", window, cx);
}
fn bot_chat(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_area(Area::Bots, window, cx);
    workspace.open_bot_chat("bot_mira", "ses_mira", None, window, cx);
}
/// New Bot, a template picked.
fn bot_new(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Bots, "new", window, cx);
    bots_page(workspace, cx).update(cx, |page, cx| page.act(BotAct::PickTemplate(Some("inbox".into())), cx));
}
/// New Bot, turned down by the box (no name).
fn bot_new_refused(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Bots, "new", window, cx);
    bots_page(workspace, cx).update(cx, |page, cx| page.act(BotAct::Create, cx));
}
fn bot_rename(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    bots(workspace, window, cx);
    bots_page(workspace, cx).update(cx, |page, cx| page.act(BotAct::Rename, cx));
}
fn bot_job(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    bots(workspace, window, cx);
    bots_page(workspace, cx).update(cx, |page, cx| page.act(BotAct::EditJob, cx));
}
fn bot_look(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    bots(workspace, window, cx);
    bots_page(workspace, cx).update(cx, |page, cx| page.act(BotAct::EditLook, cx));
}
fn bot_delete(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    bots(workspace, window, cx);
    bots_page(workspace, cx).update(cx, |page, cx| page.act(BotAct::Delete, cx));
}
/// A bot that lives on a computer that's away, with what the box holds for it.
fn bot_held(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Bots, "elsewhere:bot_scout", window, cx);
}
/// A change a computer turned down, in the box's words.
fn bot_waiting(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Bots, "elsewhere:bot_scout", window, cx);
}
/// A bot that lives on another of your computers, as the box lists it, with nothing waiting.
fn bot_elsewhere(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Bots, "elsewhere:bot_wren", window, cx);
}
/// Deleting it: what that computer's delete takes with it, in the box's words.
fn bot_elsewhere_delete(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    bot_elsewhere(workspace, window, cx);
    bots_page(workspace, cx).update(cx, |page, cx| page.act(BotAct::Delete, cx));
}
/// Nothing picked.
fn bot_none(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_area(Area::Bots, window, cx);
}
/// A bot's page in a narrow window (its sidebar folded away).
fn bot_narrow(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    bots(workspace, window, cx);
    workspace.rail_pressed(Area::Bots, window, cx);
}

fn queue(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Queue, "status:todo", window, cx);
}

/// The Work Queue's page view.
fn queue_page(workspace: &Workspace, cx: &Context<Workspace>) -> gpui::Entity<QueueView> {
    workspace.area_view(Area::Queue).and_then(|view| view.read(cx).queue_page().cloned()).expect("the Work Queue")
}
fn queue_all(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_area(Area::Queue, window, cx);
}
fn queue_status(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Queue, "status:todo", window, cx);
}
fn queue_label(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Queue, "label:lbl_design", window, cx);
}
/// What's closed unfolded, and what's deleted.
fn queue_done(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_all(workspace, window, cx);
    queue_page(workspace, cx).update(cx, |page, cx| {
        for fold in ["todo", "backlog", "done", "canceled", "deleted"] {
            page.act(QueueAct::Fold(fold.into()), cx);
        }
    });
}
fn queue_filtered(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_all(workspace, window, cx);
    queue_page(workspace, cx).update(cx, |page, cx| {
        page.act(QueueAct::AddFilter(Filter::Computer { key: "key_devbox".into(), name: "devbox".into() }), cx);
        page.act(QueueAct::AddFilter(Filter::Computer { key: "key_studio".into(), name: "studio-mini".into() }), cx);
    });
}
fn queue_search(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_all(workspace, window, cx);
    let field = queue_page(workspace, cx).read(cx).search_field().clone();
    field.update(cx, |field, cx| field.set_text("docs", cx));
}
fn queue_item(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Queue, "item:wqi_231", window, cx);
}
/// Changing an item on its page: its status's menu, and its labels'.
fn queue_item_status(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_item(workspace, window, cx);
    queue_page(workspace, cx).update(cx, |page, cx| page.act(QueueAct::Pick { target: Target::Item("wqi_231".into()), field: Field::Status }, cx));
}
fn queue_item_labels(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_item(workspace, window, cx);
    queue_page(workspace, cx).update(cx, |page, cx| page.act(QueueAct::Pick { target: Target::Item("wqi_231".into()), field: Field::Labels }, cx));
}
/// One on a computer that's away, as it last told it.
fn queue_item_elsewhere(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Queue, "item:wqi_242", window, cx);
}
/// An issue GitHub closed.
fn queue_item_done(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Queue, "item:wqi_250", window, cx);
}
/// New Item, written.
fn queue_writing(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> gpui::Entity<QueueView> {
    queue_all(workspace, window, cx);
    let page = queue_page(workspace, cx);
    page.update(cx, |page, cx| page.act(QueueAct::New, cx));
    let (title, body) = page.read_with(cx, |page, _| (page.item_fields().0.clone(), page.item_fields().1.clone()));
    title.update(cx, |field, cx| field.set_text("A status page for the API", cx));
    body.update(cx, |field, cx| field.set_text("Uptime for the last 90 days, and what's wrong now.\n\n- One line per service\n- Incidents, newest first", cx));
    page
}
fn queue_new(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_writing(workspace, window, cx);
}
/// New Item with one of its chips' menus open.
fn queue_new_with(field: Field, workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let page = queue_writing(workspace, window, cx);
    page.update(cx, |page, cx| page.act(QueueAct::Pick { target: Target::New, field }, cx));
}
fn queue_new_status(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_new_with(Field::Status, workspace, window, cx);
}
fn queue_new_priority(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_new_with(Field::Priority, workspace, window, cx);
}
fn queue_new_assignee(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_new_with(Field::Assignee, workspace, window, cx);
}
fn queue_new_due(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_new_with(Field::Due, workspace, window, cx);
}
fn queue_new_labels(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_new_with(Field::Labels, workspace, window, cx);
}
fn queue_new_narrow(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_new_with(Field::Priority, workspace, window, cx);
    workspace.rail_pressed(Area::Queue, window, cx);
}
fn queue_edit(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_item(workspace, window, cx);
    queue_page(workspace, cx).update(cx, |page, cx| page.act(QueueAct::Edit, cx));
}
fn queue_label_new(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_all(workspace, window, cx);
    let page = queue_page(workspace, cx);
    page.update(cx, |page, cx| page.act(QueueAct::NewLabel, cx));
    let name = page.read_with(cx, |page, _| page.name_field().clone());
    name.update(cx, |field, cx| field.set_text("Performance", cx));
}
fn queue_label_edit(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_label(workspace, window, cx);
    queue_page(workspace, cx).update(cx, |page, cx| page.act(QueueAct::EditLabel("lbl_design".into()), cx));
}
fn queue_label_remove(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_label(workspace, window, cx);
    queue_page(workspace, cx).update(cx, |page, cx| page.act(QueueAct::RemoveLabel("lbl_design".into()), cx));
}
/// A change the box turned down: an item on a computer that isn't answering.
fn queue_refused(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Queue, "status:waiting", window, cx);
    queue_page(workspace, cx).update(cx, |page, cx| page.act(QueueAct::Delete("wqi_242".into()), cx));
}
fn queue_narrow(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_all(workspace, window, cx);
    workspace.rail_pressed(Area::Queue, window, cx);
}
fn queue_item_narrow(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_item(workspace, window, cx);
    workspace.rail_pressed(Area::Queue, window, cx);
}

fn computers(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Computers, "computer:key_devbox", window, cx);
}

fn computers_all(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_area(Area::Computers, window, cx);
}
fn computer_this(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Computers, "this", window, cx);
}
fn computer_away(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Computers, "computer:key_studio", window, cx);
}
fn computer_older(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Computers, "computer:key_old", window, cx);
}
fn computer_shared(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Computers, "computer:key_build", window, cx);
}
/// Add a Machine, its field waiting.
fn computer_add(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Computers, "add", window, cx);
}
fn computer_narrow(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    computer_away(workspace, window, cx);
    workspace.rail_pressed(Area::Computers, window, cx);
}

fn usage(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_setting("Usage", window, cx);
}

/// Settings' view, on `section`.
fn settings(workspace: &mut Workspace, section: &str, window: &mut Window, cx: &mut Context<Workspace>) -> gpui::Entity<SettingsView> {
    workspace.show_setting(section, window, cx);
    workspace.area_view(Area::Settings).and_then(|view| view.read(cx).settings().cloned()).expect("Settings")
}

fn yours(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "yours/appearance", window, cx);
}
/// One of your own sections that isn't in the desktop app yet.
fn yours_later(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "yours/shortcuts", window, cx);
}
fn row_storage(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { settings(w, "storage", window, cx); }
fn row_worktrees(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = settings(w, "worktrees", window, cx);
    if let Some(row) = w.client().settings_page("worktrees").ok().and_then(|page| page.row("folder").cloned()) && let Some(words) = row.folder.clone() {
        view.update(cx, |view,cx| view.act(kuvo::settings::Act::Folder { at: kuvo::settings::At::of("worktrees", &row), words },cx));
    }
}
fn row_git(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { settings(w, "git", window, cx); }
fn row_archived(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { settings(w, "archived", window, cx); }
fn row_storage_confirm(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = settings(w, "storage", window, cx);
    if let Some(row) = w.client().settings_page("storage").ok().and_then(|page| page.row("budget").cloned()) {
        view.update(cx, |view, cx| view.act(kuvo::settings::Act::Change { at: kuvo::settings::At::of("storage", &row), value: serde_json::json!(536_870_912), asked: false }, cx));
    }
}
fn general(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "General", window, cx);
}
fn accounts(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "Accounts", window, cx);
}
fn models(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "Models", window, cx);
}
fn permissions(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "Permissions", window, cx);
}
fn browser(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "Browser", window, cx);
}
/// A computer's section that isn't in the desktop app yet.
fn elsewhere(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "Git", window, cx);
}
/// A machine's settings, the window still on this Mac.
fn machine(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "box/key_devbox/permissions", window, cx);
}
/// The computer picker, open.
fn picker(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = settings(w, "box/key_devbox/permissions", window, cx);
    view.update(cx, |view, cx| view.open_picker(window, cx));
}
/// A computer that's away, and one on an older Kuvo.
fn away(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "box/key_studio/general", window, cx);
}
fn older(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "box/key_old/general", window, cx);
}
/// "notice" typed in your settings' search: what it finds on the computer's side is listed too.
fn search_yours(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = settings(w, "yours/appearance", window, cx);
    let field = view.read(cx).search_field().clone();
    field.update(cx, |field, cx| field.set_text("the", cx));
    window.focus(&field.focus_handle(cx), cx);
}
/// "sign" typed in a computer's search.
fn search_computer(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = settings(w, "Browser", window, cx);
    let field = view.read(cx).search_field().clone();
    field.update(cx, |field, cx| field.set_text("sign", cx));
    window.focus(&field.focus_handle(cx), cx);
}
/// A switch flipped after another app changed it: the box turns it down, and says what's there.
fn stale(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = settings(w, "Browser", window, cx);
    let row = w.client().settings_page("browser").ok().and_then(|page| page.row("tidies").cloned());
    let _ = w.client().set_browser_settings(serde_json::json!({ "tidiesAgentTabs": false }));
    if let Some(row) = row {
        view.update(cx, |view, cx| view.act(kuvo::settings::Act::Change { at: kuvo::settings::At::of("browser", &row), value: serde_json::json!(false), asked: false }, cx));
    }
}
/// A row of every kind a box may describe.
fn kinds(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "samples", window, cx);
}
/// Signing in to `account`, its sheet open.
fn signing_in(w: &mut Workspace, side: Side, account: &str, name: &str, window: &mut Window, cx: &mut Context<Workspace>) -> gpui::Entity<SignIn> {
    let view = settings(w, "Accounts", window, cx);
    view.update(cx, |view, cx| view.open_sign_in(side, account, name, cx));
    view.read(cx).sign_in().cloned().expect("the sheet")
}
/// The ways in, a legacy one apart and opened, and the environment's variable.
fn sign_in_choose(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    signing_in(w, Side::Engine, "anthropic", "Anthropic", window, cx);
}
/// A key, turned down.
fn sign_in_key(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let sheet = signing_in(w, Side::Engine, "openai", "OpenAI", window, cx);
    let key = sheet.read(cx).key_field().clone();
    key.update(cx, |key, cx| key.set_text("not-a-real-key", cx));
    sheet.update(cx, |sheet, cx| sheet.submit_key(cx));
    window.focus(&key.focus_handle(cx), cx);
}
/// What a way asks before it starts (a region).
fn sign_in_form(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let sheet = signing_in(w, Side::Engine, "amazon-bedrock", "Amazon Bedrock", window, cx);
    sheet.update(cx, |sheet, cx| {
        let way = sheet.ways().methods[0].clone();
        sheet.pick(way, cx);
    });
}
/// In the browser, waiting.
fn sign_in_browser(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    signing_in(w, Side::Engine, "gitlab", "GitLab Duo", window, cx);
}
/// A device code to enter on the page.
fn sign_in_device(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    signing_in(w, Side::Engine, "github-copilot", "GitHub Copilot", window, cx);
}
/// Claude Code's, which may end on a code to paste back: the field opened, a code turned down.
fn sign_in_paste(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let sheet = signing_in(w, Side::Agents, "claude-code", "Claude Code", window, cx);
    sheet.update(cx, |sheet, cx| {
        sheet.paste_code("wrong-code", cx);
        sheet.submit_code(cx);
    });
}
fn sign_in_done(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let sheet = signing_in(w, Side::Agents, "claude-code", "Claude Code", window, cx);
    sheet.update(cx, |sheet, cx| {
        sheet.paste_code("stand-in-code", cx);
        sheet.submit_code(cx);
    });
}
/// Turned down on its page, in the box's words.
fn sign_in_failed(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let sheet = signing_in(w, Side::Engine, "poe", "Poe", window, cx);
    sheet.update(cx, |sheet, cx| sheet.ask_now(cx));
}
/// Disconnect, asked first.
fn disconnect(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = settings(w, "Accounts", window, cx);
    // As its row's Disconnect asks it: the box's action, with the box's confirm.
    view.update(cx, |view, cx| {
        if let Some(act) = view.account_act("github-copilot", "disconnect") {
            view.act(act, cx);
        }
    });
}

/// In a narrow window, the Sessions sidebar brought out by its button.
fn sessions_out(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.rail_pressed(Area::Sessions, window, cx);
}

fn changes(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_panel(Tab::Changes, cx);
}

/// The sign-up form's staged changes, opened.
fn diff(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_panel(Tab::Changes, cx);
    let view = workspace.panel().read(cx).changes().clone();
    view.update(cx, |view, cx| {
        let file = view.files().ready().and_then(|files| files.files.iter().find(|file| file.group == Group::Staged).cloned());
        if let Some(file) = file {
            view.open(file, cx);
        }
    });
}

/// Discarding the validation's changes, asked first.
fn discard(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_panel(Tab::Changes, cx);
    let view = workspace.panel().read(cx).changes().clone();
    view.update(cx, |view, cx| {
        let file = view.files().ready().and_then(|files| files.files.iter().find(|file| file.path.ends_with("validate.ts")).cloned());
        if let Some(file) = file {
            view.confirm_discard(file, cx);
        }
    });
}

/// The website's folders, a few open, one that can't be read.
fn tree(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_panel(Tab::Files, cx);
    let view = workspace.panel().read(cx).files().clone();
    let root = kuvo::stand_in::git::website();
    view.update(cx, |view, cx| {
        for folder in ["src", "src/components", "secrets"] {
            view.toggle(&format!("{root}/{folder}"), cx);
        }
    });
}

/// The sign-up form, opened to read.
fn file(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    tree(workspace, window, cx);
    let view = workspace.panel().read(cx).files().clone();
    let path = format!("{}/src/components/SignupForm.tsx", kuvo::stand_in::git::website());
    view.update(cx, |view, cx| view.open_file(&path, window, cx));
}

fn branches(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.panel().update(cx, |panel, cx| panel.show(Tab::Branches, cx));
    workspace.toggle_panel(cx);
}

/// Create Pull Request, the box's draft in it.
fn pull_request(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    branches(workspace, window, cx);
    let view = workspace.panel().read(cx).branches().clone();
    view.update(cx, |view, cx| view.write_pull_request(window, cx));
}

/// Selects in the open conversation, from the first `from` to the end of `to` after it.
fn select(workspace: &mut Workspace, from: &str, to: &str, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = workspace.conversation().expect("a conversation").clone();
    let found = view.update(cx, |view, cx| view.select_text(from, to, window, cx));
    assert!(found, "{from:?} … {to:?} isn't in the conversation");
}

fn select_paragraph(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    select(workspace, "into the success path", "under the email field?", window, cx);
}

fn select_code(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    select(workspace, "reset();", "error.message", window, cx);
}

fn select_across(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    select(workspace, "keeps what was typed", "the email field. The sign-up", window, cx);
}

/// Where `needle` (its start, or its end) shows in the open conversation.
fn spot_of(workspace: &Workspace, needle: &str, end: bool, cx: &gpui::App) -> gpui::Point<gpui::Pixels> {
    let view = workspace.conversation().expect("a conversation").read(cx);
    view.units()
        .iter()
        .flat_map(|(row, units)| units.iter().map(move |unit| (row, unit)))
        .find_map(|(row, unit)| {
            let at = unit.text.find(needle)? + if end { needle.len() } else { 0 };
            view.selection().point_for(&kuvo_ui::selection::UnitId { row: row.clone(), path: unit.path.clone() }, at)
        })
        .unwrap_or_else(|| panic!("{needle:?} isn't drawn"))
}

fn session_menu_row_hover(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.pin_session("ses_recipes", true, cx);
    workspace.set_hovered_row(Some("session-project:ses_dark".into()), cx);
}

fn session_menu_title_menu(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let menu = workspace.open_title_menu("ses_dark", window, cx);
    workspace.session_menu().title_menu.clone().open(menu, window, cx);
}

fn session_menu_rename(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.start_rename("ses_dark", kuvo::session_menu::PopoverAnchor::Row("session-project:ses_dark".into()), window, cx);
}

fn session_menu_delete_worktree(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.trigger_session_action("ses_dark", "delete", kuvo::session_menu::PopoverAnchor::Row("session-project:ses_dark".into()), window, cx);
}

fn session_menu_pinned(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.pin_session("ses_dark", true, cx);
}

fn session_menu_archived(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.archive_session("ses_dark", true, window, cx);
    workspace.archive_session("ses_recipes", true, window, cx);
    workspace.set_archived_filter(true, window, cx);
    workspace.set_hovered_row(Some("session-archived-project:ses_dark".into()), cx);
}

/// Sends what a person does with the mouse through the window, as the platform would.
fn mouse(window: &mut Window, cx: &mut gpui::App, event: gpui::PlatformInput) {
    window.dispatch_event(event, cx);
}

/// A screen of the first run: the box it meets, the window's size, and what's done in it first.
struct FirstRunScreen {
    slug: &'static str,
    scenario: Scenario,
    size: (f32, f32),
    /// Changes what the box says before the window reads it.
    before: fn(&StandIn),
    /// What's done in the first run once it's read.
    then: fn(&mut FirstRun, &mut Context<FirstRun>),
    /// What happens to the box after, the window reading it again (an update gate that closes).
    after: fn(&StandIn),
}

fn nothing_before(_: &StandIn) {}
fn nothing_then(_: &mut FirstRun, _: &mut Context<FirstRun>) {}

fn to_agents(run: &mut FirstRun, cx: &mut Context<FirstRun>) {
    run.go_on(cx);
    if run.step() == Step::Where {
        run.go_back(cx);
    }
}

fn to_where(run: &mut FirstRun, cx: &mut Context<FirstRun>) {
    to_agents(run, cx);
    run.go_on(cx);
}

const FIRST_RUN: &[FirstRunScreen] = &[
    FirstRunScreen { slug: "first-run-welcome", scenario: Scenario::Nothing, size: (1280., 820.), before: nothing_before, then: nothing_then, after: nothing_before },
    // A model runs already and nothing needs you: Get Started goes straight to where to work.
    FirstRunScreen { slug: "first-run-welcome-ready", scenario: Scenario::Running, size: (1280., 820.), before: nothing_before, then: nothing_then, after: nothing_before },
    // Before the box has said whether the first run's been had: being reached, and not answering.
    FirstRunScreen { slug: "first-run-starting", scenario: Scenario::Nothing, size: (1280., 820.), before: nothing_before, then: nothing_then, after: nothing_before },
    FirstRunScreen { slug: "first-run-unreachable", scenario: Scenario::Nothing, size: (1280., 820.), before: nothing_before, then: nothing_then, after: nothing_before },
    // The box stops answering mid-way.
    FirstRunScreen { slug: "first-run-welcome-unreachable", scenario: Scenario::Nothing, size: (1280., 820.), before: nothing_before, then: |run, cx| run.set_box(BoxState::Unreachable("Kuvo Code isn’t answering on this Mac.".into()), "This Mac".into(), cx), after: nothing_before },
    FirstRunScreen { slug: "first-run-agents", scenario: Scenario::Crowded, size: (1280., 900.), before: nothing_before, then: to_agents, after: nothing_before },
    FirstRunScreen { slug: "first-run-agents-nothing", scenario: Scenario::Nothing, size: (1280., 820.), before: nothing_before, then: to_agents, after: nothing_before },
    FirstRunScreen { slug: "first-run-agents-installing", scenario: Scenario::Nothing, size: (1280., 820.), before: nothing_before, then: |run, cx| { to_agents(run, cx); run.show_installing("agent-claude-code", cx); }, after: nothing_before },
    FirstRunScreen { slug: "first-run-agents-bridge-down", scenario: Scenario::Crowded, size: (1280., 820.), before: |stand_in| stand_in.agents_running(false), then: to_agents, after: nothing_before },
    // A sign-in in the browser, in place, with the code its page asks for.
    FirstRunScreen { slug: "first-run-signing-in", scenario: Scenario::Nothing, size: (1280., 900.), before: nothing_before, then: |run, cx| { to_agents(run, cx); run.act("account-github-copilot", cx); }, after: nothing_before },
    // An account with more than one way in: the box's ways, in place.
    FirstRunScreen { slug: "first-run-signing-in-ways", scenario: Scenario::Crowded, size: (1280., 900.), before: nothing_before, then: |run, cx| { to_agents(run, cx); run.act("agent-cursor", cx); }, after: nothing_before },
    FirstRunScreen { slug: "first-run-sign-in-failed", scenario: Scenario::Crowded, size: (1280., 900.), before: nothing_before, then: |run, cx| { to_agents(run, cx); run.act("account-poe", cx); if let Some((_, sheet)) = run.sign_in() { sheet.clone().update(cx, |sheet, cx| sheet.ask_now(cx)); } }, after: nothing_before },
    FirstRunScreen { slug: "first-run-keys", scenario: Scenario::Crowded, size: (1280., 820.), before: nothing_before, then: |run, cx| { to_agents(run, cx); run.show_keys(cx); }, after: nothing_before },
    FirstRunScreen { slug: "first-run-key-entry", scenario: Scenario::Crowded, size: (1280., 820.), before: nothing_before, then: |run, cx| { to_agents(run, cx); run.show_keys(cx); run.act("account-openai", cx); }, after: nothing_before },
    FirstRunScreen { slug: "first-run-where", scenario: Scenario::Crowded, size: (1280., 900.), before: nothing_before, then: to_where, after: nothing_before },
    FirstRunScreen { slug: "first-run-where-nothing", scenario: Scenario::Nothing, size: (1280., 820.), before: nothing_before, then: to_where, after: nothing_before },
    FirstRunScreen { slug: "first-run-where-machine", scenario: Scenario::Crowded, size: (1280., 900.), before: nothing_before, then: |run, cx| { to_where(run, cx); run.pick_computer("key_devbox", cx); }, after: nothing_before },
    FirstRunScreen { slug: "first-run-where-failed", scenario: Scenario::Running, size: (1280., 820.), before: |stand_in| stand_in.set_first_run(|first_run| first_run.folders_fail = true), then: to_where, after: nothing_before },
    FirstRunScreen { slug: "first-run-where-many-computers", scenario: Scenario::Crowded, size: (1280., 820.), before: |stand_in| stand_in.more_computers(12), then: to_where, after: nothing_before },
    FirstRunScreen { slug: "first-run-browse", scenario: Scenario::Nothing, size: (1280., 820.), before: nothing_before, then: |run, cx| { to_where(run, cx); run.browse(None, cx); }, after: nothing_before },
    FirstRunScreen { slug: "first-run-browse-unreadable", scenario: Scenario::Nothing, size: (1280., 820.), before: nothing_before, then: |run, cx| { to_where(run, cx); run.browse(Some("/Users/sam/Desktop".into()), cx); }, after: nothing_before },
    // The box turns out older than this app needs, mid-way.
    FirstRunScreen { slug: "first-run-held", scenario: Scenario::Crowded, size: (1280., 820.), before: nothing_before, then: to_agents, after: |stand_in| stand_in.set_older(true) },
    FirstRunScreen { slug: "first-run-narrow", scenario: Scenario::Crowded, size: (720., 480.), before: nothing_before, then: to_agents, after: nothing_before },
    // Done: the window's own landing view, set to start where you picked.
    FirstRunScreen { slug: "first-run-landing", scenario: Scenario::Running, size: (1280., 820.), before: nothing_before, then: |run, cx| { to_where(run, cx); run.go_on(cx); }, after: nothing_before },
    FirstRunScreen { slug: "first-run-landing-nothing", scenario: Scenario::Nothing, size: (1280., 820.), before: nothing_before, then: |run, cx| { to_where(run, cx); run.pick_place(kuvo_box::models::StartIn::Chat, cx); run.go_on(cx); }, after: nothing_before },
];

/// Renders the first run's screens (`screens-first-run-*`).
fn first_run_shots(cx: &mut HeadlessAppContext, only: &[String], out: &std::path::Path) -> anyhow::Result<()> {
    for screen in FIRST_RUN {
        if !only.is_empty() && !only.iter().any(|slug| slug == screen.slug || slug == "first-run") {
            continue;
        }
        for appearance in [Appearance::Light, Appearance::Dark] {
            cx.update(|cx| Theme::set_appearance(appearance, cx));
            let stand_in = StandIn::first_run(Replies::Never, NOW, screen.scenario);
            (screen.before)(&stand_in);
            let client = BoxClient::new(stand_in.clone());
            let window = cx.open_window(size(px(screen.size.0), px(screen.size.1)), |window, cx| cx.new(|cx| Workspace::new(client, false, window, cx)))?;
            let any = window.into();
            cx.update_window(any, |root, _, cx| {
                let workspace = root.downcast::<Workspace>().expect("the workspace");
                workspace.update(cx, |workspace, cx| {
                    workspace.set_now(NOW, cx);
                    if matches!(screen.slug, "first-run-starting" | "first-run-unreachable") {
                        workspace.await_box(cx);
                        if screen.slug == "first-run-unreachable" {
                            workspace.connected(Err(kuvo_box::Error::NotRunning("Kuvo Code isn’t answering on this Mac.".into())), cx);
                        }
                        return;
                    }
                    workspace.read_at_once(cx);
                    let run = workspace.first_run().cloned().expect("the first run");
                    run.update(cx, |run, cx| (screen.then)(run, cx));
                });
            })?;
            cx.run_until_parked();
            (screen.after)(&stand_in);
            cx.update_window(any, |root, window, cx| {
                let workspace = root.downcast::<Workspace>().expect("the workspace");
                workspace.update(cx, |workspace, cx| {
                    if workspace.first_run().is_some() && stand_in_changed(screen) {
                        workspace.read_at_once(cx);
                    }
                });
                window.refresh();
            })?;
            cx.run_until_parked();
            cx.update_window(any, |_, window, _| window.refresh())?;
            cx.run_until_parked();
            let image = cx.capture_screenshot(any)?;
            let name = format!("screens-{}-{}.png", screen.slug, if appearance.is_dark() { "dark" } else { "light" });
            image.save(out.join(&name))?;
            println!("{name}: {}x{}", image.width(), image.height());
            cx.update_window(any, |_, window, _| window.remove_window())?;
            cx.run_until_parked();
        }
    }
    Ok(())
}

/// Whether a screen changes its box after the first run is read (the window reads it again).
fn stand_in_changed(screen: &FirstRunScreen) -> bool {
    screen.slug == "first-run-held"
}

fn main() -> anyhow::Result<()> {
    let only: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../shots");
    std::fs::create_dir_all(&out)?;
    let text_system = gpui_platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(text_system, Arc::new(Assets), gpui_platform::current_headless_renderer);
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        bind_keys(cx);
    });
    for screen in SCREENS {
        if !only.is_empty() && !only.iter().any(|slug| slug == screen.slug) {
            continue;
        }
        for appearance in [Appearance::Light, Appearance::Dark] {
            cx.update(|cx| Theme::set_appearance(appearance, cx));
            let stand_in = if screen.slug.starts_with("held-box") { StandIn::held_older(Replies::Never) } else if screen.empty { StandIn::empty(Replies::Never) } else { StandIn::at(Replies::Never, NOW) };
            // The rate limits' conversation waits on a permission; another one waits behind it.
            let mut second = kuvo::stand_in::redis_permission();
            second["id"] = serde_json::json!("per_push");
            stand_in.set_asks("ses_limits", vec![kuvo::stand_in::redis_permission(), second]);
            if screen.slug == "asking-sandbox" {
                stand_in.set_asks("ses_limits", vec![kuvo::stand_in::sandbox_ask()]);
            }
            if screen.slug == "asking-vault" {
                stand_in.set_asks("ses_limits", vec![kuvo::stand_in::vault_ask()]);
            }
            // One computer turned down a change held for it, in its words.
            // A queue with many more labels than a menu shows at once.
            if screen.slug == "queue-new-many-labels" {
                stand_in.many_labels();
            }
            // Computers: only this Mac, and a long list.
            if screen.slug == "computers-alone" {
                stand_in.computers_alone();
            }
            if screen.slug == "computer-blocked" {
                stand_in.block_vouching("key_devbox");
            }
            if screen.slug == "computers-many" {
                stand_in.more_computers(40);
            }
            if screen.slug.starts_with("settings-row-") {
                use serde_json::json;
                let confirm = json!({"value":805306368,"confirm":{"title":"Keep less of older versions?","body":"Older versions take 768 MiB now, so the oldest of their copies go until they fit. The newest version of each file, memory, skills and what’s kept stay.","button":"Keep up to"}});
                let pages = [
                    ("storage","Storage","System","drive",json!([
                        {"id":"copies","title":"Copies of what agents made","kind":"status","value":1342177280,"unit":{"name":"GiB","factor":1073741824,"decimals":1,"rounding":"nearest"}},
                        {"id":"budget","title":"Older versions keep up to","line":"Past it, the oldest go first.","kind":"number","value":1073741824,"unit":{"name":"GiB","factor":1073741824,"decimals":2,"rounding":"nearest"},"can":true,"confirmBelow":confirm,"suggestions":[{"value":268435456,"title":"0.25 GiB"},{"value":536870912,"title":"0.5 GiB"},{"value":1073741824,"title":"1 GiB"},{"value":2147483648_u64,"title":"2 GiB"},{"value":5368709120_u64,"title":"5 GiB"},{"value":10737418240_u64,"title":"10 GiB"}]}
                    ])),
                    ("worktrees","Worktrees","System","folder",json!([{"id":"folder","title":"Folder","kind":"folder","value":"","placeholder":"Kuvo Code’s Folder","can":true,"folder":{"choose":"Choose a Folder…","clear":"Kuvo Code’s Folder","prompt":"Choose","message":"Choose where new worktrees go. Each one is a folder in it."}}])),
                    ("git","Git","System","branch",json!([{"id":"instructions","title":"Commit instructions","kind":"text","value":"Write a short title.\nExplain why the change matters.","lines":{"min":3,"max":8},"can":true}])),
                    ("archived","Archived Sessions","System","archive",json!([{"id":"sessions","title":"Kuvo Code","kind":"list","search":{"placeholder":"Search archived sessions","empty":"No archived session matches “{query}”."},"items":[{"id":"session","title":"Fix settings","line":"Kuvo Code","kind":"actions","actions":[{"id":"restore","title":"Restore"}]}]}]))
                ];
                for (id,title,group,icon,rows) in pages {
                    stand_in.add_settings_section(json!({"id":id,"title":title,"group":group,"icon":icon}),json!({"id":id,"title":title,"groups":[{"id":"settings","title": if id == "archived" { "Kuvo Code" } else { "" },"rows":rows}]}));
                }
            }
            if screen.slug == "settings-a-kinds" {
                let (section, page) = kuvo::stand_in::settings::every_kind();
                stand_in.add_settings_section(section, page);
            }
            if screen.slug == "settings-a-unreached" {
                stand_in.forget_peers();
            }
            if screen.slug == "settings-a-permissions-services" {
                stand_in.set_sandbox(serde_json::json!({ "on": true, "needsYou": true, "able": true, "agents": true, "gaps": ["services"], "needs": "user namespaces", "why": "Commands run in the sandbox, but this machine doesn’t let it keep them from your login’s services, like systemd. Until it allows user namespaces, agents don’t use your vault here and goals don’t run on their own.", "action": "help", "actionLabel": "How to Check…", "opens": "https://www.kuvo.run/code/docs/machines/#sandbox" }));
            }
            if screen.slug == "bot-waiting" {
                stand_in.refuse_bot_edit("bot_scout", "studio-mini", "studio-mini has no personality “Ship’s Cook”.");
            }
            if screen.slug == "session-menu-held" {
                stand_in.hold_session("ses_dark", Some("Moving to Studio…"));
            }
            let client = BoxClient::new(stand_in);
            let window = cx.open_window(size(px(screen.width), px(screen.height)), |window, cx| cx.new(|cx| Workspace::new(client, false, window, cx)))?;
            let any = window.into();
            cx.update_window(any, |root, window, cx| {
                let workspace = root.downcast::<Workspace>().expect("the workspace");
                workspace.update(cx, |workspace, cx| {
                    workspace.set_now(NOW, cx);
                    workspace.read_at_once(cx);
                    if let Some(session) = screen.session {
                        workspace.open_read_at_once(session, window, cx);
                    }
                    if let Some(view) = workspace.conversation() {
                        for (step, n) in screen.opens {
                            view.update(cx, |view, cx| view.toggle(kuvo::entries::Fold::Step((*step).to_owned(), *n), cx));
                        }
                    }
                    let composer = match workspace.conversation() {
                        Some(view) => view.read(cx).composer().clone(),
                        None => workspace.landing().clone(),
                    };
                    let field = composer.read(cx).field().clone();
                    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
                    composer.update(cx, |composer, cx| composer.attach(screen.attach.iter().map(|path| root.join(path)), cx));
                    if let Some(typed) = screen.typed {
                        field.update(cx, |field, cx| field.set_text(typed, cx));
                    }
                    window.focus(&field.focus_handle(cx), cx);
                    if let Some(panel) = screen.panel {
                        panel(workspace, window, cx);
                    }
                    if let Some(search) = screen.search {
                        workspace.search().clone().update(cx, |field, cx| field.set_text(search, cx));
                        workspace.focus_search(window, cx);
                    }
                });
            })?;
            cx.run_until_parked();
            // (After the search's change has come through, which leaves the list.)
            cx.update_window(any, |root, window, cx| {
                let workspace = root.downcast::<Workspace>().expect("the workspace");
                workspace.update(cx, |workspace, cx| {
                    for _ in 0..screen.walks {
                        workspace.walk(1, window, cx);
                    }
                });
            })?;
            cx.run_until_parked();
            cx.update_window(any, |_, window, _| window.refresh())?;
            cx.run_until_parked();
            // A right-click on the selection, a row's session menu, or a drag that's under way, through the window.
            if screen.slug == "select-menu" || screen.slug.starts_with("select-drag-") || screen.slug == "session-menu-row-menu" || screen.slug == "session-menu-held" {
                use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, PlatformInput};
                cx.update_window(any, |root, window, cx| {
                    let workspace = root.downcast::<Workspace>().expect("the workspace");
                    if screen.slug == "session-menu-row-menu" || screen.slug == "session-menu-held" {
                        let at = gpui::point(px(190.), px(338.));
                        mouse(window, cx, PlatformInput::MouseMove(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() }));
                        mouse(window, cx, PlatformInput::MouseDown(MouseDownEvent { position: at, button: MouseButton::Right, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }));
                        assert!(workspace.read(cx).session_menu().row_menus.get("session-project:ses_dark").is_some_and(|h| h.is_open()), "right-click at {at:?} missed ses_dark");
                    } else if screen.slug == "select-menu" {
                        let at = spot_of(workspace.read(cx), "success path", false, cx);
                        mouse(window, cx, PlatformInput::MouseMove(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() }));
                        mouse(window, cx, PlatformInput::MouseDown(MouseDownEvent { position: at, button: MouseButton::Right, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }));
                    } else {
                        let from = spot_of(workspace.read(cx), "Moving", false, cx);
                        let to = match screen.slug {
                            "select-drag-1" => spot_of(workspace.read(cx), "what was typed", true, cx),
                            "select-drag-2" => spot_of(workspace.read(cx), "say which", true, cx),
                            _ => spot_of(workspace.read(cx), "sets the error", true, cx),
                        };
                        mouse(window, cx, PlatformInput::MouseMove(MouseMoveEvent { position: from, pressed_button: None, modifiers: Modifiers::default() }));
                        mouse(window, cx, PlatformInput::MouseDown(MouseDownEvent { position: from, button: MouseButton::Left, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }));
                        mouse(window, cx, PlatformInput::MouseMove(MouseMoveEvent { position: to, pressed_button: Some(MouseButton::Left), modifiers: Modifiers::default() }));
                    }
                })?;
                cx.run_until_parked();
                cx.update_window(any, |_, window, _| window.refresh())?;
                cx.run_until_parked();
            }
            let image = cx.capture_screenshot(any)?;
            let name = format!("screens-{}-{}.png", screen.slug, if appearance.is_dark() { "dark" } else { "light" });
            image.save(out.join(&name))?;
            println!("{name}: {}x{}", image.width(), image.height());
            cx.update_window(any, |_, window, _| window.remove_window())?;
            cx.run_until_parked();
        }
    }
    first_run_shots(&mut cx, &only, &out)?;
    home_shots(&mut cx, &only, &out)
}

/// The window over your boxes: the switcher open on your computers, and a session open on another
/// of them (`switcher`, `elsewhere`).
fn home_shots(cx: &mut HeadlessAppContext, only: &[String], out: &std::path::Path) -> anyhow::Result<()> {
    use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput};
    for slug in ["switcher", "elsewhere"] {
        if !only.is_empty() && !only.iter().any(|only| only == slug) {
            continue;
        }
        for appearance in [Appearance::Light, Appearance::Dark] {
            cx.update(|cx| Theme::set_appearance(appearance, cx));
            let boxes = kuvo::home::stand_in_boxes(StandIn::at(Replies::Never, NOW));
            let first = boxes.nearest().expect("the stand-in is the nearest box");
            let window = cx.open_window(size(px(1280.), px(820.)), |window, cx| cx.new(|cx| kuvo::Home::new(boxes, first, false, window, cx)))?;
            let any = window.into();
            cx.update_window(any, |root, window, cx| {
                let home = root.downcast::<kuvo::Home>().expect("the home");
                home.update(cx, |home, cx| {
                    home.set_now(NOW, cx);
                    home.read_at_once(cx);
                    if slug == "elsewhere" {
                        assert!(home.switch_to("key_devbox", window, cx));
                        home.workspace().clone().update(cx, |workspace, cx| workspace.open_read_at_once("ses_releases", window, cx));
                    }
                });
            })?;
            cx.run_until_parked();
            cx.update_window(any, |_, window, _| window.refresh())?;
            cx.run_until_parked();
            if slug == "switcher" {
                // A click on the switcher, through the window.
                cx.update_window(any, |_, window, cx| {
                    let at = gpui::point(px(130.), px(22.));
                    mouse(window, cx, PlatformInput::MouseMove(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() }));
                    mouse(window, cx, PlatformInput::MouseDown(MouseDownEvent { position: at, button: MouseButton::Left, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }));
                    mouse(window, cx, PlatformInput::MouseUp(MouseUpEvent { position: at, button: MouseButton::Left, modifiers: Modifiers::default(), click_count: 1 }));
                })?;
                cx.run_until_parked();
                cx.update_window(any, |_, window, _| window.refresh())?;
                cx.run_until_parked();
            }
            let image = cx.capture_screenshot(any)?;
            let name = format!("screens-{slug}-{}.png", if appearance.is_dark() { "dark" } else { "light" });
            image.save(out.join(&name))?;
            println!("{name}: {}x{}", image.width(), image.height());
            cx.update_window(any, |_, window, _| window.remove_window())?;
            cx.run_until_parked();
        }
    }
    Ok(())
}
