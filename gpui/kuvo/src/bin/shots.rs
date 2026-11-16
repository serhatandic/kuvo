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
    Screen { slug: "composer-shell", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "composer-compact", width: 1280., height: 820., session: Some("ses_recipes"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "compaction-running", width: 1280., height: 820., session: Some("ses_recipes"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
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
    Screen { slug: "computer-add-key", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_add) },
    Screen { slug: "computer-add-progress", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_add) },
    Screen { slug: "computer-add-failed", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_add) },
    Screen { slug: "computer-add-done", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_add) },
    Screen { slug: "computer-add-cannot", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_add) },
    Screen { slug: "computer-narrow", width: 760., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_narrow) },
    // Settings (`kuvo/src/settings`): each section, the search, a section not in this app yet, and
    // each step of signing in.
    Screen { slug: "settings-row-storage", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(row_storage) },
    Screen { slug: "settings-row-storage-confirm", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(row_storage_confirm) },
    Screen { slug: "settings-row-worktrees", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(row_worktrees) },
    Screen { slug: "settings-row-worktrees-picker", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(row_worktrees_picker) },
    Screen { slug: "settings-row-storage-breakdown", width: 1280., height: 1100., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(row_storage_breakdown) },
    Screen { slug: "settings-row-storage-measuring", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(row_storage_measuring) },
    Screen { slug: "settings-row-git-narrow", width: 760., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(row_git) },
    Screen { slug: "settings-row-git", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(row_git) },
    // The rail's rings, each filled with what's left, and a ring's card open beside it.
    Screen { slug: "rail-ring-card", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(ring_card) },
    Screen { slug: "settings-row-archived", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(row_archived) },
    // Settings › MCP Servers (`kuvo/settings/mcp`), the first flow section: its servers with how
    // each stands, Add Server's form as it's typed, a sign-in the box leads, and what's wrong said.
    Screen { slug: "settings-skills", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(skills_list) },
    Screen { slug: "settings-skills-add", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(skills_add) },
    Screen { slug: "settings-commands", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(commands_list) },
    Screen { slug: "settings-commands-add", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(commands_add) },
    Screen { slug: "settings-custom-auto", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(custom_auto_list) },
    Screen { slug: "settings-custom-auto-add", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(custom_auto_add) },
    Screen { slug: "settings-mcp", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(mcp_list) },
    Screen { slug: "settings-mcp-add", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(mcp_add) },
    Screen { slug: "settings-mcp-add-error", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(mcp_add_error) },
    Screen { slug: "settings-mcp-signing-in", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(mcp_signing_in) },
    // Settings › Vault (`kuvo/settings/vault`), a machine's, as your other devices read it: what
    // agents wait for, its items, Pause and activity; an item's Edit and Who Can Use It sheets; Add
    // Item typed into; and the vault locked after a restart.
    Screen { slug: "settings-vault", width: 1280., height: 1400., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(vault_list) },
    Screen { slug: "settings-vault-item", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(vault_item) },
    Screen { slug: "settings-vault-who", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(vault_who) },
    Screen { slug: "settings-vault-add", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(vault_add) },
    Screen { slug: "settings-vault-locked", width: 1280., height: 800., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(vault_list) },
    Screen { slug: "settings-vault-narrow", width: 760., height: 1400., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(vault_list) },
    Screen { slug: "settings-mcp-narrow", width: 760., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(mcp_list) },
    // Settings › Agents (`kuvo/settings/agents`), as the real box answered it: the built-in agents
    // and yours with how each is used, Claude Code's to import, one's editor and New Agent's.
    Screen { slug: "settings-agents", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(agents_list) },
    Screen { slug: "settings-agents-edit", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(agents_edit) },
    Screen { slug: "settings-agents-new", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(agents_new) },
    Screen { slug: "settings-agents-narrow", width: 760., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(agents_list) },
    Screen { slug: "settings-a-yours", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(yours) },
    Screen { slug: "settings-yours-privacy", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(yours_privacy) },
    Screen { slug: "settings-yours-voice", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(yours_voice) },
    Screen { slug: "settings-yours-panel", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(yours_panel) },
    Screen { slug: "settings-yours-general", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(yours_general) },
    Screen { slug: "settings-yours-shortcuts", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(yours_shortcuts) },
    Screen { slug: "settings-yours-notifications", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(yours_notifications) },
    Screen { slug: "settings-yours-appearance", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(yours_appearance) },
    // Text size (`-small`, `-larger`: the smallest and largest), in Settings and in a
    // conversation, its sidebar and its composer.
    Screen { slug: "settings-yours-appearance-larger", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(yours_appearance) },
    Screen { slug: "conversation-text-small", width: 1280., height: 820., session: Some("ses_signup"), typed: Some("Can you also check the form on a narrow phone screen, and say what wraps?"), empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "conversation-text-larger", width: 1280., height: 820., session: Some("ses_signup"), typed: Some("Can you also check the form on a narrow phone screen, and say what wraps?"), empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    // A file of this Mac's, with the editor picked in General.
    Screen { slug: "file-open-in", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(file_open_in) },
    Screen { slug: "settings-a-yours-later", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(yours_later) },
    // Your settings › Updates (`settings/rows.rs`, `updates`): the app's version and its updates.
    Screen { slug: "settings-a-updates", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(updates) },
    Screen { slug: "settings-a-updates-narrow", width: 760., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(updates) },
    Screen { slug: "settings-a-clock", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(clock) },
    Screen { slug: "settings-a-accounts", width: 1280., height: 1400., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(accounts) },
    Screen { slug: "settings-a-usage", width: 1280., height: 1400., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(usage) },
    Screen { slug: "settings-a-models", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(models) },
    Screen { slug: "settings-a-permissions", width: 1280., height: 1640., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(permissions) },
    Screen { slug: "settings-a-permissions-services", width: 1280., height: 1640., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(permissions) },
    Screen { slug: "settings-a-browser", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(browser) },
    // Settings › This Mac (`kuvo/settings/computer`): how Kuvo stands here; what needs you; a
    // confirmation in the box's words; what macOS allows Kuvo Helper.
    Screen { slug: "settings-this-mac", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(this_mac) },
    Screen { slug: "settings-this-mac-allowed", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(this_mac) },
    Screen { slug: "settings-this-mac-attention", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(this_mac) },
    Screen { slug: "settings-this-mac-restart", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(this_mac_restart) },
    Screen { slug: "settings-this-mac-devices-off", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(this_mac_devices) },
    Screen { slug: "settings-this-mac-narrow", width: 760., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(this_mac) },
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
    // The bars over the composer (#764): one command left running and one subagent at work; many,
    // one waiting on you; folded; a command's Output; a bot's sessions at work; narrow.
    Screen { slug: "bars-one", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "bars-many", width: 1280., height: 900., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "bars-folded", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bars_folded) },
    Screen { slug: "bars-output", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bars_output) },
    Screen { slug: "bars-bot", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_chat) },
    Screen { slug: "bars-narrow", width: 760., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    // The mode menu and Full Access the first time; a new session's Local or Worktree.
    Screen { slug: "mode-chip-held", width: 1280., height: 820., session: Some("ses_recipes"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "mode-menu", width: 1280., height: 820., session: Some("ses_recipes"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "mode-full-access", width: 1280., height: 820., session: Some("ses_recipes"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(full_access_question) },
    Screen { slug: "landing-worktree", width: 1280., height: 820., session: None, typed: Some("Make the footer links work on mobile"), empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(landing_worktree) },
    Screen { slug: "landing-worktree-menu", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(landing_worktree) },
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
/// The bars, the command's folded into its line.
fn bars_folded(workspace: &mut Workspace, _window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(view) = workspace.conversation() {
        view.read(cx).bars().clone().update(cx, |bars, cx| bars.toggle(kuvo::work_bars::Bar::Background, cx));
    }
}
/// The command's Output, open.
fn bars_output(workspace: &mut Workspace, _window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(view) = workspace.conversation() {
        view.read(cx).bars().clone().update(cx, |bars, cx| bars.toggle_output("sh_dev", cx));
    }
}
/// Full Access picked the first time on this computer: asked in the composer.
fn full_access_question(workspace: &mut Workspace, _window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(view) = workspace.conversation() {
        view.read(cx).composer().clone().update(cx, |composer, cx| composer.pick_approval(kuvo_box::approval::Approval::FullAccess, cx));
    }
}
/// A new session in the website project, set to start in a worktree of its own.
fn landing_worktree(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.new_session(None, window, cx);
    workspace.pick_place(kuvo_box::models::StartIn::Project("/Users/sam/Projects/website".into()), cx);
    workspace.landing().clone().update(cx, |composer, cx| {
        composer.read_at_once(cx);
        composer.set_worktree(true, cx);
    });
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
fn updates(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "yours/updates", window, cx);
}
/// General, with the editors a Mac might have.
fn yours_general(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = settings(w, "yours/general", window, cx);
    let editors = [("com.microsoft.VSCode", "Visual Studio Code"), ("dev.zed.Zed", "Zed"), ("com.apple.TextEdit", "TextEdit")].map(|(id, name)| kuvo::yours::editors::Editor { id: id.into(), name: name.into() }).to_vec();
    view.update(cx, |view, cx| view.set_editors(editors, cx));
}
fn yours_notifications(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "yours/notifications", window, cx);
}
fn yours_appearance(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "yours/appearance", window, cx);
}
/// The sign-up form, opened to read, with TextEdit picked (on every Mac). (Kept only while the
/// shots run; the screens after it don't open files.)
fn file_open_in(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    kuvo::yours::change(kuvo::yours::Change::Editor(Some("com.apple.TextEdit".into())), cx);
    file(workspace, window, cx);
}
/// One of your own sections that isn't in the desktop app yet.
fn yours_later(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "yours/voice", window, cx);
}
fn row_storage(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { settings(w, "storage", window, cx); }
fn row_worktrees(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { settings(w, "worktrees", window, cx); }
/// The worktree folder's picker, opened from the folder in its field.
fn row_worktrees_picker(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = settings(w, "worktrees", window, cx);
    if let Some(row) = w.client().settings_page("worktrees").ok().and_then(|page| page.row("worktrees-folder").cloned()) && let Some(words) = row.folder.clone() {
        view.update(cx, |view,cx| view.act(kuvo::settings::Act::Folder { at: kuvo::settings::At::of("worktrees", &row), words },cx));
    }
}
/// Storage with its breakdown open.
fn row_storage_breakdown(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = settings(w, "storage", window, cx);
    view.update(cx, |view, cx| view.act(kuvo::settings::Act::Expand("row-used".into()), cx));
}
/// Storage as it's first measured: no parts yet.
fn row_storage_measuring(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { settings(w, "storage", window, cx); }
fn row_git(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { settings(w, "git", window, cx); }
fn row_archived(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { settings(w, "archived", window, cx); }
/// Open an Add form entirely from the section's declared rows.
fn flow_add(w: &mut Workspace, section: &str, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = settings(w, section, window, cx);
    let Some(page) = w.client().settings_page(section).ok() else { return };
    let Some(row) = page.groups.iter().flat_map(|group| &group.rows).find(|row| row.add.as_ref().is_some_and(|add| add.form.is_some())).cloned() else { return };
    let add = row.add.clone().expect("Add");
    view.update(cx, |view, cx| view.act(kuvo::settings::Act::Form { at: kuvo::settings::At::of(section, &row), action: add.id, form: Box::new(add.form.expect("form")) }, cx));
}
fn skills_list(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { settings(w, "skills", window, cx); }
fn skills_add(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { flow_add(w, "skills", window, cx); }
fn commands_list(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { settings(w, "commands", window, cx); }
fn commands_add(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { flow_add(w, "commands", window, cx); }
fn custom_auto_list(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { settings(w, "custom-auto", window, cx); }
fn custom_auto_add(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { flow_add(w, "custom-auto", window, cx); }
fn mcp_list(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { settings(w, "mcp", window, cx); }
/// Add Server's form, typed into as someone would: the box's words under each field.
fn mcp_form(w: &mut Workspace, name: &str, command: &str, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = settings(w, "mcp", window, cx);
    let Some(row) = w.client().settings_page("mcp").ok().and_then(|page| page.row("servers").cloned()) else { return };
    let Some(add) = row.add.clone() else { return };
    let form = add.form.clone().expect("a form");
    view.update(cx, |view, cx| view.act(kuvo::settings::Act::Form { at: kuvo::settings::At::of("mcp", &row), action: add.id.clone(), form: Box::new(form) }, cx));
    if let Some(sheet) = view.read(cx).form().cloned() {
        sheet.update(cx, |sheet, cx| {
            sheet.set_text("name", name, cx);
            sheet.set_text("command", command, cx);
        });
    }
}
fn mcp_add(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { mcp_form(w, "my files", "npx -y @modelcontextprotocol/server-filesystem ~/Projects", window, cx); }
fn mcp_add_error(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { mcp_form(w, "files", "serve-files --root ~/Projects", window, cx); }
fn agents_list(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { settings(w, "agents", window, cx); }
/// An agent's form, opened from its row (`row`, `action`), with `name` typed into New Agent's.
fn agents_form(w: &mut Workspace, row: &str, action: &str, name: Option<&str>, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = settings(w, "agents", window, cx);
    let Some(at) = w.client().settings_page("agents").ok().and_then(|page| page.row(row).cloned()) else { return };
    let Some(form) = at.actions.iter().chain(at.add.iter()).find(|each| each.id == action).and_then(|each| each.form.clone()) else { return };
    view.update(cx, |view, cx| view.act(kuvo::settings::Act::Form { at: kuvo::settings::At::of("agents", &at), action: action.into(), form: Box::new(form) }, cx));
    if let (Some(name), Some(sheet)) = (name, view.read(cx).form().cloned()) {
        sheet.update(cx, |sheet, cx| sheet.set_text("name", name, cx));
    }
}
fn agents_edit(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { agents_form(w, "agent-reviewer", "edit", None, window, cx); }
fn agents_new(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { agents_form(w, "agents", "add", Some("Bug Hunter"), window, cx); }
fn vault_list(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { settings(w, "vault", window, cx); }
/// One of the vault's items' sheets, by its action, for the item called `title`.
fn vault_sheet(w: &mut Workspace, title: &str, action: &str, window: &mut Window, cx: &mut Context<Workspace>) -> Option<gpui::Entity<kuvo::settings::form::FormSheet>> {
    let view = settings(w, "vault", window, cx);
    let page = w.client().settings_page("vault").ok()?;
    let row = page.row("items")?.items.iter().find(|item| item.title == title)?.clone();
    let form = row.actions.iter().find(|each| each.id == action)?.form.clone()?;
    view.update(cx, |view, cx| view.act(kuvo::settings::Act::Form { at: kuvo::settings::At::of("vault", &row), action: action.into(), form: Box::new(form) }, cx));
    view.read(cx).form().cloned()
}
fn vault_item(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { vault_sheet(w, "GitHub", "edit", window, cx); }
fn vault_who(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { vault_sheet(w, "GitHub", "who", window, cx); }
/// Add Item, a sign-in typed into, its password too short so far.
fn vault_add(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = settings(w, "vault", window, cx);
    let Some(row) = w.client().settings_page("vault").ok().and_then(|page| page.row("items").cloned()) else { return };
    let Some(add) = row.add.clone() else { return };
    let form = add.form.clone().expect("a form");
    view.update(cx, |view, cx| view.act(kuvo::settings::Act::Form { at: kuvo::settings::At::of("vault", &row), action: add.id.clone(), form: Box::new(form) }, cx));
    if let Some(sheet) = view.read(cx).form().cloned() {
        sheet.update(cx, |sheet, cx| {
            sheet.set_text("name", "Linear", cx);
            sheet.set_text("site", "linear.app", cx);
            sheet.set_text("username", "me@example.com", cx);
            sheet.set_text("password", "abc", cx);
        });
    }
}
/// A server's sign-in the box leads, under way.
fn mcp_signing_in(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = settings(w, "mcp", window, cx);
    let Some(row) = w.client().settings_page("mcp").ok().and_then(|page| page.row("server-linear").cloned()) else { return };
    view.update(cx, |view, cx| view.act(kuvo::settings::Act::Do { at: kuvo::settings::At::of("mcp", &row), action: "sign-in".into(), confirm: None, asked: false }, cx));
}
fn row_storage_confirm(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = settings(w, "storage", window, cx);
    if let Some(row) = w.client().settings_page("storage").ok().and_then(|page| page.row("older-budget").cloned()) {
        view.update(cx, |view, cx| view.act(kuvo::settings::Act::Change { at: kuvo::settings::At::of("storage", &row), value: serde_json::json!(536_870_912), asked: false }, cx));
    }
}
fn this_mac(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "computer", window, cx);
}
/// A row's action or change on This Mac's page, asked first in the box's words.
fn this_mac_asks(w: &mut Workspace, row: &str, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = settings(w, "computer", window, cx);
    let Some(row) = w.client().settings_page("computer").ok().and_then(|page| page.row(row).cloned()) else { return };
    let at = kuvo::settings::At::of("computer", &row);
    let act = match row.actions.first() {
        Some(action) => kuvo::settings::Act::Do { at, action: action.id.clone(), confirm: action.confirm.clone(), asked: false },
        None => kuvo::settings::Act::Change { at, value: serde_json::json!(false), asked: false },
    };
    view.update(cx, |view, cx| view.act(act, cx));
}
fn this_mac_restart(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    this_mac_asks(w, "state", window, cx);
}
fn this_mac_devices(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    this_mac_asks(w, "devices", window, cx);
}
fn clock(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "Clock", window, cx);
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
    settings(w, "box/key_studio/clock", window, cx);
}
fn older(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "box/key_old/clock", window, cx);
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

fn ring_card(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.rings().update(cx, |rings, cx| rings.show_card(0, cx));
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

/// A stand-in helper for the tile to offer, in a folder of the pictures' own (taken away after).
fn shots_helper() -> PathBuf {
    std::env::temp_dir().join(format!("kuvo-shots-allow-{}", std::process::id())).join("Kuvo Helper.app")
}

/// A Mac's own Kuvo whose helper says neither permission is allowed.
fn nothing_allowed(stand_in: &StandIn) {
    let helper = shots_helper();
    std::fs::create_dir_all(&helper).expect("a stand-in helper");
    let bundle = helper.to_string_lossy().into_owned();
    stand_in.set_permissions(|permissions| *permissions = kuvo::stand_in::permissions::Permissions { bundle, ..kuvo::stand_in::permissions::Permissions::nothing_allowed() });
}

fn to_allow(run: &mut FirstRun, cx: &mut Context<FirstRun>) {
    run.go_on(cx);
    if run.step() == Step::Agents {
        run.go_on(cx);
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
    FirstRunScreen { slug: "first-run-welcome-unreachable", scenario: Scenario::Nothing, size: (1280., 820.), before: nothing_before, then: |run, cx| run.set_box(BoxState::Unreachable("Kuvo isn’t answering on this Mac.".into()), "This Mac".into(), cx), after: nothing_before },
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
    // A Mac whose Kuvo says macOS doesn't allow Kuvo Helper either yet: a step of its own after the
    // agents; and with System Settings opened on one added by hand (its tile isn't in the picture).
    FirstRunScreen { slug: "first-run-allow", scenario: Scenario::Nothing, size: (1280., 820.), before: nothing_allowed, then: to_allow, after: nothing_before },
    FirstRunScreen { slug: "first-run-allow-asking", scenario: Scenario::Nothing, size: (1280., 820.), before: |stand_in| { nothing_allowed(stand_in); stand_in.set_permissions(|permissions| permissions.accessibility = true); }, then: |run, cx| {
        to_allow(run, cx);
        if let Some(permissions) = run.permissions().cloned() {
            permissions.update(cx, |permissions, cx| {
                permissions.open_settings("screenRecording", cx);
                if let Some(tile) = permissions.tile() {
                    tile.update(cx, |_, window, _| window.remove_window()).ok();
                }
            });
        }
    }, after: nothing_before },
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
                            workspace.connected(Err(kuvo_box::Error::NotRunning("Kuvo isn’t answering on this Mac.".into())), cx);
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
    if let Some(made) = shots_helper().parent().filter(|made| made.exists()) {
        std::fs::remove_dir_all(made)?;
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
        // Drawn as on a Mac, which asks Touch ID or its password before what lets something use
        // what's yours more (never asked here: nothing is pressed).
        cx.set_global(kuvo::confirm_you::ConfirmYou(std::sync::Arc::new(|_: &str| false)));
        bind_keys(cx);
        // The app's updater, made up: a release that updates itself, last checked two hours ago.
        kuvo::updates::Updates::install(std::rc::Rc::new(kuvo::updates::stand_in::StandIn::new("0.8.0", Some(NOW - 2.0 * 3_600_000.0))), None, cx);
    });
    for screen in SCREENS {
        if !only.is_empty() && !only.iter().any(|slug| slug == screen.slug) {
            continue;
        }
        for appearance in [Appearance::Light, Appearance::Dark] {
            cx.update(|cx| Theme::set_appearance(appearance, cx));
            // The text size: the default, but where a screen's name says (kept only while the
            // shots run).
            let text_size = if screen.slug.ends_with("-small") {
                kuvo::yours::TextSize::Small
            } else if screen.slug.ends_with("-larger") {
                kuvo::yours::TextSize::Larger
            } else {
                kuvo::yours::TextSize::Default
            };
            cx.update(|cx| kuvo::yours::change(kuvo::yours::Change::TextSize(text_size), cx));
            let stand_in = if screen.slug.starts_with("held-box") { StandIn::held_older(Replies::Never) } else if screen.empty { StandIn::empty(Replies::Never) } else { StandIn::at(Replies::Never, NOW) };
            if screen.slug.starts_with("settings-commands") {
                stand_in.add_settings_section(serde_json::json!({"id":"commands","title":"Commands","group":"Agents & Tools","icon":"terminal","line":"Prompts you use often, run by typing / and their name."}), serde_json::from_str(include_str!("../stand_in/settings/commands.json")).expect("Commands fixture"));
            }
            if screen.slug.starts_with("settings-custom-auto") {
                stand_in.add_settings_section(kuvo::stand_in::settings::custom_auto::section(), kuvo::stand_in::settings::custom_auto::page());
            }
            if screen.slug.starts_with("settings-skills") {
                stand_in.add_settings_section(kuvo::stand_in::settings::skills::section(), kuvo::stand_in::settings::skills::page());
            }
            if screen.slug.starts_with("settings-mcp") {
                stand_in.show_mcp();
            }
            if screen.slug.starts_with("settings-agents") {
                stand_in.show_agents();
            }
            if screen.slug.starts_with("settings-vault") {
                stand_in.show_vault();
                if screen.slug == "settings-vault-locked" {
                    stand_in.with_vault(|vault| vault.locked = true);
                }
            }
            if screen.slug == "compaction-running" { stand_in.running_compaction(); }
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
            // Add a Machine, as far as each of its states.
            match screen.slug {
                "computer-add-key" => stand_in.adding_at("mac-mini", "asking"),
                "computer-add-progress" => stand_in.adding_at("mac-mini", "copying"),
                "computer-add-failed" => stand_in.adding_at("nowhere.example", "asking"),
                "computer-add-done" => stand_in.adding_at("mac-mini", "done"),
                "computer-add-cannot" => stand_in.cannot_add("Add and remove computers from Studio."),
                _ => {}
            }
            if screen.slug == "computers-many" {
                stand_in.more_computers(40);
            }
            if screen.slug.starts_with("settings-row-") {
                // Git, Worktrees and Storage as the box describes them (settings-pages.ts), with
                // made-up values.
                for (section, page) in kuvo::stand_in::settings::described_pages(screen.slug == "settings-row-storage-measuring") {
                    stand_in.add_settings_section(section, page);
                }
            }
            if screen.slug == "settings-a-kinds" {
                let (section, page) = kuvo::stand_in::settings::every_kind();
                stand_in.add_settings_section(section, page);
            }
            if screen.slug.starts_with("settings-this-mac") {
                use kuvo::stand_in::permissions::Permissions;
                // (All put off, so the page is seen without the heads-up over it.)
                let later = vec!["screenRecording".to_owned(), "accessibility".to_owned()];
                match screen.slug {
                    "settings-this-mac-attention" | "settings-this-mac-narrow" => {
                        stand_in.set_permissions(|permissions| *permissions = Permissions { accessibility: true, later, ..Permissions::nothing_allowed() });
                        stand_in.set_computer(|computer| {
                            computer.at_login = false;
                            computer.ready = Some("1.0.1".into());
                        });
                    }
                    "settings-this-mac-restart" => stand_in.set_computer(|computer| computer.working = true),
                    "settings-this-mac" | "settings-this-mac-devices-off" => {}
                    _ => stand_in.set_permissions(|permissions| *permissions = Permissions { screen_recording: true, accessibility: true, ..Permissions::nothing_allowed() }),
                }
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
            if screen.slug.starts_with("bars-") && screen.slug != "bars-bot" {
                let many = screen.slug == "bars-many";
                let command = |id: &str, words: &str, ago: f64| serde_json::json!({ "id": id, "command": words, "started": NOW - ago, "folder": "/Users/sam/Projects/website" });
                let subagent = |id: &str, title: &str, asking: bool| serde_json::json!({ "session": id, "since": NOW - 40_000.0, "title": title, "asking": asking });
                let commands = if many { vec![command("sh_dev", "npm run dev", 754_000.0), command("sh_test", "npx vitest --watch", 212_000.0), command("sh_tail", "tail -f logs/signup.log", 61_000.0)] } else { vec![command("sh_dev", "npm run dev", 95_000.0)] };
                let subagents = if many { vec![subagent("ses_sub", "Check the other forms", false), subagent("ses_sub2", "Find where the email check runs", true), subagent("ses_sub3", "Write a test for a taken email", false)] } else { vec![subagent("ses_sub", "Check the other forms", false)] };
                stand_in.set_background("ses_signup", serde_json::json!({ "commands": commands, "subagents": subagents }), &[("sh_dev", "> website@2.4.0 dev\n> vite\n\n  VITE v6.0.3  ready in 412 ms\n\n  ➜  Local:   http://localhost:5173/\n  ➜  Network: use --host to expose")]);
            }
            if screen.slug == "mode-chip-held" {
                stand_in.set_approvals(false, Some("The sandbox isn’t running on this computer."));
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
            if screen.slug == "composer-shell" || screen.slug == "composer-compact" {
                let text = if screen.slug == "composer-shell" { "! ls" } else { "/compact" };
                let session = screen.session.expect("a session");
                cx.update_window(any, |root, _window, cx| {
                    let workspace = root.downcast::<Workspace>().expect("the workspace");
                    workspace.update(cx, |workspace, cx| {
                        let composer = workspace.conversation().expect("the conversation").read(cx).composer().clone();
                        composer.update(cx, |composer, cx| {
                            composer.field().clone().update(cx, |field, cx| field.set_text(text, cx));
                            composer.send(cx);
                        });
                    });
                })?;
                cx.run_until_parked();
                cx.update_window(any, |root, window, cx| {
                    let workspace = root.downcast::<Workspace>().expect("the workspace");
                    workspace.update(cx, |workspace, cx| {
                        workspace.open_read_at_once(session, window, cx);
                        let field = workspace.conversation().expect("the conversation").read(cx).composer().read(cx).field().clone();
                        window.focus(&field.focus_handle(cx), cx);
                    });
                })?;
                cx.run_until_parked();
            }
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
            // A composer's menu opened with a press on its chip, through the window.
            if let Some(at) = match screen.slug {
                "mode-menu" => Some(gpui::point(px(526.), px(767.))),
                "landing-worktree-menu" => Some(gpui::point(px(835.), px(412.))),
                _ => None,
            } {
                use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput};
                cx.update_window(any, |_, window, cx| {
                    mouse(window, cx, PlatformInput::MouseMove(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() }));
                    mouse(window, cx, PlatformInput::MouseDown(MouseDownEvent { position: at, button: MouseButton::Left, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }));
                    mouse(window, cx, PlatformInput::MouseUp(MouseUpEvent { position: at, button: MouseButton::Left, modifiers: Modifiers::default(), click_count: 1 }));
                })?;
                cx.run_until_parked();
                cx.update_window(any, |_, window, _| window.refresh())?;
                cx.run_until_parked();
            }
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
    launch_shots(&mut cx, &only, &out)?;
    home_shots(&mut cx, &only, &out)
}

/// The app opening on a computer (`launch-*`: this Mac's row in the Welcome card as it's set up,
/// once it's ready, and where it stopped; the quiet middle for one that isn't running; after a move,
/// what macOS allows Kuvo Helper, `launch-moved-allow`), and the tile beside a stand-in for System
/// Settings' window (`permission-tile`). `SHOTS_HELPER_ICON`: an `.icns` for the tile's stand-in helper
/// (`scripts/helper-icon.sh`'s); without it the tile draws a plain one.
fn launch_shots(cx: &mut HeadlessAppContext, only: &[String], out: &std::path::Path) -> anyhow::Result<()> {
    use kuvo::launch::{Launch, Moving, Opens, Said, Stage};
    use kuvo::stand_in::permissions::Permissions;
    use kuvo_box::install::{OnlyAttaches, Verb};
    let wanted = |slug: &str, group: &str| only.is_empty() || only.iter().any(|only| only == slug || only == group);
    let save = |cx: &mut HeadlessAppContext, any: gpui::AnyWindowHandle, slug: &str, appearance: Appearance| -> anyhow::Result<()> {
        cx.run_until_parked();
        cx.update_window(any, |_, window, _| window.refresh())?;
        cx.run_until_parked();
        let image = cx.capture_screenshot(any)?;
        let name = format!("screens-{slug}-{}.png", if appearance.is_dark() { "dark" } else { "light" });
        image.save(out.join(&name))?;
        println!("{name}: {}x{}", image.width(), image.height());
        cx.update_window(any, |_, window, _| window.remove_window())?;
        cx.run_until_parked();
        Ok(())
    };
    let offline = "The engine couldn’t be downloaded. Check that this Mac is online, then try again.";
    let not_yet = "This Mac has 2.1 GB free, and moving needs 6.4 GB. Free up some space, then try again.";
    // A move's steps, as the box lists them (`kuvo/box/rehome`'s `steps`).
    let steps: Vec<(String, gpui::SharedString)> = [
        ("plan", "Checking this Mac"),
        ("install", "Getting ready"),
        ("keys", "Handing over your keys"),
        ("clone", "Copying your sessions and settings"),
        ("upgrade", "Bringing your settings along"),
        ("database", "Bringing your sessions up to date"),
        ("rewrite", "Pointing everything at its new place"),
        ("check", "Checking nothing is missing"),
        ("place", "Putting everything in place"),
        ("fix", "Reconnecting your worktrees"),
        ("clear", "Finishing up"),
    ]
    .into_iter()
    .map(|(step, title)| (step.to_owned(), title.into()))
    .collect();
    let stuck = "Kuvo was moving your sessions to their new place on this Mac and stopped part way. Try again to finish.";
    let two = "This Mac is set up as one of your machines, and has an earlier Kuvo Code's sessions too: putting them together isn't in this version yet.";
    let dropped = "Websites you were signed in to in Kuvo Code's browser need signing in again. Your history and saved passwords come along.";
    for slug in [
        "launch-setting-up", "launch-setting-up-step", "launch-starting", "launch-done", "launch-failed", "launch-failed-narrow", "launch-not-running",
        "launch-moving", "launch-move-getting-ready", "launch-move-starting", "launch-move-refused", "launch-move-refused-narrow", "launch-move-stuck", "launch-move-waiting-long", "launch-moved", "launch-moved-dropped", "launch-two", "launch-moved-allow",
    ] {
        if !wanted(slug, "launch") {
            continue;
        }
        for appearance in [Appearance::Light, Appearance::Dark] {
            cx.update(|cx| Theme::set_appearance(appearance, cx));
            // (No installer is run for a picture: each stage is shown as it stands.)
            let box_here = (slug == "launch-done").then(|| StandIn::first_run(Replies::Never, NOW, Scenario::Nothing));
            let attach: Arc<dyn Fn() -> kuvo::launch::Attached + Send + Sync> = Arc::new(move || match box_here.clone() {
                Some(stand_in) => {
                    let boxes = kuvo::home::stand_in_boxes(stand_in);
                    let first = boxes.nearest().expect("the stand-in is the nearest box");
                    Ok((boxes, first))
                }
                None => Err("Kuvo isn’t set up on this Mac.".into()),
            });
            let opens = Opens { waits_within: kuvo::launch::WAITS_WITHIN, installer: Err(OnlyAttaches("This build of Kuvo doesn’t set up a computer. It connects to Kuvo where it’s already set up.".into())), attach };
            let (width, height) = if slug.ends_with("-narrow") { (720., 620.) } else { (1280., 820.) };
            let steps = steps.clone();
            let window = cx.open_window(size(px(width), px(height)), |window, cx| cx.new(|cx| Launch::new(opens, false, window, cx)))?;
            let any = window.into();
            cx.run_until_parked();
            cx.update_window(any, |root, window, cx| {
                let launch = root.downcast::<Launch>().expect("the launch");
                launch.update(cx, |launch, cx| match slug {
                    "launch-setting-up" => launch.show(Stage::SettingUp { verb: Verb::New, title: None, at: None }, true, cx),
                    "launch-setting-up-step" => launch.show(Stage::SettingUp { verb: Verb::New, title: Some("Downloading the engine".into()), at: None }, true, cx),
                    "launch-moving" => launch.show_move(Stage::SettingUp { verb: Verb::Move, title: Some("Copying your sessions and settings".into()), at: Some((4, 11)) }, Moving { steps, at: Some(4), of: Some(11), ..Moving::default() }, None, cx),
                    "launch-move-getting-ready" => launch.show_move(Stage::SettingUp { verb: Verb::Move, title: None, at: None }, Moving { steps, title: Some("Downloading the engine".into()), ..Moving::default() }, None, cx),
                    "launch-move-starting" => launch.show_move(Stage::Starting, Moving { steps, at: Some(11), of: Some(11), title: Some("Starting Kuvo on this Mac".into()), finishing: true, ..Moving::default() }, None, cx),
                    "launch-move-refused" | "launch-move-refused-narrow" => launch.show_move(Stage::Stopped { line: "Couldn’t move your sessions".into(), message: not_yet.into(), note: None }, Moving { steps, at: Some(1), of: Some(11), stopped_at: Some(1), ..Moving::default() }, None, cx),
                    "launch-move-stuck" => launch.show_move(Stage::Stopped { line: "Couldn’t finish moving".into(), message: stuck.into(), note: None }, Moving { steps, at: Some(11), of: Some(11), stopped_at: Some(11), ..Moving::default() }, None, cx),
                    "launch-move-waiting-long" => launch.show_move(Stage::Stopped { line: "Still moving your sessions".into(), message: "It’s taking longer than expected on this Mac. It goes on by itself: try again in a while.".into(), note: None }, Moving { steps, at: Some(6), of: Some(11), ..Moving::default() }, None, cx),
                    "launch-moved" | "launch-moved-dropped" | "launch-two" => {
                        let boxes = kuvo::home::stand_in_boxes(StandIn::at(Replies::Never, NOW));
                        let first = boxes.nearest().expect("the stand-in is the nearest box");
                        let what = if slug == "launch-two" { Said::Left(two.into()) } else { Said::Moved };
                        let dropped = if slug == "launch-moved-dropped" { vec![dropped.into(), "A bot on your machines was described only on this Mac by the earlier Kuvo Code, and may not show there. Its work is still on that machine: make it again if it's missing.".into()] } else { Vec::new() };
                        launch.show_move(Stage::Said { what, boxes, first }, Moving { steps, finishing: true, dropped, ..Moving::default() }, (slug != "launch-two").then_some("Kuvo Helper"), cx)
                    }
                    "launch-moved-allow" => {
                        let stand_in = StandIn::at(Replies::Never, NOW);
                        stand_in.set_permissions(|permissions| *permissions = Permissions { accessibility: true, ..Permissions::nothing_allowed() });
                        let boxes = kuvo::home::stand_in_boxes(stand_in);
                        let first = boxes.nearest().expect("the stand-in is the nearest box");
                        launch.show_allow(boxes, first, window, cx);
                    }
                    "launch-starting" => launch.show(Stage::Starting, true, cx),
                    "launch-failed" | "launch-failed-narrow" => launch.show(Stage::Stopped { line: "Couldn’t set up Kuvo".into(), message: offline.into(), note: None }, true, cx),
                    "launch-done" => {
                        let home = launch.home().cloned().expect("the app, open");
                        home.update(cx, |home, cx| {
                            home.set_now(NOW, cx);
                            home.read_at_once(cx);
                        });
                    }
                    _ => {}
                });
            })?;
            save(cx, any, slug, appearance)?;
        }
    }
    if wanted("permission-tile", "permission") {
        // A stand-in helper bundle in a folder of its own, with the icon given, if any.
        let made = std::env::temp_dir().join(format!("kuvo-shots-helper-{}", std::process::id()));
        let bundle = made.join("Kuvo Helper.app");
        std::fs::create_dir_all(bundle.join("Contents/Resources"))?;
        if let Some(icon) = std::env::var_os("SHOTS_HELPER_ICON") {
            std::fs::copy(icon, bundle.join("Contents/Resources/AppIcon.icns"))?;
        }
        for appearance in [Appearance::Light, Appearance::Dark] {
            cx.update(|cx| Theme::set_appearance(appearance, cx));
            let bundle = bundle.clone();
            let window = cx.open_window(size(px(1120.), px(700.)), |_, cx| {
                let tile = cx.new(|_| kuvo::permissions::tile::Tile::new("Kuvo Helper".into(), bundle));
                cx.new(|_| BesideSettings { tile })
            })?;
            save(cx, window.into(), "permission-tile", appearance)?;
        }
        std::fs::remove_dir_all(&made)?;
    }
    Ok(())
}

/// The tile as it sits beside System Settings, for a picture: a stand-in for that window (a plain
/// panel with a made-up list, nothing of macOS's) and the real tile, placed by the app's own rule.
struct BesideSettings {
    tile: gpui::Entity<kuvo::permissions::tile::Tile>,
}

impl gpui::Render for BesideSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        use gpui::{ParentElement as _, Styled as _, div, point};
        use kuvo::permissions::tile;
        use kuvo_ui::{ActiveTheme as _, StyledType as _, TypeSize};
        let c = cx.theme().colors.clone();
        let screen = gpui::Bounds::new(point(px(0.), px(0.)), window.viewport_size());
        let settings = gpui::Bounds::new(point(px(120.), px(70.)), size(px(640.), px(520.)));
        let at = tile::beside(settings, screen);
        let row = |name: &'static str, on: bool| {
            div().flex().items_center().gap(px(10.)).px(px(14.)).py(px(10.)).border_b_1().border_color(c.border)
                .child(div().size(px(22.)).rounded(px(6.)).bg(c.sunken).border_1().border_color(c.border))
                .child(div().flex_1().text_ui(TypeSize::Small).text_color(c.text).child(name))
                .child(div().w(px(34.)).h(px(20.)).rounded(px(10.)).bg(if on { c.success } else { c.border_strong }))
        };
        div()
            .size_full()
            .relative()
            .bg(c.background)
            .child(
                div().absolute().left(settings.left()).top(settings.top()).w(settings.size.width).h(settings.size.height).flex().flex_col().rounded(px(12.)).bg(c.surface).border_1().border_color(c.border).overflow_hidden()
                    .child(div().px(px(18.)).py(px(14.)).border_b_1().border_color(c.border).flex().flex_col().gap(px(2.))
                        .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("A stand-in for System Settings › Privacy & Security"))
                        .child(div().text_ui(TypeSize::Body).medium().text_color(c.text).child("Screen Recording")))
                    .child(div().m(px(18.)).rounded(px(10.)).border_1().border_color(c.border).bg(c.card).flex().flex_col().child(row("Another App", true)).child(row("A Second App", false)).child(div().px(px(14.)).py(px(10.)).text_ui(TypeSize::Small).text_color(c.text_faint).child("+   −"))),
            )
            .child(div().absolute().left(at.x).top(at.y).w(tile::SIZE.width).h(tile::SIZE.height).shadow(cx.theme().elevated_shadow()).rounded(px(18.)).child(self.tile.clone()))
    }
}

/// The window over your boxes: the switcher open on your computers, a session open on another
/// of them (`switcher`, `elsewhere`), the first Quit's note (`quit-note`), and another computer
/// whose Kuvo is older than this app, its update chip's card open: waiting a long while, with
/// Update Now (`update-waiting`), and being updated (`update-running`); one the box that lists it
/// sets up again (`update-setup`), and one nothing here reaches over SSH (`update-ssh`). And this
/// Mac's own Kuvo waiting for the release this app carries (`update-own-waiting`), and taking it
/// (`update-own-running`).
fn home_shots(cx: &mut HeadlessAppContext, only: &[String], out: &std::path::Path) -> anyhow::Result<()> {
    use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput};
    for slug in ["switcher", "elsewhere", "quit-note", "update-waiting", "update-running", "update-setup", "update-ssh", "update-own-waiting", "update-own-running"] {
        if !only.is_empty() && !only.iter().any(|only| only == slug) {
            continue;
        }
        for appearance in [Appearance::Light, Appearance::Dark] {
            cx.update(|cx| Theme::set_appearance(appearance, cx));
            let stand_in = StandIn::at(Replies::Never, NOW);
            let own = slug.starts_with("update-own-");
            if own {
                stand_in.own_update_waits(if slug == "update-own-running" { "updating" } else { "waiting" }, 2);
            } else if slug.starts_with("update-") {
                stand_in.peer_box("key_devbox").expect("devbox").set_older(true);
                if slug == "update-setup" {
                    stand_in.sets_up_again("key_devbox");
                } else if slug != "update-ssh" {
                    stand_in.bring_up_to_date("key_devbox", if slug == "update-running" { "updating" } else { "waiting" }, slug == "update-waiting", 2);
                }
            }
            let boxes = kuvo::home::stand_in_boxes(stand_in);
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
                    if slug.starts_with("update-") {
                        if !own {
                            assert!(home.switch_to("key_devbox", window, cx));
                        }
                        home.workspace().clone().update(cx, |workspace, cx| workspace.toggle_update_card(cx));
                    }
                    if slug == "quit-note" {
                        home.quit(window, cx);
                        assert!(home.quit_note_shown(), "the stand-in is in its Mac's menu bar");
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

fn yours_privacy(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { settings(w, "yours/privacy", window, cx); }
fn yours_voice(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { settings(w, "yours/voice", window, cx); }

fn yours_panel(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "yours/panel", window, cx);
}

fn yours_shortcuts(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) { settings(w, "yours/shortcuts", window, cx); }
