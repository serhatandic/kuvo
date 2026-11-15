//! Renders Kuvo's screens offscreen against the stand-in box, in light and dark, to
//! `gpui/shots/screens-<screen>-{light,dark}.png`. No window opens, nothing real is reached.
//!
//! `cargo run -p kuvo --features shots --bin shots [-- <screen>...]`

use std::{path::PathBuf, sync::Arc};

use gpui::{AppContext as _, Context, Focusable as _, HeadlessAppContext, Window, px, size};
use kuvo::area::Area;
use kuvo::panel::Tab;
use kuvo_box::git::Group;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::bots::{Act as BotAct, BotsView};
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
    // Settings (`kuvo/src/settings`): each section, the search, a section not in this app yet, and
    // each step of signing in.
    Screen { slug: "settings-general", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(general) },
    Screen { slug: "settings-accounts", width: 1280., height: 1400., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(accounts) },
    Screen { slug: "settings-usage", width: 1280., height: 1100., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(usage) },
    Screen { slug: "settings-models", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(models) },
    Screen { slug: "settings-permissions", width: 1280., height: 960., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(permissions) },
    Screen { slug: "settings-notifications", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(notifications) },
    Screen { slug: "settings-browser", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(browser) },
    Screen { slug: "settings-search", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(settings_search) },
    Screen { slug: "settings-elsewhere", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(elsewhere) },
    Screen { slug: "settings-first-run", width: 1280., height: 820., session: None, typed: None, empty: true, opens: &[], search: None, walks: 0, attach: &[], panel: Some(usage) },
    Screen { slug: "settings-narrow", width: 760., height: 760., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(browser) },
    Screen { slug: "sign-in-choose", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sign_in_choose) },
    Screen { slug: "sign-in-key", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sign_in_key) },
    Screen { slug: "sign-in-form", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sign_in_form) },
    Screen { slug: "sign-in-browser", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sign_in_browser) },
    Screen { slug: "sign-in-device", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sign_in_device) },
    Screen { slug: "sign-in-paste", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sign_in_paste) },
    Screen { slug: "sign-in-done", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sign_in_done) },
    Screen { slug: "sign-in-failed", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sign_in_failed) },
    Screen { slug: "sign-in-disconnect", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(disconnect) },
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

fn usage(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_setting("Usage", window, cx);
}

/// Settings' view, on `section`.
fn settings(workspace: &mut Workspace, section: &str, window: &mut Window, cx: &mut Context<Workspace>) -> gpui::Entity<SettingsView> {
    workspace.show_setting(section, window, cx);
    workspace.area_view(Area::Settings).and_then(|view| view.read(cx).settings().cloned()).expect("Settings")
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
fn notifications(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "Notifications", window, cx);
}
fn browser(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "Browser", window, cx);
}
/// A section that isn't in this app yet.
fn elsewhere(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, "Git", window, cx);
}
/// "sign" typed in Settings' search.
fn settings_search(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = settings(w, "Browser", window, cx);
    let field = view.read(cx).search_field().clone();
    field.update(cx, |field, cx| field.set_text("sign", cx));
    window.focus(&field.focus_handle(cx), cx);
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

/// Sends what a person does with the mouse through the window, as the platform would.
fn mouse(window: &mut Window, cx: &mut gpui::App, event: gpui::PlatformInput) {
    window.dispatch_event(event, cx);
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
            let stand_in = if screen.empty { StandIn::empty(Replies::Never) } else { StandIn::at(Replies::Never, NOW) };
            // The rate limits' conversation waits on a permission; another one waits behind it.
            let mut second = kuvo::stand_in::redis_permission();
            second["id"] = serde_json::json!("per_push");
            stand_in.set_asks("ses_limits", vec![kuvo::stand_in::redis_permission(), second]);
            // One computer turned down a change held for it, in its words.
            // A queue with many more labels than a menu shows at once.
            if screen.slug == "queue-new-many-labels" {
                stand_in.many_labels();
            }
            if screen.slug == "bot-waiting" {
                stand_in.refuse_bot_edit("bot_scout", "studio-mini", "studio-mini has no personality “Ship’s Cook”.");
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
            // A right-click on the selection, or a drag that's under way, through the window.
            if screen.slug == "select-menu" || screen.slug.starts_with("select-drag-") {
                use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, PlatformInput};
                cx.update_window(any, |root, window, cx| {
                    let workspace = root.downcast::<Workspace>().expect("the workspace");
                    if screen.slug == "select-menu" {
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
    Ok(())
}
