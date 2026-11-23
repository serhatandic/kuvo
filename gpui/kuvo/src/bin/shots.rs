//! Renders Kuvo's screens offscreen against the stand-in box, in light and dark, to
//! `gpui/shots/screens-<screen>-{light,dark}.png`. No window opens, nothing real is reached.
//!
//! `../scripts/cargo-locked.sh run -p kuvo --features shots --bin shots [-- <screen>...]`

use std::{path::PathBuf, sync::Arc};

use gpui::{AppContext as _, Context, Focusable as _, HeadlessAppContext, Window, px, size};
use kuvo::area::Area;
use kuvo::panel::Kind;
use kuvo_box::git::Group;
use kuvo::first_run::{FirstRun, Step};
use kuvo::stand_in::areas::UsageSample;
use kuvo::stand_in::first_run::Scenario;
use kuvo::stand_in::moves::Scenario as MoveScenario;
use kuvo_box::moves::{Kind as MoveKind, Target as MoveTarget};
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

/// Two quotes over the composer: the first answered, the second's answer open, with the keyboard.
fn quotes_answered(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = workspace.conversation().expect("the conversation").clone();
    view.update(cx, |view, cx| view.quote("Moving reset() into the success path keeps what was typed.", window, cx));
    let composer = view.read(cx).composer().clone();
    if let Some(field) = composer.read(cx).answer_field(0) {
        field.update(cx, |field, cx| field.set_text("Yes, do that, and keep the focus on the email field.", cx));
    }
    view.update(cx, |view, cx| view.quote("The 409 now sets the error on the email field.", window, cx));
}

/// Two quotes answered, sent: in the conversation, each answer under its quote, in the order written.
fn quotes_sent(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = workspace.conversation().expect("the conversation").clone();
    let composer = view.read(cx).composer().clone();
    let answers = [
        ("Moving reset() into the success path keeps what was typed.", "Yes, do that, and keep the focus on the email field."),
        ("The 409 now sets the error on the email field.", "Good. Say which address is taken in the message too."),
    ];
    for (at, (quote, answer)) in answers.into_iter().enumerate() {
        view.update(cx, |view, cx| view.quote(quote, window, cx));
        if let Some(field) = composer.read(cx).answer_field(at) {
            field.update(cx, |field, cx| field.set_text(answer, cx));
        }
    }
    composer.update(cx, |composer, cx| composer.send(cx));
}

/// The menu beside Send open, as a click opens it.
fn send_menu_open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let composer = workspace.conversation().expect("the conversation").read(cx).composer().clone();
    composer.update(cx, |composer, cx| composer.open_send_menu(window, cx));
}

/// The model picker open from the composer's chip (#775): on the model's provider, a search typed,
/// and another provider picked in the rail.
fn model_picker(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> Option<gpui::Entity<kuvo::model_picker::ModelPicker>> {
    let composer = workspace.conversation().expect("the conversation").read(cx).composer().clone();
    composer.update(cx, |composer, cx| composer.open_model_picker(window, cx))
}
fn model_picker_open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    model_picker(workspace, window, cx);
}
fn model_picker_search(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(picker) = model_picker(workspace, window, cx) {
        picker.update(cx, |picker, cx| picker.type_search("son", cx));
    }
}
fn model_picker_provider(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(picker) = model_picker(workspace, window, cx) {
        picker.update(cx, |picker, cx| picker.show_rail("openai", cx));
    }
}

/// Your accounts with Claude Code (#833): its tab with one account, with two (the switch), with ten
/// (the switch's list open), and a search across them.
/// (From a new conversation's composer, which offers the coding agents' models.)
fn landing_model_picker(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> Option<gpui::Entity<kuvo::model_picker::ModelPicker>> {
    let composer = workspace.landing().clone();
    composer.update(cx, |composer, cx| composer.open_model_picker(window, cx))
}
fn model_picker_accounts(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(picker) = landing_model_picker(workspace, window, cx) {
        picker.update(cx, |picker, cx| picker.show_rail("claude", cx));
    }
}
fn model_picker_accounts_open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(picker) = landing_model_picker(workspace, window, cx) {
        picker.update(cx, |picker, cx| {
            picker.show_rail("claude", cx);
            picker.open_accounts(window, cx);
        });
    }
}
/// The one picker (#975), from a new conversation's composer on Codex's GPT-6-Luna at Medium, as
/// T3's reference: on Codex, on Claude Code, on ★, a search across providers, a row highlighted
/// with its ☆, and the permissions open.
fn on_luna(workspace: &mut Workspace, cx: &mut Context<Workspace>) {
    let composer = workspace.landing().clone();
    composer.update(cx, |composer, cx| composer.pick(kuvo_box::typed::ModelRef { provider_id: "codex-cli".into(), id: "gpt-6-luna".into(), variant: Some("medium".into()) }, cx));
}
fn model_chip_luna(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    on_luna(workspace, cx);
}
fn picker_on(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>, rail: &str) -> Option<gpui::Entity<kuvo::model_picker::ModelPicker>> {
    on_luna(workspace, cx);
    let picker = landing_model_picker(workspace, window, cx)?;
    if !rail.is_empty() {
        picker.update(cx, |picker, cx| picker.show_rail(rail, cx));
    }
    Some(picker)
}
fn model_picker_codex(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    picker_on(workspace, window, cx, "");
}
fn model_picker_claude_code(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    picker_on(workspace, window, cx, "claude");
}
fn model_picker_favourites(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    picker_on(workspace, window, cx, "starred");
}
fn model_picker_search_all(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(picker) = picker_on(workspace, window, cx, "") {
        picker.update(cx, |picker, cx| picker.type_search("gpt 6", cx));
    }
}
fn model_picker_row_hover(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(picker) = picker_on(workspace, window, cx, "") {
        picker.update(cx, |picker, cx| picker.highlight(2, cx));
    }
}
fn model_picker_permissions(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(picker) = picker_on(workspace, window, cx, "") {
        picker.update(cx, |picker, cx| picker.open_permissions(cx));
    }
}
/// The conversation's picker with its permissions open (what the mode menu was).
fn mode_menu(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(picker) = model_picker(workspace, window, cx) {
        picker.update(cx, |picker, cx| picker.open_permissions(cx));
    }
}

fn model_picker_accounts_search(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(picker) = landing_model_picker(workspace, window, cx) {
        picker.update(cx, |picker, cx| picker.type_search("opus 5.5", cx));
    }
}

fn bot_wake_list(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    bot_chat(workspace, window, cx);
    let chat = workspace.bot_chat().unwrap().1.clone();
    chat.update(cx, |chat, cx| chat.open_wake_schedule(window, cx));
}

const SCREENS: &[Screen] = &[
    Screen { slug: "bot-wake-row-waking", width: 1200., height: 800., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_none) },
    Screen { slug: "bot-wake-row-scheduled", width: 1200., height: 800., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_none) },
    Screen { slug: "bot-wake-badge-hover", width: 1200., height: 800., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_none) },
    Screen { slug: "bot-wake-row-none", width: 1200., height: 800., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_none) },
    Screen { slug: "bot-wake-chip", width: 1200., height: 800., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_chat) },
    Screen { slug: "bot-wake-list-many", width: 1200., height: 800., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_wake_list) },
    Screen { slug: "bot-wake-list-long-note", width: 1200., height: 800., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_wake_list) },
    Screen { slug: "bot-wake-list-empty", width: 1200., height: 800., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_wake_list) },
    // The model picker (#775): open on the conversation's provider, searching, another provider;
    // and the composer's row with a message written (the paperclip, the picks and Send on one line).
    Screen { slug: "model-picker", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(model_picker_open) },
    Screen { slug: "model-picker-search", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(model_picker_search) },
    Screen { slug: "model-picker-accounts-one", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(model_picker_accounts) },
    Screen { slug: "model-picker-accounts-two", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(model_picker_accounts) },
    Screen { slug: "model-picker-accounts-ten-open", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(model_picker_accounts_open) },
    Screen { slug: "model-picker-accounts-search", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(model_picker_accounts_search) },
    // The one picker, laid out as T3's (#975).
    Screen { slug: "model-chip", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(model_chip_luna) },
    Screen { slug: "model-picker-codex", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(model_picker_codex) },
    Screen { slug: "model-picker-claude-code", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(model_picker_claude_code) },
    Screen { slug: "model-picker-favourites", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(model_picker_favourites) },
    Screen { slug: "model-picker-search-all", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(model_picker_search_all) },
    Screen { slug: "model-picker-row-hover", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(model_picker_row_hover) },
    Screen { slug: "model-picker-permissions", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(model_picker_permissions) },
    Screen { slug: "market-add-agent-picker", width: 1280., height: 980., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "settings-a-models-picker", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(models) },
    Screen { slug: "model-picker-provider", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(model_picker_provider) },
    Screen { slug: "composer-edited-attachments", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(composer_edited) },
    Screen { slug: "composer-row", width: 1280., height: 820., session: Some("ses_signup"), typed: Some("Check the sign-up form once more"), empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "compaction-running", width: 1280., height: 820., session: Some("ses_recipes"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "held-box-sessions", width: 1280., height: 820., session: None, typed: None, empty: true, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "held-box", width: 1280., height: 820., session: None, typed: None, empty: true, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_none) },
    Screen { slug: "conversation", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    // #949: a failed step, closed (one short line) and opened (all it said).
    Screen { slug: "conversation-failed-step", width: 1280., height: 820., session: Some("ses_dark"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "conversation-failed-step-open", width: 1280., height: 820., session: Some("ses_dark"), typed: None, empty: false, opens: &[("msg_fail_a", 1), ("msg_fail_b", 0)], search: None, walks: 0, attach: &[], panel: None },
    // #950: a subagent under its session, opened: its model and approval said, no choice; one that takes no messages.
    Screen { slug: "conversation-subagent", width: 1280., height: 820., session: Some("ses_sub"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "conversation-subagent-no-messages", width: 1280., height: 820., session: Some("ses_sub"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    // #951: a long project's list, "Show more" tapped once: 16 shown, with Show 20 more and Show fewer.
    Screen { slug: "sidebar-show-more", width: 1280., height: 1400., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(show_more_once) },
    // While the agent works (#776): a message written, the way beside Send, and the queue with one
    // steered in, one queued and another agent's letter; then the menu beside Send open.
    Screen { slug: "send-while-working", width: 1280., height: 820., session: Some("ses_signup"), typed: Some("Also keep the focus on the email field after the error shows"), empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    // Replying to two parts of a conversation, each with its answer (0.7's `QuoteList`).
    Screen { slug: "quotes-answers", width: 1280., height: 820., session: Some("ses_signup"), typed: Some("Then run the sign-up tests again"), empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(quotes_answered) },
    // Then sent: each answer under its quote in the conversation, in the order written.
    Screen { slug: "quotes-sent", width: 1280., height: 820., session: Some("ses_signup"), typed: Some("Then run the sign-up tests again"), empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(quotes_sent) },
    Screen { slug: "send-while-working-menu", width: 1280., height: 820., session: Some("ses_signup"), typed: Some("Also keep the focus on the email field after the error shows"), empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(send_menu_open) },
    // What the agent did between its words, grouped: folded to a line (one that failed shows
    // anyway; the one under way on its line), then opened; and messages from other agents as cards.
    Screen { slug: "tool-groups", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "tool-groups-open", width: 1280., height: 1100., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(tool_groups_open) },
    // Pictures in a conversation: yours as thumbnails on your messages (one, three, a tall one, a wide
    // one), the agent's larger; and one opened in the viewer.
    Screen { slug: "pictures-yours", width: 1280., height: 1500., session: Some("ses_recipes"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "pictures-viewer", width: 1280., height: 820., session: Some("ses_recipes"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(open_a_picture) },
    // Files shown by the agent (`kuvo-browser_show_file`, Flow layout): single picture + 3-picture
    // strip, then PDF & Markdown cards, missing tombstones, and a still-loading call.
    Screen { slug: "show-file-pictures", width: 1280., height: 1080., session: Some("ses_dark"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "show-file-cards", width: 1280., height: 1040., session: Some("ses_dark"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    // Inline tool-call details in the real conversation view: command, edit, Work Queue update,
    // scheduled & canceled wake-ups, failed call, and an unknown MCP tool.
    Screen { slug: "tool-details", width: 1280., height: 1420., session: Some("ses_dark"), typed: None, empty: false, opens: &[("msg_td_a1", 0), ("msg_td_a1", 1), ("msg_td_a1", 3), ("msg_td_a1", 4), ("msg_td_a1", 7)], search: None, walks: 0, attach: &[], panel: None },
    // Unified From / To agent message cards: received message, sent mid-turn outside the step
    // group, sent with waited reply, and a failed send.
    Screen { slug: "agent-messages", width: 1280., height: 820., session: Some("ses_dark"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "text-overflow", width: 1080., height: 820., session: Some("ses_dark"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(text_overflow_open) },
    Screen { slug: "work-item-hover", width: 1080., height: 760., session: Some("ses_dark"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "reply-items", width: 1080., height: 760., session: Some("ses_dark"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(hover_reply) },
    // The same, the pointer on the chip in its words: the item's hover card.
    Screen { slug: "reply-items-hover", width: 1080., height: 760., session: Some("ses_dark"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "agent-letters", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(agent_letters) },
    // Selecting text (`kuvo_ui/src/selection.rs`): across a paragraph, inside a code block, across
    // messages, the right-click menu over a selection, and a drag in three steps.
    Screen { slug: "select-paragraph", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(select_paragraph) },
    Screen { slug: "select-code", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(select_code) },
    Screen { slug: "select-across", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(select_across) },
    Screen { slug: "select-menu", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(select_paragraph) },
    Screen { slug: "message-hover-yours", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "message-hover-agent", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "select-finished", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "select-drag-1", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(reveal_drag_anchor) },
    Screen { slug: "select-drag-2", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(reveal_drag_anchor) },
    Screen { slug: "select-drag-3", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(reveal_drag_anchor) },
    Screen { slug: "landing", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    // Every computer in one window: devbox's sessions under their project; devbox switching to a
    // new release; devbox off a while.
    Screen { slug: "sessions-all-computers", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "sessions-computer-updating", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "sessions-computer-away", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    // Runs on beside the model in a new session's composer, devbox away (closed, then open), and a
    // computer's settings under Computers › that computer.
    Screen { slug: "runs-on", width: 1280., height: 820., session: None, typed: Some("Look at the deploy logs"), empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "runs-on-open", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(runs_on_open) },
    Screen { slug: "computer-settings", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_settings) },
    Screen { slug: "landing-sky-dawn", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sky_dawn) },
    Screen { slug: "landing-sky-day", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sky_day) },
    Screen { slug: "landing-sky-dusk", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sky_dusk) },
    Screen { slug: "landing-sky-night", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sky_night) },
    Screen { slug: "asking", width: 1280., height: 820., session: Some("ses_limits"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    // What the box asks itself: a command outside the sandbox, an item from the vault.
    Screen { slug: "asking-sandbox", width: 1280., height: 820., session: Some("ses_limits"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "asking-vault", width: 1280., height: 820., session: Some("ses_limits"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "question", width: 1280., height: 820., session: Some("ses_cms"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "narrow", width: 860., height: 760., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "steps", width: 1280., height: 900., session: Some("ses_signup"), typed: None, empty: false, opens: &[("msg_s2", 2), ("msg_s5", 2), ("msg_s5", 4)], search: None, walks: 0, attach: &[], panel: None },
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
    Screen { slug: "panel-narrow", width: 860., height: 760., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(changes) },
    // The side panel (#796), each tab as it shows, as draft 1 drew them.
    Screen { slug: "panel-browser", width: 1340., height: 840., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(panel_browser) },
    Screen { slug: "panel-terminal", width: 1340., height: 840., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(panel_terminal) },
    Screen { slug: "panel-new-tab", width: 1340., height: 840., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(panel_new_tab) },
    Screen { slug: "panel-file", width: 1340., height: 840., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(panel_file) },
    Screen { slug: "panel-changes", width: 1340., height: 840., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(changes) },
    Screen { slug: "panel-process", width: 1340., height: 840., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(panel_process) },
    Screen { slug: "panel-subagents", width: 1340., height: 840., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(panel_subagents) },
    Screen { slug: "panel-subagent-detail", width: 1340., height: 840., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(panel_subagent_detail) },
    Screen { slug: "panel-side-chat", width: 1340., height: 840., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(panel_side_chat) },
    Screen { slug: "panel-many-tabs", width: 1340., height: 840., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(panel_many_tabs) },
    Screen { slug: "panel-bot-chat", width: 1340., height: 840., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(panel_bot_chat) },
    Screen { slug: "panel-bot-side-chat", width: 1340., height: 840., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(panel_bot_side_chat) },
    // The rail's areas (`kuvo/src/rail.rs`, `areas.rs`): Sessions with what needs you on top, each
    // other area with a row picked, a narrow window folded and opened, and the first run.
    Screen { slug: "area-sessions", width: 1280., height: 820., session: Some("ses_limits"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "area-bots", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bots) },
    Screen { slug: "titlebar-bots-folded", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bots_folded) },
    Screen { slug: "titlebar-session", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "sidebar-long-computer", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue) },
    Screen { slug: "sidebar-wide", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bots) },
    Screen { slug: "sidebar-narrow-sessions", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "sidebar-narrow-bots", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bots) },
    Screen { slug: "sidebar-search-results", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sidebar_search_results) },
    Screen { slug: "sidebar-search-empty", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sidebar_search_empty) },
    Screen { slug: "titlebar-session-folded", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sidebar_folded) },
    Screen { slug: "area-queue", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue) },
    Screen { slug: "area-computers", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computers) },
    Screen { slug: "area-settings", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(usage) },
    Screen { slug: "area-narrow", width: 760., height: 760., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "area-narrow-open", width: 760., height: 760., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sessions_out) },
    Screen { slug: "area-first-run", width: 1280., height: 820., session: None, typed: None, empty: true, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    // Bots (`kuvo/src/bots`): a bot's page, its conversation, New Bot, each sheet, a bot waiting
    // for another computer, a narrow window and the first run.
    Screen { slug: "bot-edit-top", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bots) },
    Screen { slug: "bot-edit-scrolled", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bots) },
    Screen { slug: "bot-edit-name", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_rename) },
    Screen { slug: "bot-edit-avatar-hover", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bots) },
    Screen { slug: "bot-edit-long-personality", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bots) },
    Screen { slug: "bot-edit-personality-picker", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bots) },
    Screen { slug: "bot-edit-all-options", width: 1280., height: 1320., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bots) },
    Screen { slug: "bot-page", width: 1280., height: 1180., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bots) },
    Screen { slug: "bot-move-row", width: 1280., height: 1180., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bots) },
    Screen { slug: "bot-move-menu", width: 1280., height: 1180., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_move_menu) },
    Screen { slug: "bot-move-confirm", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_move_confirm) },
    Screen { slug: "bot-move-moving", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bots) },
    Screen { slug: "bot-move-failed", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_move_result) },
    Screen { slug: "bot-move-done", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_move_result) },
    Screen { slug: "bot-quiet", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_quiet) },
    Screen { slug: "bot-chat", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_chat) },
    Screen { slug: "bot-new", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_new) },
    Screen { slug: "bot-new-refused", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_new_refused) },
    Screen { slug: "bot-rename", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_rename) },
    Screen { slug: "bot-job", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_job) },
    Screen { slug: "bot-look", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_look) },
    Screen { slug: "bot-delete", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_delete) },
    Screen { slug: "bot-held", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_held) },
    // A bot on another computer, its conversation open: the title names it.
    Screen { slug: "bot-chat-elsewhere", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_chat_elsewhere) },
    Screen { slug: "bot-elsewhere", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_elsewhere) },
    Screen { slug: "bot-elsewhere-delete", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_elsewhere_delete) },
    Screen { slug: "bot-waiting", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_waiting) },
    Screen { slug: "bot-none", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_none) },
    Screen { slug: "bot-narrow", width: 550., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_narrow) },
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
    Screen { slug: "queue-loading", width: 1280., height: 1100., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_loading) },
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
    Screen { slug: "queue-new-empty", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_new_empty) },
    Screen { slug: "queue-new-picture", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_new_picture) },
    Screen { slug: "queue-new-enlarged", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_new_enlarged) },
    Screen { slug: "queue-edit-pictures", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_edit) },
    Screen { slug: "queue-item-hover-card", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_item) },
    Screen { slug: "queue-item-status-tooltip", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_item) },
    Screen { slug: "queue-item-references", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_item) },
    Screen { slug: "queue-item-links", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_item) },
    Screen { slug: "queue-collapsed-lanes", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(queue_collapsed_lanes) },
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
    Screen { slug: "computers-versions", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computers_all) },
    Screen { slug: "computers-many", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computers_all) },
    Screen { slug: "computer-this", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_this) },
    Screen { slug: "computer-page", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computers) },
    Screen { slug: "computer-blocked", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computers) },
    Screen { slug: "computer-away", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_away) },
    Screen { slug: "computer-older", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_older) },
    Screen { slug: "computer-shared", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_shared) },
    Screen { slug: "computer-add", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_add) },
    Screen { slug: "computer-add-key", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_add) },
    Screen { slug: "computer-add-recover", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_add) },
    Screen { slug: "computer-add-progress", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_add) },
    Screen { slug: "computer-add-failed", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_add) },
    Screen { slug: "computer-add-done", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_add) },
    Screen { slug: "computer-add-cannot", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_add) },
    Screen { slug: "computer-narrow", width: 760., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_narrow) },
    // Managing them (Computers A): who leads your setup, renaming in place, changing the main
    // computer, removing one (unlinked or wiped, one that's away), the last one, and after.
    Screen { slug: "computers-roles", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computers_all) },
    Screen { slug: "computer-roles", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computers) },
    Screen { slug: "computer-rename", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_rename) },
    Screen { slug: "computers-handover", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computers_handover) },
    Screen { slug: "computer-remove", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_remove) },
    Screen { slug: "computer-remove-away", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_remove_away) },
    Screen { slug: "computer-remove-last", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computer_remove_last) },
    Screen { slug: "computers-removed", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(computers_removed) },
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
    // The rings past the rail's room, open beside "+2", and one of their cards.
    Screen { slug: "rail-more", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(rings_more) },
    Screen { slug: "rail-more-card", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(rings_more_card) },
    // The box's cards (#952): on pace to run out, used up, Codex's resets and its confirm, an
    // agent's many accounts (20, and 4), one before its first reading, and one Google doesn't count.
    Screen { slug: "rail-card-claude-max", width: 1100., height: 900., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(ring_card) },
    Screen { slug: "rail-card-claude-used-up", width: 1100., height: 900., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(ring_card_1) },
    Screen { slug: "rail-card-codex-resets", width: 1100., height: 900., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(ring_card_2) },
    Screen { slug: "rail-card-codex-confirm", width: 1100., height: 900., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(ring_card_confirm) },
    Screen { slug: "rail-card-antigravity-20", width: 1100., height: 900., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(ring_card_3) },
    Screen { slug: "rail-card-antigravity-4", width: 1100., height: 900., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(ring_card) },
    Screen { slug: "rail-card-codex-no-resets", width: 1100., height: 900., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(ring_card_1) },
    Screen { slug: "rail-card-antigravity-as-of", width: 1100., height: 900., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(ring_card) },
    Screen { slug: "settings-row-archived", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(row_archived) },
    // Settings › MCP Servers (`kuvo/settings/mcp`), the first flow section: its servers with how
    // each stands, Add Server's form as it's typed, a sign-in the box leads, and what's wrong said.
    Screen { slug: "settings-skills", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(skills_list) },
    Screen { slug: "settings-skills-add", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(skills_add) },
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
    Screen { slug: "settings-no-whose-line", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(clock) },
    Screen { slug: "settings-a-accounts", width: 1280., height: 1400., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(accounts) },
    Screen { slug: "accounts-update-available", width: 1280., height: 1400., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(accounts) },
    Screen { slug: "accounts-updating", width: 1280., height: 1400., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(accounts) },
    // The coding agents not running (PROTOCOL.md, "The coding agents' state"): Accounts, Usage,
    // the rail's rings (their card open) and the line over a conversation's composer.
    Screen { slug: "agents-accounts-down", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(accounts) },
    Screen { slug: "agents-accounts-down-narrow", width: 760., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(accounts) },
    Screen { slug: "agents-accounts-restarting", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(accounts) },
    Screen { slug: "agents-usage-down", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(usage) },
    Screen { slug: "agents-usage-restarting", width: 1280., height: 1000., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(usage) },
    Screen { slug: "agents-rail-down", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(ring_card) },
    Screen { slug: "agents-rail-restarting", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    // One standard for warnings and errors (#339): each placement in the real views.
    Screen { slug: "warn-failed-send", width: 1280., height: 820., session: Some("ses_recipes"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(warn_failed_send) },
    Screen { slug: "warn-stopped-turn", width: 1280., height: 820., session: Some("ses_dark"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "warn-account-ended", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(accounts) },
    Screen { slug: "warn-bot-away", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_none) },
    Screen { slug: "bot-move-failed-notifications", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_move_notifications) },
    Screen { slug: "bot-move-done-notifications", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_move_notifications) },
    Screen { slug: "agents-conversation-down", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(conversation_agents_down) },
    Screen { slug: "insights-default", width: 1280., height: 960., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "insights-expanded", width: 1280., height: 960., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "insights-providers", width: 1280., height: 960., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "insights-narrowed", width: 1280., height: 960., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "insights-refreshing", width: 1280., height: 960., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "insights-empty", width: 1280., height: 960., session: None, typed: None, empty: true, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    // Before the box's first figures: the title and one quiet line.
    Screen { slug: "insights-loading", width: 1280., height: 960., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "library-grid", width: 1240., height: 860., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "library-list", width: 1240., height: 860., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "library-bot", width: 1240., height: 860., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "library-images", width: 1240., height: 860., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "library-menu", width: 1240., height: 860., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "library-empty", width: 1240., height: 860., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "market-apps", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "market-category", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "market-connect", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "market-connect-oauth", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "market-custom", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "market-agents", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "market-add-agent", width: 1280., height: 980., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "market-skills", width: 1280., height: 980., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "market-yours", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "market-preview", width: 1280., height: 980., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "market-new-skill", width: 1280., height: 980., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "market-loading", width: 1280., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "market-narrow", width: 820., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "settings-a-usage", width: 1280., height: 1400., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(usage) },
    Screen { slug: "usage-many", width: 1140., height: 900., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(usage) },
    Screen { slug: "usage-one", width: 920., height: 600., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(usage) },
    Screen { slug: "usage-empty", width: 920., height: 600., session: None, typed: None, empty: true, opens: &[], search: None, walks: 0, attach: &[], panel: Some(usage) },
    Screen { slug: "usage-failed", width: 920., height: 600., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(usage) },
    Screen { slug: "usage-stale", width: 920., height: 600., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(usage) },
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
    Screen { slug: "sign-in-code", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sign_in_code) },
    Screen { slug: "sign-in-by-itself", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(sign_in_by_itself) },
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
    // The conversation header, its activity popover, and the composer’s message queue.
    Screen { slug: "conversation-activity-popover", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(conversation_activity) },
    // A long command its agent waits on in the foreground, listed under Processes while it runs.
    Screen { slug: "activity-foreground-command", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(conversation_activity) },
    Screen { slug: "conversation-activity-empty", width: 1280., height: 820., session: Some("ses_recipes"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(conversation_activity) },
    // Activity with the conversation's context (#963): open, pinned beside the chat, busy, quiet.
    Screen { slug: "activity-context-unpinned", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(conversation_activity) },
    Screen { slug: "activity-pinned", width: 1280., height: 820., session: Some("ses_signup"), typed: Some("Let's also check how the warning reads in dark mode near 400k."), empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(activity_pinned) },
    // Small fixes B: Add Project at Projects (the heading hovered), the search without its filter
    // button, and the pinned Activity card's Summary, Full and after Unpin.
    Screen { slug: "small-1123-b-1-add-project", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "small-1123-b-2-search", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "small-1123-b-4-pinned-summary", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(activity_pinned) },
    Screen { slug: "small-1123-b-4-pinned-full", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(activity_pinned_full) },
    Screen { slug: "small-1123-b-4-unpinned", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(activity_unpinned) },
    Screen { slug: "activity-pinned-busy", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(activity_pinned) },
    Screen { slug: "activity-context-quiet", width: 1280., height: 820., session: Some("ses_recipes"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(conversation_activity) },
    // The summarize point picked in Activity (2026-11-23, option B): early, its menu open, near
    // it, and before anything's measured.
    Screen { slug: "activity-summarize-early", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(conversation_activity) },
    Screen { slug: "activity-summarize-picking", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(conversation_activity_picking) },
    Screen { slug: "activity-summarize-near", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(conversation_activity) },
    Screen { slug: "activity-summarize-unmeasured", width: 1280., height: 820., session: Some("ses_recipes"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(conversation_activity) },
    Screen { slug: "conversation-header-running", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "floating-composer-end", width: 1280., height: 820., session: Some("ses_lighthouse"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    // The same in a long group chat and a long side chat: the last message clear of the composer at
    // the end; read up, what's above goes on under it.
    Screen { slug: "floating-composer-group-end", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(group_chat) },
    Screen { slug: "floating-composer-group-scrolled", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(group_chat) },
    Screen { slug: "floating-composer-side-end", width: 1340., height: 840., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(side_chat_long) },
    Screen { slug: "floating-composer-side-scrolled", width: 1340., height: 840., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(side_chat_long) },
    Screen { slug: "floating-composer-scrolled", width: 1280., height: 820., session: Some("ses_lighthouse"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "floating-composer-grown", width: 1280., height: 820., session: Some("ses_lighthouse"), typed: Some("Here are the two screens: the header still wraps on the narrow one, and the footer’s links run into each other."), empty: false, opens: &[], search: None, walks: 0, attach: &["shots/theme-dark.png", "kuvo_ui/src/theme.rs"], panel: None },
    Screen { slug: "composer-queue-two", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "composer-queue-dragging", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "composer-queue-long", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "bars-one", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "bars-many", width: 1280., height: 900., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "bars-folded", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(conversation_activity) },
    Screen { slug: "bars-output", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(activity_output) },
    Screen { slug: "bars-bot", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_chat) },
    Screen { slug: "bars-narrow", width: 760., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    // The mode menu and Full Access the first time; a new session's Local or Worktree.
    Screen { slug: "mode-chip-held", width: 1280., height: 820., session: Some("ses_recipes"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: None },
    Screen { slug: "mode-menu", width: 1280., height: 820., session: Some("ses_recipes"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(mode_menu) },
    Screen { slug: "mode-full-access", width: 1280., height: 820., session: Some("ses_recipes"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(full_access_question) },
    Screen { slug: "landing-worktree", width: 1280., height: 820., session: None, typed: Some("Make the footer links work on mobile"), empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(landing_worktree) },
    Screen { slug: "landing-worktree-menu", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(landing_worktree) },
    // Bot sidebar status marks (trailing column & compact under-face gutter) and the Outputs popover media strip.
    Screen { slug: "bots-sidebar-states", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bots) },
    Screen { slug: "bots-sessions-unreachable", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bots) },
    // A bot on studio-mini, off for three hours: when it was last seen.
    Screen { slug: "bots-computer-away", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bots) },
    // A bot on another computer: its conversation while that computer is off (its last transcript
    // with one quiet line, the composer still there), and once it answers again.
    Screen { slug: "bot-chat-computer-away", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(remote_bot_chat) },
    Screen { slug: "bot-chat-computer-back", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(remote_bot_chat) },
    Screen { slug: "bots-compact-states", width: 1280., height: 820., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bots) },
    Screen { slug: "outputs-popover-media", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(conversation_activity) },
    Screen { slug: "outputs-popover-images", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(conversation_activity_images) },
    Screen { slug: "outputs-popover-bot", width: 1280., height: 860., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_activity) },
    // A long schedule in the same popover: a few of each, then Show All (it scrolls as one).
    Screen { slug: "outputs-popover-bot-long", width: 1280., height: 860., session: None, typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(bot_activity) },
    // The Outputs popover (#932): every section, some folded, loading, and the side chats.
    Screen { slug: "outputs-popover-full", width: 1280., height: 900., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(conversation_activity) },
    Screen { slug: "outputs-popover-collapsed", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(conversation_activity_folded) },
    Screen { slug: "outputs-popover-loading", width: 1280., height: 820., session: Some("ses_signup"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(conversation_activity_loading) },
    Screen { slug: "stage-viewer-single", width: 1280., height: 820., session: Some("ses_dark"), typed: None, empty: false, opens: &[], search: None, walks: 0, attach: &[], panel: Some(open_single_picture) },
];

/// An area's row, picked: what its page shows.
fn pick(workspace: &mut Workspace, area: Area, key: &str, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_area(area, window, cx);
    if let Some(view) = workspace.area_view(area).cloned() {
        view.update(cx, |view, cx| view.select(Some(key.to_owned()), cx));
    }
}

/// Bots with the sidebar folded: the group and the divider stay together, then the title.
fn bots_folded(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Bots, "bot:bot_mira", window, cx);
    workspace.toggle_sidebar(window, cx);
}

/// A session with the sidebar folded.
fn sidebar_folded(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.toggle_sidebar(window, cx);
}

fn bots(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Bots, "bot:bot_mira", window, cx);
}

fn bot_move_menu(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    bots(workspace, window, cx);
    kuvo::moves::MovesView::open_settings_menu(workspace.moves(), MoveTarget::new(MoveKind::Bot, "bot_mira"), workspace.box_name().to_string(), window, cx);
}

fn bot_move_confirm(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    bots(workspace, window, cx);
    let target = MoveTarget::new(MoveKind::Bot, "bot_mira");
    let place = workspace.client().move_places(&target).expect("move places").places.remove(0);
    workspace.moves().update(cx, |view, cx| view.ask(target, place, None, window, cx));
}

/// A move's result, then the title bar's notifications open, where it's said (#339).
fn bot_move_notifications(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    bot_move_result(workspace, window, cx);
    workspace.toggle_notifications(cx);
}

/// One of yours that couldn't go (the stand-in refuses sends), with its notice under it.
fn warn_failed_send(workspace: &mut Workspace, _window: &mut Window, cx: &mut Context<Workspace>) {
    let composer = workspace.conversation().expect("the conversation").read(cx).composer().clone();
    let field = composer.read(cx).field().clone();
    field.update(cx, |field, cx| field.set_text("Also cap the backoff at 30 seconds and log the attempt count.", cx));
    composer.update(cx, |composer, cx| composer.send(cx));
}

fn bot_move_result(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    bot_move_confirm(workspace, window, cx);
    workspace.moves().update(cx, |view, cx| view.submit(window, cx));
}

fn sidebar_search_results(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    bots(workspace, window, cx);
    workspace.open_quick_search(window, cx).expect("compact search").update(cx, |field, cx| field.set_text("in", cx));
}

fn sidebar_search_empty(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    bots(workspace, window, cx);
    workspace.open_quick_search(window, cx).expect("compact search").update(cx, |field, cx| field.set_text("zzzz", cx));
}

/// Bots' page view.
fn bots_page(workspace: &Workspace, cx: &gpui::App) -> gpui::Entity<BotsView> {
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
fn show_more_once(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    let website = format!("{}/Projects/website", kuvo::stand_in::HOME);
    workspace.show_more(&website, cx);
}

fn conversation_activity(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let activity = workspace.conversation().expect("conversation").read(cx).bars().clone();
    activity.update(cx, |activity, cx| activity.open_activity(window, cx));
}

/// Its Activity pinned at the top right of the conversation.
fn activity_pinned(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let activity = workspace.conversation().expect("conversation").read(cx).bars().clone();
    activity.update(cx, |activity, cx| activity.pin_now(window, cx));
}

/// Pinned, in Full.
fn activity_pinned_full(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let activity = workspace.conversation().expect("conversation").read(cx).bars().clone();
    activity.update(cx, |activity, cx| {
        activity.pin_now(window, cx);
        activity.show_full_now(true, cx);
    });
}

/// Pinned, then unpinned: the card gone, nothing in its place.
fn activity_unpinned(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let activity = workspace.conversation().expect("conversation").read(cx).bars().clone();
    activity.update(cx, |activity, cx| {
        activity.pin_now(window, cx);
        activity.unpin_now(cx);
    });
}

fn conversation_activity_picking(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let activity = workspace.conversation().expect("conversation").read(cx).bars().clone();
    activity.update(cx, |activity, cx| activity.open_activity_picking_summarize(window, cx));
}

fn conversation_activity_folded(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let activity = workspace.conversation().expect("conversation").read(cx).bars().clone();
    activity.update(cx, |activity, cx| {
        activity.fold_section("outputs", cx);
        activity.fold_section("processes", cx);
        activity.open_activity(window, cx);
    });
}

fn conversation_activity_loading(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let activity = workspace.conversation().expect("conversation").read(cx).bars().clone();
    activity.update(cx, |activity, cx| activity.open_activity_loading(window, cx));
}

fn conversation_activity_images(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let activity = workspace.conversation().expect("conversation").read(cx).bars().clone();
    activity.update(cx, |activity, cx| {
        activity.open_activity_outputs(kuvo::work_bars::OutputKind::Images, window, cx);
    });
}

fn bot_activity(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    bot_chat(workspace, window, cx);
    let activity = workspace.bot_chat().expect("bot chat").1.read(cx).bars().clone();
    activity.update(cx, |activity, cx| activity.open_activity(window, cx));
}

fn activity_output(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    conversation_activity(workspace, window, cx);
    let activity = workspace.conversation().expect("conversation").read(cx).bars().clone();
    let command = activity.read(cx).background().commands.iter().find(|command| command.id == "sh_dev").cloned();
    if let Some(command) = command {
        workspace.arrive(kuvo::panel::Arrival::Process { session: "ses_signup".into(), command }, cx);
    }
}
/// Full Access picked the first time on this computer: asked in the composer.
fn full_access_question(workspace: &mut Workspace, _window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(view) = workspace.conversation() {
        view.read(cx).composer().clone().update(cx, |composer, cx| composer.pick_approval(kuvo_box::approval::Approval::FullAccess, cx));
    }
}
/// A new session in the website project, set to start in a worktree of its own.
/// The landing's sky at a fixed hour, so each time of day can be seen (every other screen shows noon).
fn sky_dawn(workspace: &mut Workspace, _window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.set_sky_hour(6.5, cx);
}

fn sky_day(workspace: &mut Workspace, _window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.set_sky_hour(12., cx);
}

fn sky_dusk(workspace: &mut Workspace, _window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.set_sky_hour(19., cx);
}

fn sky_night(workspace: &mut Workspace, _window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.set_sky_hour(23., cx);
}

fn landing_worktree(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.new_session(None, window, cx);
    workspace.pick_place(kuvo_box::models::StartIn::Project("/Users/sam/Projects/website".into()), cx);
    workspace.landing().clone().update(cx, |composer, cx| {
        composer.read_at_once(cx);
        composer.set_worktree(true, cx);
    });
}
fn remote_bot_chat(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_area(Area::Bots, window, cx);
    workspace.open_bot_chat("bot_release", "ses_releases", None, window, cx);
}
fn bot_chat(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_area(Area::Bots, window, cx);
    workspace.open_bot_chat("bot_mira", "ses_mira", None, window, cx);
}
/// New Bot: ready-made bots are in the Marketplace.
fn bot_chat_elsewhere(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_area(Area::Bots, window, cx);
    workspace.open_bot_chat("bot_wren", "ses_wren", None, window, cx);
}

fn bot_new(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Bots, "new", window, cx);
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
    let page = bots_page(workspace, cx);
    let (_, details) = page.read(cx).job_fields();
    window.focus(&details.focus_handle(cx), cx);
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
fn queue_loading(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_all(workspace, window, cx);
    queue_page(workspace, cx).update(cx, |page, cx| page.show_loading_for_shot(cx));
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
        page.act(QueueAct::AddFilter(Filter::Owner { owner: "ses_wren".into(), name: "Wren".into() }), cx);
        page.act(QueueAct::AddFilter(Filter::Owner { owner: "ses_scout".into(), name: "Scout".into() }), cx);
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
fn queue_new_empty(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_all(workspace, window, cx);
    queue_page(workspace, cx).update(cx, |page, cx| page.act(QueueAct::New, cx));
}

fn queue_new_enlarged(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_new_empty(workspace, window, cx);
    queue_page(workspace, cx).update(cx, |page, cx| page.act(QueueAct::ToggleEditorSize, cx));
}

fn queue_new_picture(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_new_empty(workspace, window, cx);
    let page = queue_page(workspace, cx);
    let (title, body, composer) = page.read_with(cx, |page, _| {
        (page.item_fields().0.clone(), page.item_fields().1.clone(), page.item_composer().clone())
    });
    title.update(cx, |field, cx| field.set_text("Tighten the Work Queue layout", cx));
    body.update(cx, |field, cx| field.set_text("The current Work Queue leaves a large gap between groups.", cx));
    let image = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../shots/sidebar-c-queue-light.png");
    composer.update(cx, |composer, cx| composer.attach_named([image], cx));
    body.update(cx, |field, cx| field.replace_selection("Keep the group names and counts visible.", cx));
}

fn queue_collapsed_lanes(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    queue_all(workspace, window, cx);
    queue_page(workspace, cx).update(cx, |page, cx| {
        for status in ["needs-you", "in-review", "in-progress", "waiting", "todo", "backlog"] {
            page.act(QueueAct::Fold(status.into()), cx);
        }
    });
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
/// Computers' page, as the window shows it.
fn computers_page(workspace: &Workspace, cx: &mut Context<Workspace>) -> Option<gpui::Entity<kuvo::computers::ComputersView>> {
    workspace.area_view(Area::Computers).and_then(|view| view.read(cx).computers_page().cloned())
}
fn computer_rename(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    computers(workspace, window, cx);
    if let Some(page) = computers_page(workspace, cx) {
        page.update(cx, |page, cx| page.show_rename("key_devbox", cx));
    }
}
fn computers_handover(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    computers_all(workspace, window, cx);
    if let Some(page) = computers_page(workspace, cx) {
        page.update(cx, |page, cx| page.show_lead(None, cx));
    }
}
fn computer_remove(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    computers(workspace, window, cx);
    if let Some(page) = computers_page(workspace, cx) {
        page.update(cx, |page, cx| page.show_removal("key_devbox", false, cx));
    }
}
fn computer_remove_away(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    pick(workspace, Area::Computers, "computer:key_more_2", window, cx);
    if let Some(page) = computers_page(workspace, cx) {
        page.update(cx, |page, cx| page.show_removal("key_more_2", true, cx));
    }
}
fn computer_remove_last(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    computer_this(workspace, window, cx);
    if let Some(page) = computers_page(workspace, cx) {
        page.update(cx, |page, cx| page.show_removal("key_stand_in", true, cx));
    }
}
/// devbox removed and wiped: what the box said, over all your computers.
fn computers_removed(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    computer_remove(workspace, window, cx);
    if let Some(page) = computers_page(workspace, cx) {
        page.update(cx, |page, cx| page.go_now(true, cx));
    }
    workspace.read_at_once(cx);
}
fn computer_narrow(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    computer_away(workspace, window, cx);
    workspace.rail_pressed(Area::Computers, window, cx);
}

fn usage(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_setting(&kuvo::settings::box_key("", "usage"), window, cx);
}

/// Settings' view, on `section`.
fn settings(workspace: &mut Workspace, section: &str, window: &mut Window, cx: &mut Context<Workspace>) -> gpui::Entity<SettingsView> {
    workspace.show_setting(section, window, cx);
    // (Your own in Settings; a computer's under Computers › that computer.)
    workspace.area_view(workspace.area()).and_then(|view| view.read(cx).settings().cloned()).expect("Settings")
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
/// Runs on's menu open, in a new session's composer.
fn runs_on_open(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let landing = w.landing().clone();
    landing.update(cx, |composer, cx| composer.open_runs_on(window, cx));
}
/// devbox's settings, under Computers › devbox: its Permissions.
fn computer_settings(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    settings(w, &kuvo::settings::box_key("key_devbox", "permissions"), window, cx);
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
/// The coding agents down, as the box says over a conversation it couldn't read again.
fn conversation_agents_down(w: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    let down = br#"{"message":"The coding agents keep stopping. Kuvo tries again in about 5 minutes.","reason":"agentsDown","action":{"label":"Try Again","route":"kuvo/agents/start"}}"#;
    let view = w.conversation().expect("the conversation").clone();
    view.update(cx, |view, cx| view.take(kuvo_box::ConversationUpdate::Failed(kuvo_box::Error::Refused(kuvo_box::Refusal::from_answer(503, down))), cx));
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
/// Claude Code's, which ends on a code to paste back: the field there at once, focused.
fn sign_in_code(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    signing_in(w, Side::Agents, "claude-code", "Claude Code", window, cx);
}
/// Codex's in the browser, which comes back by itself: only a quiet wait.
fn sign_in_by_itself(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let sheet = signing_in(w, Side::Agents, "codex-cli", "Codex", window, cx);
    sheet.update(cx, |sheet, cx| {
        let way = sheet.ways().methods[0].clone();
        sheet.pick(way, cx);
    });
}
/// Claude Code's, which ends on a code to paste back: a code turned down.
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
        sheet.ask_now(cx);
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

/// The side panel on its New tab page.
fn panel_new_tab(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.toggle_panel(cx);
}

/// Two files open, the sign-up form showing, the folder's files beside it.
fn panel_file(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let root = kuvo::stand_in::git::website();
    workspace.preview_file(&format!("{root}/src/lib/validate.ts"), window, cx);
    workspace.preview_file(&format!("{root}/src/components/SignupForm.tsx"), window, cx);
    if let Some(tree) = workspace.panel().read(cx).files() {
        tree.update(cx, |tree, cx| {
            for folder in ["src", "src/components"] {
                tree.toggle(&format!("{root}/{folder}"), cx);
            }
        });
    }
}

/// Another computer's browser, live (its page's picture comes after it opens).
fn panel_browser(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_panel(Kind::Browser(0), cx);
}

/// A terminal in the session's folder (what its shell printed comes after it opens).
fn panel_terminal(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_panel(Kind::Terminal(0), cx);
}

/// A dev server's output as it prints.
fn panel_process(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    let command = serde_json::from_value(serde_json::json!({ "id": "sh_dev", "command": "bun run dev", "started": NOW - 95_000.0, "folder": kuvo::stand_in::git::website() })).expect("a command");
    workspace.arrive(kuvo::panel::Arrival::Process { session: "ses_signup".into(), command }, cx);
}

fn panel_subagents(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.arrive(kuvo::panel::Arrival::Subagents(None), cx);
}

/// One subagent open, its conversation in the tab.
fn panel_subagent_detail(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.arrive(kuvo::panel::Arrival::Subagents(Some("ses_limits".into())), cx);
}

/// A queued message taken back to edit (as its box gives it back): its words, and its picture
/// (sent with its bytes) and file back in the strip.
fn composer_edited(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    use base64::Engine as _;
    let composer = workspace.conversation().expect("the conversation").read(cx).composer().clone();
    // (Its picture kept, as Edit keeps one, in a folder of the shots' own.)
    cx.set_global(kuvo::pasted::PastedFolder(std::env::temp_dir().join("kuvo-shots-pasted")));
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_fn(96, 64, |x, y| if y < 20 { image::Rgba([220, 60, 60, 255]) } else if x % 24 < 12 { image::Rgba([240, 240, 245, 255]) } else { image::Rgba([200, 205, 215, 255]) }).write_to(&mut png, image::ImageFormat::Png).expect("a picture");
    let picture = kuvo_box::parts::File { uri: format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png.into_inner())), name: Some("Pasted Image 2026-11-23 at 10.12.04.png".into()) };
    let file = kuvo_box::parts::File::at(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/stand_in/fixtures/SignupForm.tsx"));
    composer.update(cx, |composer, cx| composer.restore("[Image 1] The error still shows under the password field: move it under the email one.", &[picture, file], cx));
}

/// A side chat with two questions answered.
fn panel_side_chat(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_panel(Kind::SideChat(None), cx);
    let Some(view) = workspace.panel().read(cx).side_chat() else { return };
    for question in ["Why check 23505 instead of querying first?", "Should 409 share the rate-limit bucket?"] {
        let field = view.read(cx).field().clone();
        field.update(cx, |field, cx| field.set_text(question, cx));
        view.update(cx, |view, cx| view.ask(cx));
    }
    // The next one's composer: a file attached, on another model than the conversation's.
    view.update(cx, |view, cx| {
        view.attach([std::path::PathBuf::from(format!("{}/notes/signup-review.md", kuvo::stand_in::git::website()))], cx);
        view.pick(kuvo_box::typed::ModelRef { provider_id: "anthropic".into(), id: "claude-sonnet-5-5".into(), variant: Some("medium".into()) }, cx);
    });
}

/// A long side chat: eight questions, each answered at some length.
fn side_chat_long(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_panel(Kind::SideChat(None), cx);
    let Some(view) = workspace.panel().read(cx).side_chat() else { return };
    for (ix, question) in SIDE_QUESTIONS.iter().enumerate() {
        let field = view.read(cx).field().clone();
        field.update(cx, |field, cx| field.set_text(&format!("{question} ({})", ix + 1), cx));
        view.update(cx, |view, cx| view.ask(cx));
    }
}

const SIDE_QUESTIONS: [&str; 8] = [
    "Why check 23505 instead of querying first?",
    "Should 409 share the rate-limit bucket?",
    "Is the email lowercased before the insert?",
    "What does the form show while it waits?",
    "Does the test cover a taken email?",
    "Where is the error message worded?",
    "Would a retry send the email twice?",
    "What’s left before this can merge?",
];

/// A bot's conversation with the side panel opened from the title bar: the New tab page beside it.
fn panel_bot_chat(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    bot_chat(workspace, window, cx);
    workspace.toggle_panel(cx);
}

/// A side chat beside a bot's conversation, with its question answered.
fn panel_bot_side_chat(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    bot_chat(workspace, window, cx);
    workspace.show_panel(Kind::SideChat(None), cx);
    let Some(view) = workspace.panel().read(cx).side_chat() else { return };
    let field = view.read(cx).field().clone();
    field.update(cx, |field, cx| field.set_text("What do you check first each morning?", cx));
    view.update(cx, |view, cx| view.ask(cx));
}

/// Many tabs: Changes, two files, a process, the subagents; a file showing.
fn panel_many_tabs(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_panel(Kind::Changes, cx);
    panel_process(workspace, window, cx);
    workspace.show_panel(Kind::Subagents, cx);
    let root = kuvo::stand_in::git::website();
    workspace.preview_file(&format!("{root}/src/lib/validate.ts"), window, cx);
    workspace.preview_file(&format!("{root}/src/components/SignupForm.tsx"), window, cx);
}

fn changes(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_panel(Kind::Changes, cx);
}

/// The sign-up form's staged changes, opened.
fn diff(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_panel(Kind::Changes, cx);
    let Some(view) = workspace.panel().read(cx).changes() else { return };
    view.update(cx, |view, cx| {
        let file = view.files().ready().and_then(|files| files.files.iter().find(|file| file.group == Group::Staged).cloned());
        if let Some(file) = file {
            view.open(file, cx);
        }
    });
}

/// Discarding the validation's changes, asked first.
fn discard(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_panel(Kind::Changes, cx);
    let Some(view) = workspace.panel().read(cx).changes() else { return };
    view.update(cx, |view, cx| {
        let file = view.files().ready().and_then(|files| files.files.iter().find(|file| file.path.ends_with("validate.ts")).cloned());
        if let Some(file) = file {
            view.confirm_discard(file, cx);
        }
    });
}

/// The website's folders, a few open, one that can't be read.
fn tree(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_panel(Kind::Files, cx);
    let Some(view) = workspace.panel().read(cx).files() else { return };
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
    let path = format!("{}/src/components/SignupForm.tsx", kuvo::stand_in::git::website());
    workspace.preview_file(&path, window, cx);
}

fn branches(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.show_panel(Kind::Branches, cx);
}

/// Create Pull Request, the box's draft in it.
fn pull_request(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    branches(workspace, window, cx);
    let Some(view) = workspace.panel().read(cx).branches() else { return };
    view.update(cx, |view, cx| view.write_pull_request(window, cx));
}

/// Selects in the open conversation, from the first `from` to the end of `to` after it.
fn select(workspace: &mut Workspace, from: &str, to: &str, window: &mut Window, cx: &mut Context<Workspace>) {
    let view = workspace.conversation().expect("a conversation").clone();
    let found = view.update(cx, |view, cx| view.select_text(from, to, window, cx));
    assert!(found, "{from:?} … {to:?} isn't in the conversation");
}

/// A picture as a PNG: a soft wash from one colour to another, so its shape reads in a shot.
fn wash(width: u32, height: u32, from: [u8; 3], to: [u8; 3]) -> Vec<u8> {
    let picture = image::RgbImage::from_fn(width, height, |x, y| {
        let t = (x as f32 / width as f32 + y as f32 / height as f32) / 2.;
        image::Rgb(std::array::from_fn(|n| (from[n] as f32 + (to[n] as f32 - from[n] as f32) * t) as u8))
    });
    let mut out = std::io::Cursor::new(Vec::new());
    picture.write_to(&mut out, image::ImageFormat::Png).expect("a PNG");
    out.into_inner()
}

/// Messages of yours with pictures (one, three, a tall one, a wide one), and the agent's reply
/// showing one, at the end of the recipes chat; each a file on its computer.
fn stage_pictures(stand_in: &StandIn) {
    let file = |name: &str, bytes: Vec<u8>| {
        let path = format!("/Users/sam/Pictures/{name}");
        stand_in.put_file(&path, bytes);
        serde_json::json!({ "mime": "image/png", "name": name, "url": format!("file://{path}") })
    };
    let yours = |id: &str, ago: f64, text: &str, files: Vec<serde_json::Value>| serde_json::json!({ "type": "user", "id": id, "time": { "created": NOW - ago }, "text": text, "files": files });
    stand_in.add_message("ses_recipes", yours("msg_pic1", 400_000.0, "What's on this shelf?", vec![file("shelf.png", wash(1600, 1000, [214, 180, 140], [120, 84, 60]))]));
    let three = vec![
        file("fridge.png", wash(1200, 900, [180, 214, 230], [70, 110, 150])),
        file("pantry.png", wash(900, 1200, [230, 200, 160], [150, 100, 60])),
        file("spices.png", wash(1000, 1000, [200, 120, 90], [110, 50, 40])),
    ];
    stand_in.add_message("ses_recipes", yours("msg_pic2", 300_000.0, "And these three?", three));
    stand_in.add_message("ses_recipes", yours("msg_pic3", 200_000.0, "The whole recipe card", vec![file("card.png", wash(600, 2400, [240, 236, 220], [190, 180, 150]))]));
    stand_in.add_message("ses_recipes", yours("msg_pic4", 100_000.0, "And the counter, end to end", vec![file("counter.png", wash(3000, 600, [160, 170, 150], [80, 90, 70]))]));
    stand_in.put_file("/Users/sam/Pictures/stew.png", wash(1400, 900, [250, 210, 150], [200, 120, 70]));
    stand_in.add_message("ses_recipes", serde_json::json!({ "type": "assistant", "id": "msg_pic5", "time": { "created": NOW - 90_000.0, "completed": NOW - 80_000.0 }, "agent": "build", "finish": "stop", "content": [
        { "type": "text", "text": "Here's what you could make tonight:" },
        { "type": "tool", "id": "prt_pic", "name": "kuvo-browser_show_file", "title": "Showed a file", "doing": "Showing a file", "state": { "status": "completed", "input": { "path": "/Users/sam/Pictures/stew.png", "caption": "A lentil stew, with what's on the shelf" }, "content": [] } },
    ] }));
}

/// Real conversation turns for `show-file-pictures`: one single picture with a caption, and three
/// pictures in one turn forming the 3-across strip, with no redundant "Showed a file" step lines.
fn stage_show_file_pictures(stand_in: &StandIn) {
    use serde_json::json;
    stand_in.set_facts("ses_dark", |facts| {
        facts["title"] = json!("Pricing & checkout visual check");
    });
    stand_in.put_file("/Users/sam/Projects/website/shots/pricing-hero.png", encode_png(pricing_hero_image()));
    stand_in.put_file("/Users/sam/Projects/website/shots/checkout-light.png", encode_png(checkout_desktop_image(false)));
    stand_in.put_file("/Users/sam/Projects/website/shots/checkout-dark.png", encode_png(checkout_desktop_image(true)));
    stand_in.put_file("/Users/sam/Projects/website/shots/checkout-narrow.png", encode_png(checkout_mobile_image()));

    stand_in.add_message("ses_dark", json!({
        "type": "user", "id": "msg_sf_u1", "time": { "created": NOW - 240_000.0 },
        "text": "Can you show me how the new pricing page hero looks?"
    }));
    stand_in.add_message("ses_dark", json!({
        "type": "assistant", "id": "msg_sf_a1", "time": { "created": NOW - 230_000.0, "completed": NOW - 224_000.0 },
        "agent": "build", "finish": "stop",
        "content": [
            { "type": "text", "text": "Here’s the updated hero from the latest build:" },
            { "type": "tool", "id": "prt_sf_1", "name": "kuvo-browser_show_file", "title": "Showed a file", "doing": "Showing a file",
              "state": { "status": "completed", "input": { "path": "/Users/sam/Projects/website/shots/pricing-hero.png", "caption": "Pricing hero at 1440 px — tighter headline spacing and the annual toggle." }, "content": [] } }
        ]
    }));
    stand_in.add_message("ses_dark", json!({
        "type": "user", "id": "msg_sf_u2", "time": { "created": NOW - 160_000.0 },
        "text": "Compare light, dark, and the narrow checkout state together."
    }));
    stand_in.add_message("ses_dark", json!({
        "type": "assistant", "id": "msg_sf_a2", "time": { "created": NOW - 150_000.0, "completed": NOW - 142_000.0 },
        "agent": "build", "finish": "stop",
        "content": [
            { "type": "text", "text": "All three states side by side:" },
            { "type": "tool", "id": "prt_sf_2", "name": "kuvo-browser_show_file", "title": "Showed a file", "doing": "Showing a file",
              "state": { "status": "completed", "input": { "path": "/Users/sam/Projects/website/shots/checkout-light.png", "caption": "Light mode" }, "content": [] } },
            { "type": "tool", "id": "prt_sf_3", "name": "kuvo-browser_show_file", "title": "Showed a file", "doing": "Showing a file",
              "state": { "status": "completed", "input": { "path": "/Users/sam/Projects/website/shots/checkout-dark.png", "caption": "Dark mode" }, "content": [] } },
            { "type": "tool", "id": "prt_sf_4", "name": "kuvo-browser_show_file", "title": "Showed a file", "doing": "Showing a file",
              "state": { "status": "completed", "input": { "path": "/Users/sam/Projects/website/shots/checkout-narrow.png", "caption": "Narrow (390 px)" }, "content": [] } }
        ]
    }));
}

/// Real conversation turns for `show-file-cards`: PDF + Markdown cards, missing file tombstones
/// (with no retry), and a still-running `kuvo-browser_show_file` call keeping its quiet step line.
fn stage_show_file_cards(stand_in: &StandIn) {
    use serde_json::json;
    stand_in.set_facts("ses_dark", |facts| {
        facts["title"] = json!("Release artifacts & notes");
    });
    let mut pdf = b"%PDF-1.7\n".to_vec();
    pdf.resize(2_516_582, b' '); // 2.4 MB
    stand_in.put_file("/Users/sam/Projects/website/docs/release/Q4-launch-report.pdf", pdf);
    let mut md = b"# Schema migration plan\n\n1. Backfill new columns\n2. Switch reads\n".to_vec();
    md.resize(18_432, b' '); // 18 KB
    stand_in.put_file("/Users/sam/Projects/website/docs/release/migration-plan.md", md);
    // Intentionally omit `old-wireframe.png` and `draft-notes.pdf` so `file_bytes` / `file_info`
    // return real `NotFound` and render the calm dashed "Missing" rows without retry.

    stand_in.add_message("ses_dark", json!({
        "type": "user", "id": "msg_sfc_u1", "time": { "created": NOW - 240_000.0 },
        "text": "Export the release summary as a PDF and show me the migration notes too."
    }));
    stand_in.add_message("ses_dark", json!({
        "type": "assistant", "id": "msg_sfc_a1", "time": { "created": NOW - 230_000.0, "completed": NOW - 222_000.0 },
        "agent": "build", "finish": "stop",
        "content": [
            { "type": "text", "text": "Both files are ready in `docs/release`:" },
            { "type": "tool", "id": "prt_sfc_1", "name": "kuvo-browser_show_file", "title": "Showed a file", "doing": "Showing a file",
              "state": { "status": "completed", "input": { "path": "/Users/sam/Projects/website/docs/release/Q4-launch-report.pdf", "caption": "Executive summary and rollout timeline for Thursday." }, "content": [] } },
            { "type": "tool", "id": "prt_sfc_2", "name": "kuvo-browser_show_file", "title": "Showed a file", "doing": "Showing a file",
              "state": { "status": "completed", "input": { "path": "/Users/sam/Projects/website/docs/release/migration-plan.md", "caption": "Step-by-step schema migration checklist." }, "content": [] } }
        ]
    }));
    stand_in.add_message("ses_dark", json!({
        "type": "user", "id": "msg_sfc_u2", "time": { "created": NOW - 140_000.0 },
        "text": "Pull up the earlier wireframe and draft notes from Tuesday."
    }));
    stand_in.add_message("ses_dark", json!({
        "type": "assistant", "id": "msg_sfc_a2", "time": { "created": NOW - 130_000.0, "completed": NOW - 124_000.0 },
        "agent": "build", "finish": "stop",
        "content": [
            { "type": "text", "text": "Here are the two files from earlier:" },
            { "type": "tool", "id": "prt_sfc_3", "name": "kuvo-browser_show_file", "title": "Showed a file", "doing": "Showing a file",
              "state": { "status": "completed", "input": { "path": "/Users/sam/Projects/website/tmp/old-wireframe.png", "caption": "Initial hero wireframe before the copy pass." }, "content": [] } },
            { "type": "tool", "id": "prt_sfc_4", "name": "kuvo-browser_show_file", "title": "Showed a file", "doing": "Showing a file",
              "state": { "status": "completed", "input": { "path": "/Users/sam/Projects/website/tmp/draft-notes.pdf" }, "content": [] } }
        ]
    }));
    stand_in.add_message("ses_dark", json!({
        "type": "user", "id": "msg_sfc_u3", "time": { "created": NOW - 30_000.0 },
        "text": "And grab the updated mobile nav screenshot too."
    }));
    stand_in.add_message("ses_dark", json!({
        "type": "assistant", "id": "msg_sfc_a3", "time": { "created": NOW - 20_000.0 },
        "agent": "build",
        "content": [
            { "type": "text", "text": "Capturing the mobile nav now:" },
            { "type": "tool", "id": "prt_sfc_5", "name": "kuvo-browser_show_file", "title": "Showed a file", "doing": "Showing a file",
              "state": { "status": "running", "input": { "path": "/Users/sam/Projects/website/shots/mobile-nav.png" } } }
        ]
    }));
}

/// Real conversation turn for `tool-details`: command with output, file edit with diff, sent
/// message, Work Queue update, scheduled wake-up, canceled wake-up, failed call, and an unknown
/// MCP tool fallback — rendered in the real ConversationView.
fn stage_tool_details(stand_in: &StandIn) {
    use serde_json::json;
    stand_in.set_facts("ses_dark", |facts| {
        facts["title"] = json!("Finish shown-files view and update Mira");
    });
    let patch = "\
--- a/gpui/kuvo/src/entries.rs
+++ b/gpui/kuvo/src/entries.rs
@@ -512,7 +512,10 @@ fn folds(items: &[(Rc<Step>, usize)]) -> bool {
 fn is_completed_show_file(content: &Content) -> bool {
     match content {
         Content::Tool { name, state, .. } if name == \"kuvo-browser_show_file\" => {
-            state_status(state) == Some(\"completed\")
+            let done = state_status(state) == Some(\"completed\");
+            let has_path = input_path(state).is_some();
+            done && has_path
         }
         _ => false,
     }
 }";

    stand_in.add_message("ses_dark", json!({
        "type": "user", "id": "msg_td_u1", "time": { "created": NOW - 180_000.0 },
        "text": "Finish wiring the shown-files view, run the unit tests, let Mira know, update #231, remind me at 19:30 to check the release build, cancel the groundwork reminder, close #999, and log the dark mode border issue in Linear."
    }));
    stand_in.add_message("ses_dark", json!({
        "type": "assistant", "id": "msg_td_a1", "time": { "created": NOW - 160_000.0, "completed": NOW - 120_000.0 },
        "agent": "build", "finish": "stop",
        "content": [
            {
                "type": "tool", "id": "prt_td_0", "name": "shell",
                "state": {
                    "status": "completed",
                    "input": { "command": "cargo test -p kuvo shown_files" },
                    "content": [{ "type": "text", "text": "running 7 tests\ntest shown_files::tests::adapt_maps_document_states ... ok\ntest shown_files::tests::adapt_maps_picture_missing_vs_failed ... ok\ntest result: ok. 7 passed; 0 failed; 0 ignored" }]
                }
            },
            {
                "type": "tool", "id": "prt_td_1", "name": "edit",
                "state": {
                    "status": "completed",
                    "input": { "filePath": "/Users/sam/Projects/kuvo/gpui/kuvo/src/entries.rs" },
                    "content": [{ "type": "text", "text": "Edited." }],
                    "metadata": {
                        "files": [{
                            "file": "/Users/sam/Projects/kuvo/gpui/kuvo/src/entries.rs",
                            "patch": patch,
                            "additions": 3,
                            "deletions": 1,
                            "status": "modified"
                        }]
                    }
                }
            },
            {
                "type": "tool", "id": "prt_td_2", "name": "kuvo-browser_send_message",
                "title": "Sent a message", "doing": "Sending a message",
                "state": {
                    "status": "completed",
                    "input": {
                        "to": "&Mira (ses_1ac77df1a001YU3j)",
                        "wait": false,
                        "text": "Groundwork is merged and Flow is wired into ConversationView. All 7 shown-files tests pass."
                    },
                    "content": [{ "type": "text", "text": "Delivered to &Mira (ses_1ac77df1a001YU3j)." }]
                }
            },
            {
                "type": "tool", "id": "prt_td_3", "name": "kuvo-browser_queue_update",
                "title": "Updated a Work Queue item", "doing": "Updating a Work Queue item",
                "state": {
                    "status": "completed",
                    "input": {
                        "item": "231",
                        "status": "in-review",
                        "note": "Flow view wired to kuvo/900c-groundwork; light and dark shots rendered."
                    },
                    "content": [{ "type": "text", "text": "Updated #231 — status set to In review." }]
                }
            },
            {
                "type": "tool", "id": "prt_td_4", "name": "kuvo-browser_schedule_wake_up",
                "title": "Scheduled a wake-up", "doing": "Scheduling a wake-up",
                "state": {
                    "status": "completed",
                    "input": {
                        "when": "today 19:30",
                        "summary": "Check whether the release build passed",
                        "note": "Check CI for kuvo/900-batch-c-drafts and report any failing checks."
                    },
                    "content": [{ "type": "text", "text": "Scheduled wake_8f2a for today at 19:30 (+03:00)." }]
                }
            },
            {
                "type": "tool", "id": "prt_td_5", "name": "kuvo-browser_cancel_wake_up",
                "title": "Canceled a wake-up", "doing": "Canceling a wake-up",
                "state": {
                    "status": "completed",
                    "input": { "id": "wake_4b19" },
                    "content": [{ "type": "text", "text": "Canceled wake_4b19 (Check groundwork branch)." }]
                }
            },
            {
                "type": "tool", "id": "prt_td_6", "name": "kuvo-browser_queue_update",
                "title": "Updated a Work Queue item", "doing": "Updating a Work Queue item",
                "state": {
                    "status": "failed",
                    "input": { "item": "999", "status": "done" },
                    "error": "Error: #999 not found"
                }
            },
            {
                "type": "tool", "id": "prt_td_7", "name": "linear_create_issue",
                "title": "Linear · Create issue", "doing": "Linear · Creating issue",
                "state": {
                    "status": "completed",
                    "input": {
                        "team_id": "team_9f2a8b1c",
                        "draft": false,
                        "title": "Tighten dark mode borders",
                        "description": "Hairline borders on cards blend into the page background in dark appearance."
                    },
                    "content": [{ "type": "text", "text": "Created KUV-412 in Design Backlog." }]
                }
            },
            {
                "type": "text",
                "text": "All seven shown-files tests pass, Mira has the update, #231 is now In review, the 19:30 wake-up is set, the groundwork reminder is canceled, and KUV-412 is logged in Linear. I couldn’t close #999 because that item wasn’t found."
            }
        ]
    }));
}

/// Real conversation for `agent-messages`: a received message from another agent, a sent message
/// in the middle of a turn (outside the step group, not counted in the summary), a sent message
/// with a waited reply (`To` followed immediately by `From`), and a failed send.
fn stage_agent_messages(stand_in: &StandIn) {
    use serde_json::json;
    stand_in.set_facts("ses_dark", |facts| {
        facts["title"] = json!("Coordinate beta.35 release");
    });

    // 1. Received message from another agent (note leading `&` in metadata is stripped).
    stand_in.add_message("ses_dark", json!({
        "type": "user",
        "id": "msg_am_recv",
        "time": { "created": NOW - 240_000.0 },
        "text": "[From &Check the other forms] The profile form resets email when the server rejects it too. Sharing the validation helper should fix both.",
        "metadata": {
            "kuvo": {
                "parts": {
                    "words": "The profile form resets email when the server rejects it too. Sharing the validation helper should fix both.",
                    "mail": { "name": "&Check the other forms", "session": "ses_sub" }
                }
            }
        }
    }));

    // 2. Assistant turn: steps before, sent message mid-turn (leaves the step group), steps after,
    //    sent message with waited reply, failed send, and wrap-up text.
    stand_in.add_message("ses_dark", json!({
        "type": "assistant",
        "id": "msg_am_a1",
        "time": { "created": NOW - 180_000.0, "completed": NOW - 120_000.0 },
        "agent": "build",
        "finish": "stop",
        "content": [
            {
                "type": "tool",
                "id": "prt_am_0",
                "name": "read",
                "state": {
                    "status": "completed",
                    "input": { "filePath": "/Users/sam/Projects/kuvo/src/lib/validate.ts" },
                    "content": [{ "type": "text", "text": "export function validateEmail() {}" }]
                }
            },
            {
                "type": "tool",
                "id": "prt_am_1",
                "name": "shell",
                "state": {
                    "status": "completed",
                    "input": { "command": "cargo test -p kuvo --lib" },
                    "content": [{ "type": "text", "text": "test result: ok. 110 passed; 0 failed" }]
                }
            },
            {
                "type": "tool",
                "id": "prt_am_2",
                "name": "kuvo-browser_send_message",
                "title": "Sent a message",
                "doing": "Sending a message",
                "state": {
                    "status": "completed",
                    "input": {
                        "to": "ses_mira",
                        "wait": false,
                        "text": "Groundwork is merged and the shared validation helper covers both forms. Ready for review."
                    },
                    "content": [{
                        "type": "text",
                        "text": "Sent to Mira. Its answer, if it writes one, comes to you as a message."
                    }]
                }
            },
            {
                "type": "text",
                "text": "I let Mira know the form fix is ready. Checking release status and kicking off the package next."
            },
            {
                "type": "tool",
                "id": "prt_am_3",
                "name": "kuvo-browser_send_message",
                "title": "Sent a message",
                "doing": "Sending a message",
                "state": {
                    "status": "completed",
                    "input": {
                        "to": "&Release helper (ses_rel)",
                        "wait": true,
                        "text": "Is beta.35 green on macOS and Linux?"
                    },
                    "content": [{
                        "type": "text",
                        "text": "Release helper answered:\n\nBoth builds are green and notarized. Good to tag when ready."
                    }]
                }
            },
            {
                "type": "tool",
                "id": "prt_am_4",
                "name": "kuvo-browser_send_message",
                "title": "Sent a message",
                "doing": "Sending a message",
                "state": {
                    "status": "failed",
                    "input": {
                        "to": "ses_build",
                        "wait": false,
                        "text": "Trigger the beta.35 release package."
                    },
                    "error": "Error: Build runner can't be reached now. Try again later."
                }
            },
            {
                "type": "text",
                "text": "Release helper confirmed macOS and Linux are green, but Build runner couldn’t be reached to start packaging. I can retry once it’s back online."
            }
        ]
    }));
}

fn encode_png(img: image::RgbImage) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    img.write_to(&mut bytes, image::ImageFormat::Png).expect("encode stand-in PNG");
    bytes.into_inner()
}

fn fill_round_rect(img: &mut image::RgbImage, x: i32, y: i32, w: i32, h: i32, r: i32, color: [u8; 3]) {
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    let r = r.min(w / 2).min(h / 2).max(0);
    let (x0, y0, x1, y1) = (x.max(0), y.max(0), (x + w).min(iw), (y + h).min(ih));
    let rf = r as f32;
    for py in y0..y1 {
        for px in x0..x1 {
            let (lx, ly) = (px - x, py - y);
            let dx = if lx < r { r - lx } else if lx >= w - r { lx - (w - r - 1) } else { 0 };
            let dy = if ly < r { r - ly } else if ly >= h - r { ly - (h - r - 1) } else { 0 };
            if dx == 0 && dy == 0 {
                img.put_pixel(px as u32, py as u32, image::Rgb(color));
            } else {
                let dist = ((dx * dx + dy * dy) as f32).sqrt();
                if dist <= rf - 0.5 {
                    img.put_pixel(px as u32, py as u32, image::Rgb(color));
                } else if dist < rf + 0.5 {
                    let alpha = (rf + 0.5 - dist).clamp(0.0, 1.0);
                    let dst = img.get_pixel(px as u32, py as u32).0;
                    let blended = std::array::from_fn(|i| (dst[i] as f32 * (1.0 - alpha) + color[i] as f32 * alpha).round() as u8);
                    img.put_pixel(px as u32, py as u32, image::Rgb(blended));
                }
            }
        }
    }
}

fn bordered_round_rect(img: &mut image::RgbImage, x: i32, y: i32, w: i32, h: i32, r: i32, bw: i32, border: [u8; 3], fill: [u8; 3]) {
    fill_round_rect(img, x, y, w, h, r, border);
    fill_round_rect(img, x + bw, y + bw, w - bw * 2, h - bw * 2, (r - bw).max(0), fill);
}

fn fill_circle(img: &mut image::RgbImage, cx: i32, cy: i32, r: i32, color: [u8; 3]) {
    fill_round_rect(img, cx - r, cy - r, r * 2, r * 2, r, color);
}

fn pricing_hero_image() -> image::RgbImage {
    let (w, h) = (800, 500);
    let mut img = image::RgbImage::from_fn(w, h, |x, y| {
        let (t, s) = (y as f32 / h as f32, x as f32 / w as f32);
        image::Rgb([(247.0 - t * 6.0 - s * 4.0) as u8, (248.0 - t * 5.0 - s * 2.0) as u8, (252.0 - t * 2.0) as u8])
    });
    fill_round_rect(&mut img, 0, 0, 800, 38, 0, [255, 255, 255]);
    fill_round_rect(&mut img, 0, 38, 800, 1, 0, [232, 234, 240]);
    fill_circle(&mut img, 22, 19, 5, [238, 106, 95]);
    fill_circle(&mut img, 39, 19, 5, [244, 191, 79]);
    fill_circle(&mut img, 56, 19, 5, [97, 197, 84]);
    fill_round_rect(&mut img, 265, 9, 270, 20, 10, [242, 244, 248]);
    fill_round_rect(&mut img, 352, 17, 96, 4, 2, [175, 182, 194]);
    fill_round_rect(&mut img, 56, 56, 22, 22, 6, [25, 28, 36]);
    fill_round_rect(&mut img, 88, 64, 54, 7, 3, [40, 44, 54]);
    for (i, nw) in [44, 48, 38].into_iter().enumerate() {
        fill_round_rect(&mut img, 320 + i as i32 * 64, 65, nw, 5, 2, [150, 156, 168]);
    }
    fill_round_rect(&mut img, 676, 56, 68, 24, 12, [25, 28, 36]);
    fill_round_rect(&mut img, 224, 106, 352, 18, 6, [26, 29, 38]);
    fill_round_rect(&mut img, 276, 134, 248, 12, 4, [132, 140, 154]);
    fill_round_rect(&mut img, 324, 154, 152, 8, 4, [166, 173, 186]);
    bordered_round_rect(&mut img, 304, 178, 192, 32, 16, 1, [224, 228, 236], [255, 255, 255]);
    fill_round_rect(&mut img, 326, 191, 56, 6, 3, [140, 146, 160]);
    fill_round_rect(&mut img, 404, 182, 88, 24, 12, [10, 122, 255]);
    fill_round_rect(&mut img, 424, 191, 48, 6, 3, [255, 255, 255]);
    for (idx, cx) in [56, 292, 528].into_iter().enumerate() {
        let featured = idx == 1;
        bordered_round_rect(&mut img, cx, 230, 216, 236, 14, if featured { 2 } else { 1 }, if featured { [10, 122, 255] } else { [226, 230, 238] }, [255, 255, 255]);
        if featured {
            fill_round_rect(&mut img, cx + 18, 248, 64, 18, 9, [231, 242, 255]);
            fill_round_rect(&mut img, cx + 29, 255, 42, 4, 2, [10, 122, 255]);
        } else {
            fill_round_rect(&mut img, cx + 18, 252, 56, 8, 4, [152, 158, 170]);
        }
        fill_round_rect(&mut img, cx + 18, 278, 76, 22, 5, [28, 31, 40]);
        fill_round_rect(&mut img, cx + 102, 289, 34, 7, 3, [170, 176, 188]);
        fill_round_rect(&mut img, cx + 18, 314, 180, 30, 8, if featured { [10, 122, 255] } else { [242, 244, 248] });
        fill_round_rect(&mut img, cx + 64, 326, 88, 6, 3, if featured { [255, 255, 255] } else { [54, 58, 68] });
        fill_round_rect(&mut img, cx + 18, 358, 180, 1, 0, [236, 238, 244]);
        for row in 0..4 {
            let ry = 374 + row * 20;
            fill_circle(&mut img, cx + 24, ry + 4, 4, if featured { [10, 122, 255] } else { [46, 158, 91] });
            let lw = 112 + ((idx * 17 + row as usize * 23) % 44) as i32;
            fill_round_rect(&mut img, cx + 38, ry + 1, lw, 6, 3, [142, 148, 162]);
        }
    }
    img
}

fn checkout_desktop_image(dark: bool) -> image::RgbImage {
    let (bg, card, well, border, text, muted, accent) = if dark {
        ([18, 19, 23], [28, 30, 36], [22, 24, 29], [48, 52, 62], [236, 238, 244], [126, 132, 148], [46, 139, 255])
    } else {
        ([245, 246, 249], [255, 255, 255], [244, 245, 248], [226, 229, 236], [34, 37, 46], [152, 158, 172], [10, 122, 255])
    };
    let mut img = image::RgbImage::from_pixel(720, 500, image::Rgb(bg));
    fill_round_rect(&mut img, 0, 0, 720, 44, 0, card);
    fill_round_rect(&mut img, 0, 44, 720, 1, 0, border);
    fill_round_rect(&mut img, 28, 14, 18, 18, 5, accent);
    fill_round_rect(&mut img, 56, 19, 92, 8, 4, text);
    fill_round_rect(&mut img, 616, 16, 76, 14, 7, well);
    bordered_round_rect(&mut img, 28, 68, 296, 396, 14, 1, border, card);
    fill_round_rect(&mut img, 50, 92, 118, 10, 4, text);
    fill_round_rect(&mut img, 50, 112, 164, 6, 3, muted);
    bordered_round_rect(&mut img, 50, 138, 252, 74, 10, 1, border, well);
    fill_round_rect(&mut img, 64, 154, 42, 42, 9, accent);
    fill_round_rect(&mut img, 118, 160, 102, 9, 4, text);
    fill_round_rect(&mut img, 118, 178, 136, 6, 3, muted);
    for row in 0..3 {
        let ry = 236 + row * 34;
        fill_round_rect(&mut img, 50, ry, 124 - row * 12, 7, 3, muted);
        fill_round_rect(&mut img, 254, ry, 48, 7, 3, text);
        fill_round_rect(&mut img, 50, ry + 18, 252, 1, 0, border);
    }
    fill_round_rect(&mut img, 50, 356, 76, 11, 4, text);
    fill_round_rect(&mut img, 222, 352, 80, 18, 5, text);
    fill_round_rect(&mut img, 50, 404, 252, 36, 9, well);
    fill_circle(&mut img, 68, 422, 6, [46, 158, 91]);
    fill_round_rect(&mut img, 84, 419, 178, 6, 3, muted);
    bordered_round_rect(&mut img, 344, 68, 348, 396, 14, 1, border, card);
    fill_round_rect(&mut img, 368, 92, 134, 10, 4, text);
    fill_round_rect(&mut img, 368, 120, 300, 36, 9, text);
    fill_round_rect(&mut img, 474, 135, 88, 7, 3, card);
    for (idx, label_w) in [72, 96, 64].into_iter().enumerate() {
        let fy = 180 + idx as i32 * 64;
        fill_round_rect(&mut img, 368, fy, label_w, 6, 3, muted);
        bordered_round_rect(&mut img, 368, fy + 14, 300, 34, 8, 1, border, well);
        fill_round_rect(&mut img, 382, fy + 28, 132 + idx as i32 * 18, 6, 3, muted);
    }
    fill_round_rect(&mut img, 368, 384, 300, 42, 10, accent);
    fill_round_rect(&mut img, 468, 401, 100, 8, 4, [255, 255, 255]);
    img
}

fn checkout_mobile_image() -> image::RgbImage {
    let (bg, card, well, border, text, muted, accent) = ([245, 246, 249], [255, 255, 255], [243, 244, 248], [226, 229, 236], [34, 37, 46], [150, 156, 170], [10, 122, 255]);
    let mut img = image::RgbImage::from_pixel(420, 560, image::Rgb(bg));
    fill_round_rect(&mut img, 0, 0, 420, 54, 0, card);
    fill_round_rect(&mut img, 0, 54, 420, 1, 0, border);
    fill_round_rect(&mut img, 170, 10, 80, 6, 3, [215, 218, 226]);
    fill_round_rect(&mut img, 24, 28, 18, 16, 4, accent);
    fill_round_rect(&mut img, 52, 32, 118, 8, 4, text);
    fill_round_rect(&mut img, 344, 30, 52, 12, 6, well);
    bordered_round_rect(&mut img, 20, 72, 380, 92, 14, 1, border, card);
    fill_round_rect(&mut img, 36, 90, 48, 48, 10, accent);
    fill_round_rect(&mut img, 98, 96, 132, 9, 4, text);
    fill_round_rect(&mut img, 98, 114, 168, 6, 3, muted);
    fill_round_rect(&mut img, 314, 102, 68, 14, 5, text);
    bordered_round_rect(&mut img, 20, 180, 380, 356, 14, 1, border, card);
    fill_round_rect(&mut img, 36, 200, 348, 40, 10, text);
    fill_round_rect(&mut img, 160, 217, 100, 7, 3, card);
    for (idx, lw) in [78, 104, 68].into_iter().enumerate() {
        let fy = 262 + idx as i32 * 64;
        fill_round_rect(&mut img, 36, fy, lw, 6, 3, muted);
        bordered_round_rect(&mut img, 36, fy + 14, 348, 36, 9, 1, border, well);
        fill_round_rect(&mut img, 50, fy + 29, 152 + idx as i32 * 14, 6, 3, muted);
    }
    fill_round_rect(&mut img, 36, 472, 348, 44, 11, accent);
    fill_round_rect(&mut img, 154, 490, 112, 8, 4, [255, 255, 255]);
    img
}

/// The pictures staged, with one opened in the viewer.
fn open_a_picture(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(view) = workspace.conversation() {
        view.update(cx, |view, cx| view.open_picture("msg_pic2#1", window, cx));
    }
}

/// A conversation with a single picture, opened in the Stage viewer (no nav arrows or filmstrip).
fn open_single_picture(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(view) = workspace.conversation() {
        view.update(cx, |view, cx| view.open_picture("msg_single_pic#0", window, cx));
    }
}

fn stage_single_picture(stand_in: &StandIn) {
    use serde_json::json;
    let path = "/Users/sam/Projects/website/shots/pricing-hero.png";
    stand_in.set_facts("ses_dark", |facts| {
        facts["title"] = json!("Pricing hero review");
    });
    stand_in.put_file(path, encode_png(pricing_hero_image()));
    stand_in.add_message("ses_dark", json!({
        "type": "user", "id": "msg_single_pic", "time": { "created": NOW - 60_000.0 },
        "text": "Does the new pricing hero spacing look balanced?",
        "files": [{ "mime": "image/png", "name": "pricing-hero.png", "url": format!("file://{path}") }],
    }));
    stand_in.add_message("ses_dark", json!({
        "type": "assistant", "id": "msg_single_reply",
        "time": { "created": NOW - 50_000.0, "completed": NOW - 45_000.0 },
        "agent": "build", "finish": "stop",
        "content": [
            { "type": "text", "text": "Yes — the headline spacing, tier cards, and primary button contrast look balanced." },
        ],
    }));
}

fn tool_groups_open(workspace: &mut Workspace, _window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(view) = workspace.conversation() {
        view.update(cx, |view, cx| {
            for group in ["msg_s5", "msg_s5#6"] {
                view.toggle(kuvo::entries::Fold::Group(group.to_owned()), cx);
            }
        });
    }
}

/// Real conversation with a long steps-group summary line and a question card with long options
/// (reproducing `.feedback/text-overflow.png` and option overflow in light and dark).
fn stage_text_overflow(stand_in: &StandIn) {
    use serde_json::json;
    stand_in.set_facts("ses_dark", |facts| {
        facts["title"] = json!("Outputs popover rework & image viewer");
    });
    stand_in.add_message("ses_dark", json!({
        "type": "user", "id": "msg_ov_u1", "time": { "created": NOW - 120_000.0 },
        "text": "Queue the Outputs popover rework, check the image viewer branch, and ask which draft direction to pursue first."
    }));
    stand_in.add_message("ses_dark", json!({
        "type": "assistant", "id": "msg_ov_a1", "time": { "created": NOW - 110_000.0, "completed": NOW - 95_000.0 },
        "agent": "build", "finish": "stop",
        "content": [
            { "type": "tool", "id": "ov_1", "name": "kuvo-browser_queue_list", "title": "Listed the Work Queue", "doing": "Listing the Work Queue", "state": { "status": "completed", "input": { "search": "image viewer" }, "content": [{ "type": "text", "text": "2 items" }] } },
            { "type": "tool", "id": "ov_2", "name": "shell", "state": { "status": "completed", "input": { "command": "cd ~/Projects/kuvo-ai && git worktree add -q -b kuvo/900d-popover-viewer ~/.kuvo-code/tmp/sol/900d main" }, "content": [{ "type": "text", "text": "" }] } },
            { "type": "tool", "id": "ov_3", "name": "kuvo-browser_queue_add", "title": "Added \"Outputs popover rework (cluttered, hundreds of sessions, unclear wake-ups)\" to the Work Queue", "doing": "Adding to the Work Queue", "state": { "status": "completed", "input": { "title": "Outputs popover rework (cluttered, hundreds of sessions, unclear wake-ups)" }, "content": [{ "type": "text", "text": "Added #932" }] } },
            { "type": "tool", "id": "ov_4", "name": "kuvo-browser_start_session", "title": "Started a session", "doing": "Starting a session", "state": { "status": "completed", "input": { "title": "Drafts: one image viewer + a calmer Outputs popover (Gemini Neon)" }, "content": [{ "type": "text", "text": "Started" }] } },
            { "type": "tool", "id": "ov_5", "name": "shell", "state": { "status": "completed", "input": { "command": "cd ~/.kuvo-code/bots/bot_drisrnywfr/workspace/notes && echo \"- 11-20 00:3x: user OK'd drafts\" >> log.md" }, "content": [{ "type": "text", "text": "" }] } },
            { "type": "text", "text": "Queued #932 and set up the worktree. Before building the unified image viewer, which behavior do you want for large screenshots and tiny icons?" }
        ]
    }));
    stand_in.set_asks("ses_dark", vec![json!({
        "id": "frm_ov", "kind": "question", "session": "ses_dark", "title": "Question",
        "questions": [{
            "key": "q0",
            "title": "Unified image viewer direction",
            "question": "Which viewer layout should replace the lightbox across conversations, the Outputs popover, Work Queue attachments, and the side panel?",
            "options": [
                {
                    "value": "stage",
                    "label": "Stage (floating dark gallery)",
                    "description": "Always-dark scrim with edge chevrons, bottom filmstrip for multi-image galleries, and floating zoom + action pills"
                },
                {
                    "value": "sheet",
                    "label": "Sheet (theme-native Quick Look dialog)",
                    "description": "Centered elevated card matching Kuvo's light and dark themes with zero chrome overlapping the image canvas"
                },
                {
                    "value": "loupe",
                    "label": "Loupe (full-bleed inspector with named thumbnail rail)",
                    "description": "Edge-to-edge toolbar, collapsible left filename rail for 20+ screenshots, and a bottom metadata status bar"
                }
            ],
            "multiple": false, "custom": true, "required": true
        }]
    })]);
}

fn text_overflow_open(workspace: &mut Workspace, _window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(view) = workspace.conversation() {
        view.update(cx, |view, cx| {
            view.toggle(kuvo::entries::Fold::Group("msg_ov_a1".to_owned()), cx);
        });
    }
}

/// Real conversation with a `#840` work-item reference chip near the bottom, long title, and
/// markdown body (reproducing `.feedback/work-item-popover-dark.png` in light and dark).
fn stage_work_item_hover(stand_in: &StandIn) {
    use serde_json::json;
    stand_in.set_facts("ses_dark", |facts| {
        facts["title"] = json!("Outputs popover rework & image viewer");
    });
    stand_in.work_item_fixture("wqi_234", json!({
        "owner": "ses_dark",
        "number": 840,
        "title": "Text runs past the right edge instead of wrapping or truncating: question options and step summaries",
        "body": "The user, 11-18 13:53: \"file this view bug\", with the shot below. **What's wrong:** in a question card from a bot (`AskCard`), long option rows and `steps()` summaries overflow instead of wrapping or truncating with an ellipsis.",
        "status": "todo",
        "priority": "normal",
        "labels": ["lbl_design"],
        "assignee": null,
    }), None);
    // (In parentheses, as agents often write it: the chip stands in for them.)
    let reply = "Added. It's now part of the shared text-row fix (#840), for single-line truncation and wrapping options.";
    let start = reply.find("#840").unwrap();
    stand_in.work_queue_mention_fixture(reply, json!([
        { "start": start, "end": start + 4, "number": 840 }
    ]));
    stand_in.add_message("ses_dark", json!({
        "type": "user", "id": "msg_wh_u1", "time": { "created": NOW - 120_000.0 },
        "text": "We had a text overflow problem happening in a couple places, we can add this to the list: it also happened in question prompts' options."
    }));
    stand_in.add_message("ses_dark", json!({
        "type": "assistant", "id": "msg_wh_a1", "time": { "created": NOW - 110_000.0, "completed": NOW - 95_000.0 },
        "agent": "build", "finish": "stop",
        "content": [
            { "type": "text", "text": "Looking at both overflow sites: the steps-group summary row in entries.rs and the numbered option rows in ask_card.rs both wrapped their text in flex_none containers, which stops GPUI from shrinking or wrapping the text inside its column." }
        ]
    }));
    stand_in.add_message("ses_dark", json!({
        "type": "user", "id": "msg_wh_u2", "time": { "created": NOW - 80_000.0 },
        "text": "Add it to the Work Queue and hand it off so it gets fixed in one shared text-row primitive with before/after screenshots."
    }));
    stand_in.add_message("ses_dark", json!({
        "type": "assistant", "id": "msg_wh_a2", "time": { "created": NOW - 70_000.0, "completed": NOW - 30_000.0 },
        "agent": "build", "finish": "stop",
        "content": [
            { "type": "text", "text": reply }
        ]
    }));
}

/// A reply naming Work Queue items: in its words (a chip where it's written) and in a Work Queue
/// step's row, its words in two parts around what it did, the pointer on its first part (its one
/// time, Copy and Quote at its end).
fn stage_reply_items(stand_in: &StandIn) {
    use serde_json::json;
    stand_in.set_facts("ses_dark", |facts| {
        facts["title"] = json!("Conversation polish");
    });
    stand_in.work_item_fixture("wqi_234", json!({ "owner": "ses_dark", "number": 954, "title": "Remove slash commands from the desktop app and the box", "status": "in-progress" }), None);
    stand_in.work_item_fixture("wqi_235", json!({ "owner": "ses_dark", "number": 820, "title": "Drop the Worked for line under replies", "status": "done" }), None);
    let first = "I checked #954 first: it's under way, and #820 is done, so the list is clear.";
    let mention = |needle: &str, number: u64| { let start = first.find(needle).unwrap(); json!({ "start": start, "end": start + needle.len(), "number": number }) };
    stand_in.work_queue_mention_fixture(first, json!([mention("#954", 954), mention("#820", 820)]));
    stand_in.add_message("ses_dark", json!({
        "type": "user", "id": "msg_ri_u1", "time": { "created": NOW - 120_000.0 },
        "text": "Where are we on the polish items? Mark the slash commands one as in progress."
    }));
    stand_in.add_message("ses_dark", json!({
        "type": "assistant", "id": "msg_ri_a1", "time": { "created": NOW - 110_000.0, "completed": NOW - 60_000.0 },
        "agent": "build", "finish": "stop",
        "content": [
            { "type": "text", "text": first },
            { "type": "tool", "id": "call_ri_q", "name": "kuvo-browser_queue_update", "state": { "status": "completed", "input": { "item": "#954", "status": "in-progress" } } },
            { "type": "text", "text": "Moved it to In progress. Next I'll take the composer changes." }
        ]
    }));
    stand_in.add_message("ses_dark", json!({ "type": "idle", "id": "msg_ri_end", "time": { "created": NOW - 60_000.0 }, "outcome": "succeeded" }));
}

/// The pointer on a reply's first part.
fn hover_reply(workspace: &mut Workspace, _window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(view) = workspace.conversation() {
        view.update(cx, |view, cx| view.hover_row("msg_ri_a1", cx));
    }
}

/// The messages from other agents in view, the longer one opened.
fn agent_letters(workspace: &mut Workspace, _window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(view) = workspace.conversation() {
        view.update(cx, |view, cx| {
            view.toggle(kuvo::entries::Fold::Card("msg_s3a".to_owned(), 0), cx);
            let at = view.entries().iter().position(|entry| entry.key() == "msg_s3a").unwrap_or(0);
            view.list().list_state().scroll_to(gpui::ListOffset { item_ix: at.saturating_sub(2), offset_in_item: gpui::px(0.) });
        });
    }
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

fn reveal_drag_anchor(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    // The drag starts in an older turn. Bring its text into the transcript viewport
    // before looking up screen coordinates, even when newer cards fill the bottom.
    let view = workspace.conversation().expect("the conversation").read(cx);
    let item_ix = view.units().iter().position(|(_, units)| units.iter().any(|unit| unit.text.contains("Moving")))
        .expect("the conversation has the drag anchor");
    let list = view.list().list_state();
    list.pause_following_tail();
    list.scroll_to(gpui::ListOffset { item_ix, offset_in_item: px(0.) });
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

fn rings_more(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.rings().update(cx, |rings, cx| rings.show_more(cx));
}

fn rings_more_card(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.rings().update(cx, |rings, cx| {
        rings.show_more(cx);
        rings.show_card(4, cx);
    });
}

fn ring_card_1(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.rings().update(cx, |rings, cx| rings.show_card(1, cx));
}

fn ring_card_2(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.rings().update(cx, |rings, cx| rings.show_card(2, cx));
}

fn ring_card_3(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.rings().update(cx, |rings, cx| rings.show_card(3, cx));
}

/// Codex's card asking before a reset is used.
fn ring_card_confirm(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    workspace.rings().update(cx, |rings, cx| {
        rings.show_card(2, cx);
        rings.ask_reset("codex#pro".into(), cx);
    });
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
    FirstRunScreen { slug: "first-run-allow-asking", scenario: Scenario::Nothing, size: (1280., 820.), before: |stand_in| { nothing_allowed(stand_in); stand_in.set_permissions(|permissions| { permissions.accessibility = true; permissions.screen_recording = true; }); }, then: |run, cx| {
        to_allow(run, cx);
        if let Some(permissions) = run.permissions().cloned() {
            permissions.update(cx, |permissions, cx| {
                permissions.open_settings("fullDiskAccess", cx);
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
            cx.update(|cx| {
                // Each onboarding picture starts on a fresh desktop, even if an earlier
                // picture marked the shared app context as welcomed.
                kuvo::yours::init(None, cx);
                Theme::set_appearance(appearance, cx);
            });
            let stand_in = StandIn::first_run(Replies::Never, NOW, screen.scenario);
            (screen.before)(&stand_in);
            let client = BoxClient::new(stand_in.clone());
            let window = cx.open_window(size(px(screen.size.0), px(screen.size.1)), |window, cx| cx.new(|cx| Workspace::new(client, false, window, cx)))?;
            let any = window.into();
            cx.update_window(any, |root, _, cx| {
                let workspace = root.downcast::<Workspace>().expect("the workspace");
                workspace.update(cx, |workspace, cx| {
                    workspace.set_now(NOW, cx);
                    workspace.set_sky_hour(12., cx);
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

/// Where the Sessions sidebar's Projects heading is drawn in the stand-in (the add-project shot).
const PROJECTS_Y: f32 = 237.;

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
            cx.update(|cx| {
                use kuvo::yours::{Change, change};
                change(Change::TextSize(text_size), cx);
                let width = match screen.slug {
                    "sidebar-wide" => 420.,
                    "sidebar-narrow-sessions" => 228.,
                    "sidebar-narrow-bots" | "bots-compact-states" | "sidebar-search-results" | "sidebar-search-empty" => kuvo::sidebar::COMPACT_WIDTH,
                    _ => kuvo::sidebar::WIDTH,
                };
                change(Change::SidebarWidth(width), cx);
            });
            let stand_in = if screen.slug.starts_with("held-box") { StandIn::held_older(Replies::Never) } else if screen.empty { StandIn::empty(Replies::Never) } else { StandIn::at(Replies::Never, NOW) };
            // (Kept to print into a terminal once the screen has opened it.)
            let shell = stand_in.clone();
            if screen.slug == "sidebar-long-computer" {
                stand_in.set_elsewhere(true);
                stand_in.set_computer_name("WWWWW Engineering Workstation East Coast");
            }
            if screen.slug == "sidebar-narrow-bots" {
                stand_in.set_bot_now("bot_mira", serde_json::json!({ "working": 1, "asking": 0, "state": "working", "since": NOW - 60_000. }));
            }
            if screen.slug == "warn-failed-send" {
                stand_in.fail_sends();
            }
            if screen.slug == "warn-bot-away" {
                stand_in.set_elsewhere(true);
            }
            if screen.slug == "warn-stopped-turn" {
                stand_in.add_message("ses_dark", serde_json::json!({ "type": "user", "id": "msg_limit_u", "time": { "created": NOW - 60_000. }, "text": "Add rate limits per API key: 100 requests a minute." }));
                stand_in.add_message("ses_dark", serde_json::json!({ "type": "assistant", "id": "msg_limit_a", "time": { "created": NOW - 59_000., "completed": NOW - 58_000. }, "agent": "build", "finish": "error",
                    "content": [{ "type": "text", "text": "Added the sliding-window limiter keyed by API token. Next up is the integration test for burst traffic." }],
                    "error": { "name": "APIError", "data": { "message": "Claude usage limit reached. Your limit will reset at 5pm." } } }));
            }
            if screen.slug.starts_with("conversation-failed-step") {
                let refused = "Blocked: sleep 240 followed by: cd /Users/sam/Projects/website scripts/build.sh status tail -3 /private/tmp/claude-501/-Users-sam-Projects-website/28e29ba5/tasks/bcj6wfy9b.output. To wait for a condition, use Monitor with an until-loop (e.g. `until <check>; do sleep 2; done`). To wait for a command you started, use run_in_background: true. Do not chain shorter sleeps to work around this block.";
                let raw = format!("<tool_use_error>{refused}</tool_use_error>");
                stand_in.add_message("ses_dark", serde_json::json!({ "type": "user", "id": "msg_fail_u", "time": { "created": NOW - 60_000. }, "text": "Wait for the build, then check the toggle." }));
                stand_in.add_message("ses_dark", serde_json::json!({ "type": "assistant", "id": "msg_fail_a", "time": { "created": NOW - 59_000., "completed": NOW - 58_000. }, "agent": "build", "finish": "stop", "content": [
                    { "type": "text", "text": "Waiting on the build before I check the toggle." },
                    { "type": "tool", "id": "t_fail", "name": "shell", "state": { "status": "error", "input": { "command": "sleep 240; cd /Users/sam/Projects/website && scripts/build.sh status" }, "error": { "type": "tool", "message": raw }, "content": [{ "type": "text", "text": raw }] },
                      "failure": { "line": "Ran 1 command · blocked", "reason": "blocked", "error": refused } },
                    { "type": "text", "text": "The build will tell me when it's done." }
                ] }));
                stand_in.add_message("ses_dark", serde_json::json!({ "type": "assistant", "id": "msg_fail_b", "time": { "created": NOW - 57_000., "completed": NOW - 56_000. }, "agent": "build", "finish": "stop", "content": [
                    { "type": "tool", "id": "t_fail2", "name": "read", "state": { "status": "error", "input": { "filePath": "/Users/sam/Projects/website/src/theme/toggle.tsx" }, "error": { "type": "tool", "message": "<tool_use_error>File does not exist. Note: your current working directory is /Users/sam/Projects/website.</tool_use_error>" } },
                      "failure": { "line": "Read 1 file · not found", "reason": "not found", "error": "File does not exist. Note: your current working directory is /Users/sam/Projects/website." } },
                    { "type": "text", "text": "The toggle lives in `src/settings/Theme.tsx`; checking it there." }
                ] }));
            }
            if screen.slug.starts_with("conversation-subagent") {
                stand_in.subagent_of("ses_sub", "ses_signup");
                stand_in.set_model_state("ses_sub", serde_json::json!({ "model": { "providerID": "anthropic", "id": "claude-haiku-4-5" }, "name": "Haiku 4.5", "runs": true, "fixed": true }));
                stand_in.add_message("ses_sub", serde_json::json!({ "type": "user", "id": "msg_sub_u", "time": { "created": NOW - 60_000. }, "text": "Check the other forms on the site for the same wiping bug." }));
                stand_in.add_message("ses_sub", serde_json::json!({ "type": "assistant", "id": "msg_sub_a", "time": { "created": NOW - 59_000., "completed": NOW - 50_000. }, "agent": "build", "finish": "stop", "content": [
                    { "type": "tool", "id": "t_sub1", "name": "grep", "state": { "status": "completed", "input": { "pattern": "reset\\(" }, "content": [{ "type": "text", "text": "src/forms/Contact.tsx\nsrc/forms/Newsletter.tsx" }] } },
                    { "type": "text", "text": "The contact and newsletter forms reset the same way on an error. The sign-up fix applies to both." }
                ] }));
                if screen.slug.ends_with("no-messages") {
                    stand_in.set_facts("ses_sub", |facts| facts["messages"] = serde_json::json!({ "via": "none", "note": "Claude Code's Explore agents can't take messages." }));
                }
            }
            if screen.slug == "sidebar-show-more" {
                for _ in 0..40 {
                    use kuvo_box::transport::Transport as _;
                    let _ = stand_in.send(&kuvo_box::transport::Request::post("kuvo/sessions", serde_json::json!({ "project": format!("{}/Projects/website", kuvo::stand_in::HOME) })));
                }
            }
            if screen.slug == "sidebar-search-results" {
                stand_in.set_facts("ses_signup", |facts| {
                    facts["bot"] = serde_json::json!("bot_mira");
                    facts["title"] = serde_json::json!("Fix the sign-up form wiping on a taken email when the previous invitation has expired");
                });
            }
            if screen.slug.starts_with("library-") {
                stand_in.library_sample(screen.slug == "library-empty");
            }
            if screen.slug.starts_with("market-") {
                stand_in.marketplace_sample(kuvo::stand_in::marketplace::Sample::InUse);
            }
            match screen.slug {
                "usage-many" => stand_in.set_usage_sample(UsageSample::Many),
                "usage-one" => stand_in.set_usage_sample(UsageSample::One),
                "usage-failed" => stand_in.set_usage_sample(UsageSample::Failed),
                "usage-stale" => stand_in.set_usage_sample(UsageSample::Stale),
                "rail-card-antigravity-4" | "rail-card-codex-no-resets" => stand_in.set_usage_sample(UsageSample::CardsFew),
                "rail-card-antigravity-as-of" => stand_in.set_usage_sample(UsageSample::CardsAsOf),
                slug if slug.starts_with("rail-card-") => stand_in.set_usage_sample(UsageSample::Cards),
                _ => {}
            }
            if screen.slug.starts_with("settings-custom-auto") {
                stand_in.add_settings_section(kuvo::stand_in::settings::custom_auto::section(), kuvo::stand_in::settings::custom_auto::page());
            }
            if screen.slug.starts_with("settings-skills") {
                stand_in.add_settings_section(kuvo::stand_in::settings::skills::section(), kuvo::stand_in::settings::skills::page());
            }
            if screen.slug.starts_with("model-picker") {
                // A star, and more providers' models, as a box with several accounts has.
                stand_in.set_starred(vec![("openai", "gpt-6"), ("codex-cli", "gpt-6-luna"), ("anthropic", "claude-opus-5-5")]);
                stand_in.more_models(24);
                stand_in.offers_codex();
            }
            if screen.slug == "model-chip" || screen.slug == "market-add-agent-picker" || screen.slug == "settings-a-models-picker" {
                stand_in.set_starred(vec![("openai", "gpt-6"), ("anthropic", "claude-opus-5-5")]);
                stand_in.offers_codex();
            }
            match screen.slug {
                "model-picker-claude-code" => stand_in.claude_accounts(&[("sam@example.com", Some("Max")), ("Work", Some("Team"))]),
                "model-picker-accounts-one" => stand_in.claude_accounts(&[("sam@example.com", Some("Max"))]),
                "model-picker-accounts-two" | "model-picker-accounts-search" => stand_in.claude_accounts(&[("sam@example.com", Some("Max")), ("Work", Some("Team"))]),
                "model-picker-accounts-ten-open" => stand_in.claude_accounts(&[
                    ("sam@example.com", Some("Max")), ("Work", Some("Team")), ("Personal", Some("Pro")), ("Client A", Some("Team")), ("Client B", Some("Pro")),
                    ("design@example.com", Some("Max")), ("ops@example.com", Some("Pro")), ("Research", Some("Max")), ("Side project", Some("Pro")), ("qa@example.com", Some("Pro")),
                ]),
                _ => {}
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
            if screen.slug == "activity-foreground-command" {
                stand_in.set_background("ses_signup", serde_json::json!({
                    "commands": [{ "id": "shl_triage", "command": "bash scripts/triage.sh --since beta.33 --all", "started": NOW - 754_000.0, "agents": true }],
                    "subagents": [],
                }), &[("shl_triage", "checked 12 of 40 reports")]);
            }
            if screen.slug.starts_with("send-while-working") {
                stand_in.set_queue("ses_signup", vec![
                    serde_json::json!({ "id": "msg_w0", "words": "Check the error copy against the tone guide too", "files": [], "keyword": null, "from": null, "on": "engine", "delivery": "steer", "held": false }),
                    serde_json::json!({ "id": "msg_w1", "words": "Then do the same for the password field: keep it, and say what's wrong with it", "files": [], "keyword": null, "from": null, "on": "engine", "delivery": "queue", "held": false }),
                    serde_json::json!({ "id": "msg_w2", "words": "The design review is at 3: the error copy should match the new tone guide.", "files": ["tone-guide.pdf"], "keyword": null, "from": "Mira", "on": "box", "delivery": "queue", "held": false }),
                ]);
            }
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
            if screen.slug == "queue-edit-pictures" {
                stand_in.work_item_fixture("wqi_231", serde_json::json!({
                    "body": "The email field should keep its value if the address is taken.\n\n![signup.png](kuvo-attachment:file/signup)\n\nCheck the other fields after submitting.\n\n![signup-wide.png](kuvo-attachment:file/signup-wide)",
                    "files": [{"name": "signup.png", "type": "image/png", "anchor": "signup", "at": 1},
                              {"name": "signup-wide.png", "type": "image/png", "anchor": "signup-wide", "at": 2}],
                }), Some(("signup.png", include_bytes!("../../../shots/screens-composer-row-light.png").to_vec())));
                stand_in.work_item_fixture("wqi_231", serde_json::json!({}), Some(("signup-wide.png", include_bytes!("../../../shots/screens-composer-row-dark.png").to_vec())));
            }
            if matches!(screen.slug, "queue-item-references" | "queue-item-links" | "queue-item-hover-card") {
                let body = "Coordinate the error copy with #234 before merging. The page ranked #1.";
                let mention = |body: &str, number: u64| {
                    let tag = format!("#{number}");
                    let start = body.find(&tag).unwrap();
                    serde_json::json!({"start": start, "end": start + tag.len(), "number": number})
                };
                let current_title = "Keep what was typed when the sign-up email is taken";
                let target_title = "Dark mode for the settings page";
                stand_in.work_item_fixture("wqi_234", serde_json::json!({
                    "title": target_title,
                    "body": "Use the new theme tokens throughout Settings. Check the sidebar and permission panels in both appearances.",
                    "status": "in-progress", "priority": "high", "labels": ["lbl_design"],
                    "assignee": {"kind": "bot", "id": "bot_mira", "name": "Mira"},
                    "itemRelations": [{"kind": "mentioned-in", "id": "wqi_231", "number": 231, "title": current_title}],
                    "canLinkItems": true,
                }), None);
                let mut links = vec![serde_json::json!({
                    "kind": "mentions", "id": "wqi_234", "number": 234, "title": target_title,
                })];
                if screen.slug == "queue-item-links" {
                    let post = "Write the launch post, with the three screenshots and the short video from the design review";
                    let post_body = "Include the sign-up fix from #231 in the launch post.";
                    stand_in.work_item_fixture("wqi_235", serde_json::json!({
                        "title": post, "body": post_body, "bodyMentions": [mention(post_body, 231)],
                        "itemRelations": [{"kind": "mentions", "id": "wqi_231", "number": 231, "title": current_title}],
                        "canLinkItems": true,
                    }), None);
                    links.extend([
                        serde_json::json!({"kind": "mentioned-in", "id": "wqi_235", "number": 235, "title": post}),
                        serde_json::json!({"kind": "blocks", "id": "wqi_236", "number": 236, "title": "Nightly builds for the Linux packages"}),
                        serde_json::json!({"kind": "related", "id": "wqi_237", "number": 237, "title": "Get the Lighthouse score above 90"}),
                    ]);
                }
                stand_in.work_item_fixture("wqi_231", serde_json::json!({
                    "title": current_title, "body": body, "bodyMentions": [mention(body, 234)],
                    "itemRelations": links, "canLinkItems": true, "files": [], "links": [], "notes": [],
                }), None);
            }
            if screen.slug == "queue-new-many-labels" {
                stand_in.many_labels();
            }
            if matches!(screen.slug, "accounts-update-available" | "accounts-updating") {
                let mut metadata = serde_json::json!({ "latestVersion": "2.4.2", "outdated": true, "updates": true });
                if screen.slug == "accounts-updating" {
                    metadata["programUpdate"] = serde_json::json!({ "state": "updating", "at": 1 });
                }
                stand_in.set_agent_metadata("claude-code", metadata);
            }
            // Computers: only this Mac, and a long list.
            if screen.slug == "computers-alone" {
                stand_in.computers_alone();
            }
            if matches!(screen.slug, "computers-roles" | "computer-roles" | "computer-rename" | "computers-handover" | "computer-remove" | "computer-remove-away" | "computer-remove-last" | "computers-removed") {
                stand_in.setup_roles();
            }
            if screen.slug == "computer-remove-last" {
                stand_in.computers_alone();
            }
            if screen.slug == "computer-remove-away" {
                stand_in.more_computers(3);
            }
            if screen.slug == "computer-blocked" {
                stand_in.block_vouching("key_devbox");
            }
            // Add a Machine, as far as each of its states.
            match screen.slug {
                "computer-add-key" => stand_in.adding_at("mac-mini", "asking"),
                "computer-add-recover" => stand_in.adding_at("mac-mini", "recovery"),
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
                let later = vec!["screenRecording".to_owned(), "accessibility".to_owned(), "fullDiskAccess".to_owned()];
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
                    _ => stand_in.set_permissions(|permissions| *permissions = Permissions { screen_recording: true, accessibility: true, full_disk_access: true, ..Permissions::nothing_allowed() }),
                }
            }
            if screen.slug == "settings-a-unreached" {
                stand_in.forget_peers();
            }
            if screen.slug == "settings-a-permissions-services" {
                stand_in.set_sandbox(serde_json::json!({ "on": true, "needsYou": true, "able": true, "agents": true, "gaps": ["services"], "needs": "user namespaces", "why": "Commands run in the sandbox, but this machine doesn’t let it keep them from your login’s services, like systemd. Until it allows user namespaces, agents don’t use your vault here and goals don’t run on their own.", "action": "help", "actionLabel": "How to Check…", "opens": "https://www.kuvo.run/code/docs/machines/#sandbox" }));
            }
            if screen.slug.starts_with("bot-move-") {
                stand_in.set_move_scenario(match screen.slug {
                    "bot-move-moving" => MoveScenario::Copying,
                    "bot-move-failed" | "bot-move-failed-notifications" => MoveScenario::Failed,
                    _ => MoveScenario::Ready,
                });
            }
            if matches!(screen.slug, "bot-edit-long-personality" | "bot-edit-personality-picker") {
                BoxClient::new(stand_in.clone()).change_bot("bot_mira", &kuvo_box::bots::BotChange {
                    personality: Some(Some("custom.pirate".into())),
                    ..Default::default()
                })?;
            }
            if screen.slug.starts_with("bot-wake-") {
                use serde_json::json;
                let waking = screen.slug == "bot-wake-row-waking";
                stand_in.set_bot_now("bot_mira", json!({ "working": usize::from(waking), "asking": 0, "waking": waking, "state": if waking { "working" } else { "idle" }, "since": NOW }));
                let count = match screen.slug {
                    "bot-wake-row-none" | "bot-wake-list-empty" => 0,
                    "bot-wake-list-many" => 9,
                    _ => 1,
                };
                let notes = ["Review the deployment", "Check the staging error rate", "Draft the Friday release notes", "Review signup conversion", "Run the accessibility audit", "Check links in the help center", "Review pending translations", "Send the weekly traffic summary", "Check the production backup"];
                let wakes = notes.iter().take(count).enumerate().map(|(index, note)| {
                    let summary = if screen.slug == "bot-wake-list-long-note" {
                        "Review this morning’s rollout across all regions, compare signup conversion with the last successful release, check the error rate for each browser and flag the steps that need another look."
                    } else { note };
                    json!({ "id": format!("wake{index}"), "due": NOW / 1000. + 180. + index as f64 * 3600., "every": if index % 2 == 0 { Some("1d") } else { None }, "summary": summary, "cancelKey": format!("wake{index}") })
                }).collect();
                let watches = if count > 1 { vec![json!({ "letters": "ci", "every": "30m", "summary": "Watch the CI checks", "next": NOW / 1000. + 900. })] } else { Vec::new() };
                stand_in.set_bot_wakes("bot_mira", wakes, watches);
            }
            if screen.slug == "bot-waiting" {
                stand_in.refuse_bot_edit("bot_scout", "studio-mini", "studio-mini has no personality “Ship’s Cook”.");
            }
            if screen.slug == "panel-browser" {
                stand_in.set_elsewhere(true);
                stand_in.browser_pages(serde_json::json!({ "active": "P", "pages": [{ "targetId": "P", "url": "http://localhost:3000/signup", "title": "", "loading": false, "canGoBack": true, "canGoForward": false, "working": false }] }));
            }
            if screen.slug.starts_with("panel-") {
                use serde_json::json;
                stand_in.set_background("ses_signup", json!({
                    "commands": [{ "id": "sh_dev", "command": "bun run dev", "started": NOW - 95_000.0, "folder": kuvo::stand_in::git::website() }],
                    "subagents": [],
                }), &[("sh_dev", "$ next dev --turbopack\n▲ Next.js 15.3.1 (Turbopack)\n- Local:   http://localhost:3000\n\n✓ Ready in 612ms\n○ Compiling /signup …\n✓ Compiled /signup in 184ms\nPOST /api/signup 409 in 19ms\nPOST /api/signup 201 in 31ms")]);
                let day = 86_400_000.0;
                let done: Vec<_> = [("ses_d1", "Check 409 retry path"), ("ses_limits", "Audit email rate limits"), ("ses_d3", "Scan signup tests"), ("ses_d4", "Verify focus ring order"), ("ses_d5", "Compare OAuth conflicts"), ("ses_d6", "Read Postgres error codes")]
                    .iter()
                    .map(|(session, title)| json!({ "session": session, "title": title, "started": NOW - 3.0 * day, "ended": NOW - 3.0 * day + 18_000.0, "state": "finished", "model": "Claude Sonnet 4.6", "effort": "High" }))
                    .collect();
                stand_in.set_subagents_done("ses_signup", done);
                stand_in.answer_side_questions(&[
                    "Postgres raises **23505** atomically on a unique violation, so two sign-ups with the same email can’t both pass a check made first.",
                    "Yes: counting 409s alongside 201s stops anyone probing whether an address already has an account.",
                ]);
            }
            if screen.slug == "panel-bot-side-chat" {
                stand_in.answer_side_questions(&["The queue: anything that's waiting on you, then what's due today, then what moved overnight."]);
            }
            if screen.slug == "session-menu-held" {
                stand_in.hold_session("ses_dark", Some("Moving to Studio…"));
            }
            if matches!(screen.slug, "outputs-popover-full" | "outputs-popover-collapsed" | "outputs-popover-loading") {
                use serde_json::json;
                stand_in.set_background("ses_signup", json!({
                    "commands": [{ "id": "sh_dev", "command": "npm run dev", "started": NOW - 12. * 60_000. }],
                    "subagents": [
                        { "session": "ses_sub0", "title": "Check the signup forms", "since": NOW - 60_000., "model": "GPT-5.4" },
                        { "session": "ses_sub1", "title": "Run the browser tests", "since": NOW - 30_000., "model": "Claude Sonnet 4.6" },
                    ],
                }), &[]);
                stand_in.set_subagents_done("ses_signup", ["Claude Opus 4.6", "GPT-5.4", "Gemini 3 Pro", "Claude Sonnet 4.6", "GPT-5.4", "Claude Sonnet 4.6"].iter().enumerate().map(|(ix, model)| json!({
                    "session": format!("ses_done{ix}"), "title": format!("Finished check {ix}"), "started": NOW - 3_600_000., "ended": NOW - 600_000., "state": "finished", "model": model,
                })).collect());
                let mini = |r: u8, g: u8, b: u8| {
                    let img = image::RgbImage::from_fn(240, 150, |x, y| if y < 18 { image::Rgb([r / 2, g / 2, b / 2]) } else if x > 20 && x < 220 && y > 40 && y < 110 { image::Rgb([r, g, b]) } else { image::Rgb([236, 238, 242]) });
                    let mut out = std::io::Cursor::new(Vec::new());
                    img.write_to(&mut out, image::ImageFormat::Png).expect("png");
                    out.into_inner()
                };
                for (ix, (name, colour)) in [("insights-overview-light.png", (64u8, 132u8, 240u8)), ("insights-overview-dark.png", (40, 44, 60)), ("signup-narrow.png", (236, 120, 80))].iter().enumerate() {
                    stand_in.add_output("ses_signup", json!({
                        "id": format!("art_img_{ix:021}"), "path": format!("/Users/sam/Projects/website/shots/{name}"), "name": name,
                        "kind": "image", "type": "image/png", "latest": { "n": 1 }
                    }), &mini(colour.0, colour.1, colour.2));
                }
                for (ix, path) in ["/Users/sam/Projects/website/src/fleet.rs", "/Users/sam/Projects/website/tests/fleet.rs", "/Users/sam/Projects/website/notes/signup-review.md"].iter().enumerate() {
                    let name = path.rsplit('/').next().unwrap();
                    stand_in.add_output("ses_signup", json!({
                        "id": format!("art_doc_{ix:021}"), "path": path, "name": name,
                        "kind": if name.ends_with(".md") { "document" } else { "code" }, "type": "text/plain", "latest": { "n": 1 }
                    }), b"// fleet");
                }
                let client = BoxClient::new(stand_in.clone());
                stand_in.answer_side_questions(&["Yes, it's atomic.", "Three of them."]);
                let _ = client.ask_aside("ses_signup", "Why does 23505 mean the email is taken?", None, None, &[]);
                let _ = client.ask_aside("ses_signup", "Find three random Mac photos", None, None, &[]);
                stand_in.set_queue("ses_signup", vec![]);
            }
            if screen.slug.starts_with("activity-summarize") {
                use serde_json::json;
                stand_in.add_settings_section(
                    json!({ "id": "conversations", "title": "Conversations", "group": "Agents & Tools", "icon": "message", "line": "When supported agents summarize long conversations on this computer." }),
                    json!({ "id": "conversations", "title": "Conversations", "groups": [{ "id": "context", "title": "Context", "rows": [{
                        "id": "compaction", "title": "Summarize long conversations at", "kind": "choice", "value": "400000", "can": true,
                        "choices": [{ "id": "200000", "title": "200k tokens" }, { "id": "400000", "title": "400k tokens" }, { "id": "600000", "title": "600k tokens" }, { "id": "never", "title": "Never (large windows)" }] }] }] }),
                );
                let choices = json!([{ "id": "200000", "title": "200k" }, { "id": "400000", "title": "400k", "line": "Default" }, { "id": "600000", "title": "600k" }, { "id": "never", "title": "Never" }]);
                let session = if screen.slug == "activity-summarize-unmeasured" { "ses_recipes" } else { "ses_signup" };
                stand_in.set_summarize(session, json!({ "value": "400000", "label": "Summarize at", "choices": choices, "line": "All conversations" }));
                let used = match screen.slug { "activity-summarize-near" => 372_000, "activity-summarize-unmeasured" => 0, _ => 89_000 };
                stand_in.set_context(session, json!({ "used": used, "window": 1_000_000, "summarizeAt": 400_000 }));
            }
            if screen.slug.starts_with("activity-context") || screen.slug.starts_with("activity-pinned") || screen.slug.starts_with("small-1123-b-4") {
                use serde_json::json;
                let busy = screen.slug == "activity-pinned-busy";
                let (used, session) = match screen.slug { "activity-pinned-busy" => (382_000, "ses_signup"), "activity-context-quiet" => (12_000, "ses_recipes"), _ => (184_000, "ses_signup") };
                stand_in.set_context(session, json!({ "used": used, "window": 1_000_000, "summarizeAt": 400_000 }));
                if session == "ses_signup" {
                    let titles: &[(&str, &str)] = if busy {
                        &[("Check the signup forms", "working"), ("Inspect the email rate limits", "asking"), ("Verify dark contrast", "working"), ("Measure the narrow layout", "working"), ("Run the browser tests", "working")]
                    } else {
                        &[("Check the signup forms", "working"), ("Run the browser tests", "working")]
                    };
                    let sessions: Vec<_> = titles.iter().enumerate().map(|(ix, (title, state))| json!({ "session": format!("ses_w{ix}"), "title": title, "state": state })).collect();
                    let asking = titles.iter().filter(|(_, state)| *state == "asking").count();
                    let commands: Vec<_> = (if busy { &["npm run dev", "npx vitest --watch", "tail -f logs/signup.log"][..] } else { &["npm run dev"][..] })
                        .iter().enumerate().map(|(ix, command)| json!({ "id": format!("sh_{ix}"), "command": command, "started": NOW - 6. * 60_000. })).collect();
                    stand_in.set_background("ses_signup", json!({ "commands": commands, "subagents": [], "activity": { "working": titles.len() - asking, "asking": asking, "sessions": sessions } }), &[]);
                    let files = if busy { 11 } else { 3 };
                    for ix in 0..files {
                        let name = ["signup-review.md", "fleet.rs", "rate-limits.csv"][ix % 3];
                        stand_in.add_output("ses_signup", json!({
                            "id": format!("art_ctx_{ix:021}"), "path": format!("/Users/sam/Projects/website/notes/{ix}-{name}"), "name": format!("{ix}-{name}"),
                            "kind": "document", "type": "text/plain", "latest": { "n": 1 }
                        }), b"notes");
                    }
                }
            }
            if matches!(screen.slug, "conversation-activity-popover" | "conversation-header-running") {
                use serde_json::json;
                let commands: Vec<_> = ["npm run dev", "npx vitest --watch", "tail -f logs/signup.log"].iter().enumerate()
                    .map(|(index, command)| json!({ "id": format!("sh_{index}"), "command": command, "started": NOW })).collect();
                let subagents: Vec<_> = ["Check the signup forms", "Run the browser tests", "Review the report"].iter().enumerate()
                    .map(|(index, title)| json!({ "session": format!("ses_sub{index}"), "title": title, "since": NOW })).collect();
                stand_in.set_background("ses_signup", json!({ "commands": commands, "subagents": subagents }), &[]);
                for (index, (provider, model)) in [("codex-cli", "gpt-6"), ("codex-cli", "gpt-6"), ("claude-code", "opus")].iter().enumerate() {
                    stand_in.set_model_state(&format!("ses_sub{index}"), json!({
                        "model": { "providerID": provider, "id": model }, "runs": true
                    }));
                }

                stand_in.add_output("ses_signup", json!({
                    "id": "art_000000000000000000000001", "path": "/tmp/signup-review.md", "name": "signup-review.md",
                    "kind": "document", "type": "text/markdown", "latest": { "n": 1 }
                }), b"# Signup review");
                stand_in.add_output("ses_signup", json!({
                    "id": "art_000000000000000000000002", "path": "/tmp/signup-preview.svg", "name": "signup-preview.svg",
                    "kind": "image", "type": "image/svg+xml", "latest": { "n": 1 }
                }), include_bytes!("../../../assets/icons/activity.svg"));
                stand_in.set_queue("ses_signup", vec![]);
            }
            if matches!(screen.slug, "sessions-all-computers" | "sessions-computer-updating" | "sessions-computer-away" | "runs-on" | "runs-on-open") {
                stand_in.share_setup_lists();
                match screen.slug {
                    "runs-on" | "runs-on-open" => stand_in.setup_away("key_devbox", NOW - 3.0 * 3_600_000.0, false),
                    "sessions-computer-updating" => stand_in.setup_away("key_devbox", NOW - 90_000.0, true),
                    "sessions-computer-away" => stand_in.setup_away("key_devbox", NOW - 3.0 * 3_600_000.0, false),
                    _ => {}
                }
            }
            // (The box still reading its first figures.)
            if screen.slug == "insights-loading" {
                stand_in.set_insights(kuvo::stand_in::insights::Sample::Reading);
            }
            if matches!(screen.slug, "bot-chat-computer-away" | "bot-chat-computer-back") {
                stand_in.share_setup_lists();
                stand_in.enable_delivery();
                stand_in.move_bot("bot_release", "devbox");
                stand_in.set_elsewhere_session("bot_release", "ses_releases");
                if screen.slug == "bot-chat-computer-away" {
                    // (Read here while devbox answered, then it went off five minutes ago.)
                    stand_in.carry("key_devbox", "ses_releases");
                    stand_in.peer_box("key_devbox").expect("devbox").set_down(true);
                    stand_in.setup_away("key_devbox", NOW - 5.0 * 60_000.0, false);
                    stand_in.bot_computer_away("bot_release", Some(NOW - 5.0 * 60_000.0));
                }
            }
            if screen.slug == "bots-computer-away" {
                stand_in.move_bot("bot_release", "studio-mini");
                stand_in.bot_computer_away("bot_release", Some(NOW - 3.0 * 3_600_000.0));
            }
            if screen.slug == "bots-sessions-unreachable" {
                stand_in.sessions_readable(false);
                stand_in.set_bot_now("bot_mira", serde_json::json!({ "working": 0, "asking": 0, "state": "idle", "since": NOW }));
                stand_in.set_bot_now("bot_inbox", serde_json::json!({ "working": 1, "asking": 0, "state": "working", "since": NOW }));
                stand_in.add_bot("bot_build", "Build Runner", serde_json::json!({ "color": "mint", "shape": "round", "face": "calm" }));
                stand_in.add_bot("bot_review", "Review Helper", serde_json::json!({ "color": "orange", "shape": "capsule", "face": "bright" }));
            }
            if matches!(screen.slug, "bots-sidebar-states" | "bots-compact-states") {
                use serde_json::json;
                let client = BoxClient::new(stand_in.clone());
                for (id, color, shape, face) in [
                    ("bot_mira", "sky", "round", "happy"),
                    ("bot_inbox", "coral", "squircle", "curious"),
                    ("bot_release", "lime", "pebble", "wink"),
                    ("bot_docs", "violet", "gem", "calm"),
                ] {
                    let _ = client.change_bot(id, &kuvo_box::bots::BotChange {
                        look: Some(kuvo_box::areas::Look { color: color.into(), shape: shape.into(), face: face.into() }),
                        ..Default::default()
                    });
                }
                stand_in.set_bot_now("bot_mira", json!({ "working": 0, "asking": 1, "state": "needsYou", "since": NOW - 60_000. }));
                stand_in.set_bot_now("bot_inbox", json!({ "working": 1, "asking": 0, "state": "working", "since": NOW - 90_000. }));
                stand_in.set_bot_now("bot_release", json!({ "working": 0, "asking": 0, "state": "idle", "since": NOW - 3_600_000. }));
                stand_in.set_bot_wakes("bot_release", vec![json!({
                    "id": "wake_rel", "due": NOW / 1000. + 3. * 3600., "every": "1d",
                    "summary": "Check Friday beta readiness", "cancelKey": "wake_rel"
                })], Vec::new());
                stand_in.add_bot("bot_ops", "Ops Watcher", json!({ "color": "orange", "shape": "capsule", "face": "bright" }));
                stand_in.set_bot_now("bot_ops", json!({ "working": 0, "asking": 0, "blocked": 1, "blockedBy": "limit", "state": "blocked", "since": NOW - 120_000. }));
            }
            if matches!(screen.slug, "outputs-popover-media" | "outputs-popover-images") {
                use serde_json::json;
                let mini_screen = |dark: bool, accent: [u8; 3], bars: bool| -> Vec<u8> {
                    let (w, h) = (240u32, 152u32);
                    let bg = if dark { [18u8, 19, 22] } else { [243u8, 243, 245] };
                    let side = if dark { [24u8, 25, 29] } else { [248u8, 248, 250] };
                    let card = if dark { [34u8, 36, 42] } else { [255u8, 255, 255] };
                    let line = if dark { [58u8, 62, 70] } else { [222u8, 224, 228] };
                    let img = image::RgbImage::from_fn(w, h, |x, y| {
                        let px = if x < 54 {
                            if x == 53 { line } else if y > 22 && y < 32 && x > 10 && x < 42 { accent } else if (y == 46 || y == 60 || y == 74) && x > 10 && x < 40 { line } else { side }
                        } else if y < 18 {
                            if y == 17 { line } else { side }
                        } else if x > 66 && x < 228 && y > 28 && y < 86 {
                            if x == 67 || x == 227 || y == 29 || y == 85 { line }
                            else if bars && y > 46 && y < 74 && ((x > 80 && x < 98) || (x > 108 && x < 126 && y > 54) || (x > 136 && x < 154 && y > 40) || (x > 164 && x < 182 && y > 58)) { accent }
                            else if !bars && y > 40 && y < 48 && x > 80 && x < 154 { accent }
                            else if !bars && (y == 58 || y == 68) && x > 80 && x < 208 { line }
                            else { card }
                        } else if x > 66 && x < 228 && y > 96 && y < 140 {
                            if x == 67 || x == 227 || y == 97 || y == 139 { line }
                            else if (y == 112 || y == 124) && x > 80 && x < 190 { line }
                            else { card }
                        } else {
                            bg
                        };
                        image::Rgb(px)
                    });
                    let mut out = std::io::Cursor::new(Vec::new());
                    img.write_to(&mut out, image::ImageFormat::Png).expect("mini png");
                    out.into_inner()
                };

                let commands: Vec<_> = [
                    ("npm run dev", 754_000.0),
                    ("npx vitest --watch", 212_000.0),
                    ("tail -f logs/signup.log", 61_000.0),
                ]
                .iter()
                .enumerate()
                .map(|(index, (command, ago))| json!({ "id": format!("sh_{index}"), "command": command, "started": NOW - ago }))
                .collect();
                let subagents = vec![
                    json!({ "session": "ses_sub0", "title": "Check the signup forms", "since": NOW - 90_000.0, "asking": false }),
                    json!({ "session": "ses_sub1", "title": "Run the browser tests", "since": NOW - 45_000.0, "asking": true }),
                    json!({ "session": "ses_sub2", "title": "Review the report", "since": NOW - 20_000.0, "asking": false }),
                ];
                stand_in.set_background("ses_signup", json!({ "commands": commands, "subagents": subagents }), &[]);
                for (index, (provider, model)) in [("codex-cli", "gpt-6"), ("codex-cli", "gpt-6"), ("claude-code", "opus")].iter().enumerate() {
                    stand_in.set_model_state(&format!("ses_sub{index}"), json!({
                        "model": { "providerID": provider, "id": model }, "runs": true
                    }));
                }

                let items: Vec<(&str, &str, &str, &str, Vec<u8>)> = vec![
                    ("art_d01", "morning-report-1118.md", "document", "text/markdown", b"# Morning report".to_vec()),
                    ("art_d02", "screens-insights-overview-light.png", "image", "image/png", mini_screen(false, [10, 122, 255], true)),
                    ("art_d03", "PLAN.md", "document", "text/markdown", b"# Plan".to_vec()),
                    ("art_d04", "screens-insights-overview-dark.png", "image", "image/png", mini_screen(true, [79, 156, 245], true)),
                    ("art_d05", "screens-computer-pairing-waiting-light.png", "image", "image/png", mini_screen(false, [46, 158, 91], false)),
                    ("art_d06", "screens-computer-pairing-waiting-dark.png", "image", "image/png", mini_screen(true, [94, 199, 184], false)),
                    ("art_d07", "sysload.csv", "download", "text/csv", b"ts,load\n1,0.42\n".to_vec()),
                    ("art_d08", "feedback-review-ranking.md", "document", "text/markdown", b"# Ranking".to_vec()),
                    ("art_d09", "screens-usage-rings-light.png", "image", "image/png", mini_screen(false, [242, 115, 92], true)),
                    ("art_d10", "screens-usage-rings-dark.png", "image", "image/png", mini_screen(true, [138, 92, 245], true)),
                    ("art_d11", "machine-scenarios-1118.md", "document", "text/markdown", b"# Scenarios".to_vec()),
                    ("art_d12", "screens-bots-sidebar-light.png", "image", "image/png", mini_screen(false, [245, 153, 61], false)),
                    ("art_d13", "brief-template.md", "document", "text/markdown", b"# Brief".to_vec()),
                    ("art_d14", "slots.json", "code", "application/json", b"{\"slots\":4}".to_vec()),
                ];
                for (id, name, kind, mime, bytes) in items {
                    stand_in.add_output("ses_signup", json!({
                        "id": id, "path": format!("/tmp/{name}"), "name": name,
                        "kind": kind, "type": mime, "latest": { "n": 1 }
                    }), &bytes);
                }
                stand_in.set_queue("ses_signup", vec![]);
            }
            if screen.slug.starts_with("outputs-popover-bot") {
                use serde_json::json;
                stand_in.set_bot_work(
                    "bot_mira",
                    vec![
                        json!({
                            "session": "ses_signup", "title": "Fix the sign-up form wiping on a taken email",
                            "folder": "/Users/you/Projects/website", "project": "/Users/you/Projects/website",
                            "startedBy": "bot_mira", "depth": 1, "state": "working",
                            "started": NOW - 18. * 60_000., "tokens": 48_210,
                            "model": { "providerID": "claude-code", "id": "claude-opus-4-6" },
                            "account": { "name": "Claude Code", "title": "Claude" }
                        }),
                        json!({
                            "session": "ses_cms", "title": "Move the blog to the new CMS",
                            "folder": "/Users/you/Projects/website", "project": "/Users/you/Projects/website",
                            "startedBy": "bot_mira", "depth": 1, "state": "asking",
                            "started": NOW - 42. * 60_000., "tokens": 31_400,
                            "asks": "Wants to run a command: npm run migrate",
                            "model": { "providerID": "codex-cli", "id": "gpt-5.4" },
                            "account": { "name": "Codex", "title": "ChatGPT" }
                        }),
                        json!({
                            "session": "ses_gemini", "title": "Audit mobile viewport spacing on onboarding",
                            "folder": "/Users/you/Projects/website", "project": "/Users/you/Projects/website",
                            "startedBy": "bot_mira", "depth": 1, "state": "working",
                            "started": NOW - 9. * 60_000., "tokens": 19_840,
                            "model": { "providerID": "google", "id": "gemini-3-pro" },
                            "account": { "name": "Gemini", "title": "Google" }
                        }),
                        json!({
                            "session": "ses_dark", "title": "Dark mode for the settings page",
                            "folder": "/Users/you/Projects/website", "project": "/Users/you/Projects/website",
                            "startedBy": "bot_mira", "depth": 1, "state": "finished",
                            "started": NOW - 3. * 3_600_000., "tokens": 27_900,
                            "model": { "providerID": "antigravity", "id": "gemini-3.1-pro" },
                            "account": { "name": "Antigravity", "title": "Antigravity" }
                        }),
                        json!({
                            "session": "ses_lighthouse", "title": "Get the Lighthouse score above 90",
                            "folder": "/Users/you/Projects/website", "project": "/Users/you/Projects/website",
                            "startedBy": "bot_mira", "depth": 1, "state": "stopped",
                            "started": NOW - 73. * 3_600_000., "tokens": 15_200,
                            "model": { "providerID": "codex-cli", "id": "gpt-5.4" },
                            "account": { "name": "Codex", "title": "ChatGPT" }
                        }),
                    ],
                );
                stand_in.set_bot_wakes(
                    "bot_mira",
                    vec![
                        json!({
                            "id": "wk_brief", "due": NOW / 1000. + 25. * 60., "every": "1d",
                            "summary": "Send Friday beta readiness brief", "cancelKey": "wk_brief"
                        }),
                    ],
                    vec![
                        json!({
                            "letters": "ci", "next": NOW / 1000. + 180., "every": "5m",
                            "summary": "Check CI on kuvo/beta35 release branch"
                        }),
                        json!({
                            "letters": "inbox", "next": NOW / 1000. + 420., "every": "10m",
                            "summary": "Watch support queue for new P0 reports"
                        }),
                    ],
                );
            }
            if screen.slug == "outputs-popover-bot-long" {
                let names = ["15:00 branch sweep", "16:00 batch test build", "03:00 branch sweep", "Weekly upstream check", "Nightly docs sync", "Friday beta readiness brief"];
                let wakes = names.iter().enumerate().map(|(ix, name)| serde_json::json!({ "id": format!("wk_{ix}"), "due": NOW / 1000. + 3600. * (ix as f64 + 4.), "every": "1d", "summary": name, "cancelKey": format!("wk_{ix}") })).collect();
                let watches = ["ci", "inbox", "deploys", "errors"].iter().enumerate().map(|(ix, letters)| serde_json::json!({ "letters": letters, "next": NOW / 1000. + 180. * (ix as f64 + 1.), "every": "5m", "summary": format!("Watch {letters}") })).collect();
                stand_in.set_bot_wakes("bot_mira", wakes, watches);
            }
            if screen.slug.starts_with("floating-composer-group") {
                let lines = [
                    ("bot_release", "I’ve moved the pricing copy to Thursday morning so legal has a full day with it."),
                    ("bot_docs", "Release notes are drafted. I left a gap for the sign-up fix until it merges, and linked the two issues it closes."),
                    ("bot_mira", "The taken-email message is in: “That email already has an account. Sign in instead?” with the link going to sign in, the email filled in."),
                    ("bot_release", "Good. Staging is on the release branch now. I’ll tag the candidate once the notes are in."),
                    ("bot_docs", "Notes are in. Two screenshots still show the old header; I’ll take new ones after the candidate is up."),
                    ("bot_mira", "Ran the full suite against the candidate: 412 pass, 0 fail. The slowest is still the import test at 41 seconds."),
                    ("bot_release", "Tagged 2.4.0-rc.1. If nothing turns up by Thursday noon, it ships Friday at ten."),
                    ("bot_docs", "New screenshots are in the notes. That’s everything from me for 2.4."),
                ];
                for (bot, text) in lines {
                    stand_in.group_says("grp_launch", bot, text);
                }
            }
            if screen.slug.starts_with("floating-composer-side") {
                stand_in.answer_side_questions(&[
                    "Querying first leaves a gap: two sign-ups with the same email can both pass the check and then race to the insert. The unique index is the one place that settles it, so the code inserts and treats 23505 as “taken”.",
                    "No. A 409 is a normal answer to a normal request, and counting it would lock someone out for retyping their email. The limiter counts attempts that reach the password check.",
                    "Yes, in validate.ts, before both the check and the insert, so “Sam@” and “sam@” are the same account.",
                    "The button turns into a spinner and the fields are read-only until the answer comes, so a double click doesn’t send twice.",
                    "It does now: signup.test.ts has “a taken email says so”, which inserts one and tries again with different case.",
                    "In messages.ts, beside the other sign-up errors, so the copy review sees all of them in one place.",
                    "No: the insert is the only write, and a retry after a 409 gets the same 409. The welcome email goes out only after the insert succeeds.",
                    "Only the review. The tests pass on staging, and the release notes mention it.",
                ]);
            }
            if screen.slug == "floating-composer-grown" {
                stand_in.set_queue("ses_lighthouse", ["Check the form on mobile", "Add a changelog entry"].iter().enumerate().map(|(index, text)| {
                    serde_json::json!({ "id": format!("msg_shot{index}"), "words": text, "delivery": "queue", "on": "engine", "held": false })
                }).collect());
            }
            if matches!(screen.slug, "composer-queue-two" | "composer-queue-long" | "composer-queue-dragging") {
                use serde_json::json;
                let messages = if screen.slug != "composer-queue-long" {
                    vec!["Check the form on mobile", "Add a changelog entry"]
                } else {
                    vec![
                        "Compare the new signup flow with the existing mobile and desktop versions, and call out any changes to field validation, focus or keyboard navigation.",
                        "Update the accessibility checklist", "Run the browser tests", "Add a changelog entry",
                        "Make a screenshot of the result", "Send the review summary",
                    ]
                };
                stand_in.set_queue("ses_signup", messages.iter().enumerate().map(|(index, text)| {
                    json!({ "id": format!("msg_shot{index}"), "words": text, "delivery": "queue", "on": "engine", "held": false })
                }).collect());
            }
            if screen.slug.starts_with("bars-") && screen.slug != "bars-bot" {
                let many = screen.slug == "bars-many";
                let command = |id: &str, words: &str, ago: f64| serde_json::json!({ "id": id, "command": words, "started": NOW - ago, "folder": "/Users/sam/Projects/website" });
                let subagent = |id: &str, title: &str, asking: bool| serde_json::json!({ "session": id, "since": NOW - 40_000.0, "title": title, "asking": asking });
                let commands = if many { vec![command("sh_dev", "npm run dev", 754_000.0), command("sh_test", "npx vitest --watch", 212_000.0), command("sh_tail", "tail -f logs/signup.log", 61_000.0)] } else { vec![command("sh_dev", "npm run dev", 95_000.0)] };
                let subagents = if many { vec![subagent("ses_sub", "Check the other forms", false), subagent("ses_sub2", "Find where the email check runs", true), subagent("ses_sub3", "Write a test for a taken email", false)] } else { vec![subagent("ses_sub", "Check the other forms", false)] };
                stand_in.set_background("ses_signup", serde_json::json!({ "commands": commands, "subagents": subagents }), &[("sh_dev", "> website@2.4.0 dev\n> vite\n\n  VITE v6.0.3  ready in 412 ms\n\n  ➜  Local:   http://localhost:5173/\n  ➜  Network: use --host to expose")]);
            }
            if screen.slug.starts_with("pictures-") {
                stage_pictures(&stand_in);
            }
            if screen.slug == "stage-viewer-single" {
                stage_single_picture(&stand_in);
            }
            if screen.slug == "show-file-pictures" {
                stage_show_file_pictures(&stand_in);
            }
            if screen.slug == "show-file-cards" {
                stage_show_file_cards(&stand_in);
            }
            if screen.slug == "tool-details" {
                stage_tool_details(&stand_in);
            }
            if screen.slug == "agent-messages" {
                stage_agent_messages(&stand_in);
            }
            if screen.slug == "text-overflow" {
                stage_text_overflow(&stand_in);
            }
            if screen.slug == "work-item-hover" {
                stage_work_item_hover(&stand_in);
            }
            if screen.slug == "reply-items" || screen.slug == "reply-items-hover" {
                stage_reply_items(&stand_in);
            }
            if screen.slug == "mode-chip-held" {
                stand_in.set_approvals(false, Some("The sandbox isn’t running on this computer."));
            }
            match screen.slug {
                "agents-accounts-down" | "agents-accounts-down-narrow" | "agents-usage-down" | "agents-rail-down" => stand_in.agents_down("The coding agents keep stopping. Kuvo tries again in about 5 minutes.", true),
                "agents-accounts-restarting" => stand_in.agents_restarting(),
                _ => {}
            }
            let agents = stand_in.clone();
            let client = BoxClient::new(stand_in);
            let window = cx.open_window(size(px(screen.width), px(screen.height)), |window, cx| cx.new(|cx| Workspace::new(client, false, window, cx)))?;
            let any = window.into();
            cx.update_window(any, |root, window, cx| {
                let workspace = root.downcast::<Workspace>().expect("the workspace");
                workspace.update(cx, |workspace, cx| {
                    workspace.set_now(NOW, cx);
                    workspace.set_sky_hour(12., cx);
                    workspace.read_at_once(cx);
                    if let Some(session) = screen.session {
                        workspace.open_read_at_once(session, window, cx);
                    }
                    if let Some(view) = workspace.conversation() {
                        for (step, n) in screen.opens {
                            view.update(cx, |view, cx| view.open_step(step, *n, cx));
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
                    if screen.slug.starts_with("insights-") {
                        workspace.insights_sample(screen.slug, window, cx);
                    }
                    if screen.slug.starts_with("library-") {
                        workspace.library_sample(screen.slug, NOW, window, cx);
                    }
                    if screen.slug.starts_with("market-") {
                        workspace.marketplace_sample(screen.slug.trim_end_matches("-picker"), window, cx);
                    }
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
            if screen.slug == "panel-browser" {
                // A page's picture: a plain page, as wide as the panel shows it.
                let picture = image::RgbImage::from_pixel(960, 1200, image::Rgb([250, 250, 250]));
                let mut jpeg = Vec::new();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 80).encode_image(&picture).expect("a picture");
                shell.browser_frame(serde_json::json!({ "targetId": "P", "frameId": 1, "width": 480, "height": 600, "scrollX": 0, "scrollY": 0 }), &jpeg);
                cx.run_until_parked();
            }
            if screen.slug == "panel-terminal" {
                shell.pty_print("pty_1", b"\x1b[1;34m~/website\x1b[0m \x1b[2mfix/signup-taken-email\x1b[0m\r\n\xe2\x9d\xaf bun test src/components/SignupForm.test.tsx\r\n\r\n \x1b[32m\xe2\x9c\x93\x1b[0m keeps the email on 409 \x1b[2m9ms\x1b[0m\r\n \x1b[32m\xe2\x9c\x93\x1b[0m focuses the email after the error \x1b[2m6ms\x1b[0m\r\n \x1b[32m\xe2\x9c\x93\x1b[0m submits a new account \x1b[2m14ms\x1b[0m\r\n\r\n \x1b[32m3 pass\x1b[0m \x1b[2m0 fail [42ms]\x1b[0m\r\n\r\n\x1b[1;34m~/website\x1b[0m \x1b[2mfix/signup-taken-email\x1b[0m\r\n\xe2\x9d\xaf ");
                cx.run_until_parked();
            }
            if screen.slug == "market-custom" || screen.slug == "market-new-skill" {
                cx.update_window(any, |root, window, cx| {
                    let workspace = root.downcast::<Workspace>().expect("the workspace");
                    workspace.read(cx).marketplace().clone().update(cx, |view, cx| view.open_sample_form(screen.slug, window, cx));
                })?;
                cx.run_until_parked();
            }
            if screen.slug == "library-menu" {
                cx.update_window(any, |root, window, cx| {
                    let workspace = root.downcast::<Workspace>().expect("the workspace");
                    workspace.read(cx).library().clone().update(cx, |view, cx| view.open_sample_menu(window, cx));
                })?;
                cx.run_until_parked();
            }
            if screen.slug == "composer-queue-dragging" {
                let to = cx.update_window(any, |root, window, cx| {
                    let workspace = root.downcast::<Workspace>().expect("the workspace");
                    let conversation = workspace.read(cx).conversation().expect("the conversation");
                    let (from, to) = conversation.read(cx).queue_drag_points();
                    let button = gpui::MouseButton::Left;
                    mouse(window, cx, gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                        button, position: from, click_count: 1, ..Default::default()
                    }));
                    mouse(window, cx, gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                        position: from + gpui::point(px(0.), px(8.)), pressed_button: Some(button), ..Default::default()
                    }));
                    assert!(cx.has_active_drag(), "the pointer missed the queue row");
                    to + gpui::point(px(12.), px(8.))
                })?;
                cx.run_until_parked();
                cx.update_window(any, |_, window, cx| {
                    mouse(window, cx, gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                        position: to, pressed_button: Some(gpui::MouseButton::Left), ..Default::default()
                    }));
                    window.refresh();
                })?;
                cx.run_until_parked();
                cx.update_window(any, |_, window, _| window.refresh())?;
                cx.run_until_parked();
            }
            // Restarting after the rings and limits were read: they're kept, marked as updating.
            if matches!(screen.slug, "agents-rail-restarting" | "agents-usage-restarting") {
                agents.agents_restarting();
                cx.update_window(any, |root, window, cx| {
                    let workspace = root.downcast::<Workspace>().expect("the workspace");
                    workspace.update(cx, |workspace, cx| {
                        workspace.rings().clone().update(cx, |rings, cx| {
                            rings.read_at_once(cx);
                            rings.show_card(0, cx);
                        });
                        // (Another section, then Usage again: it's read again as it opens.)
                        if screen.slug == "agents-usage-restarting" {
                            settings(workspace, "Accounts", window, cx);
                            settings(workspace, "Usage", window, cx);
                        }
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
            if matches!(screen.slug, "bot-edit-scrolled" | "bot-edit-avatar-hover" | "bot-edit-personality-picker") {
                cx.update_window(any, |root, window, cx| {
                    let workspace = root.downcast::<Workspace>().expect("the workspace");
                    let page = bots_page(workspace.read(cx), cx);
                    if screen.slug == "bot-edit-scrolled" {
                        page.read(cx).scroll_to_end();
                    } else {
                        use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, PlatformInput};
                        if screen.slug == "bot-edit-avatar-hover" {
                            let position = page.read(cx).avatar_center().expect("the avatar is visible");
                            mouse(window, cx, PlatformInput::MouseMove(MouseMoveEvent { position, pressed_button: None, modifiers: Modifiers::default() }));
                        } else {
                            let position = page.read(cx).personality_menu_center().expect("the personality picker is visible");
                            mouse(window, cx, PlatformInput::MouseDown(MouseDownEvent { position, button: MouseButton::Left, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }));
                        }
                    }
                    window.refresh();
                })?;
                cx.run_until_parked();
            }
            if screen.slug == "small-1123-b-1-add-project" {
                use gpui::{Modifiers, MouseMoveEvent, PlatformInput};
                // On the Projects heading, where its + shows.
                cx.update_window(any, |_, window, cx| mouse(window, cx, PlatformInput::MouseMove(MouseMoveEvent {
                    position: gpui::point(px(150.), px(PROJECTS_Y)), pressed_button: None, modifiers: Modifiers::default(),
                })))?;
                cx.run_until_parked();
            }
            if screen.slug == "bot-wake-badge-hover" {
                use gpui::{Modifiers, MouseMoveEvent, PlatformInput};
                // Mira's avatar, the first bot in this fixture.
                cx.update_window(any, |_, window, cx| mouse(window, cx, PlatformInput::MouseMove(MouseMoveEvent {
                    position: gpui::point(px(83.), px(118.)), pressed_button: None, modifiers: Modifiers::default(),
                })))?;
                cx.run_until_parked();
                cx.advance_clock(std::time::Duration::from_millis(550));
                cx.run_until_parked();
            }
            // A composer's menu opened with a press on its chip, through the window.
            if let Some(at) = match screen.slug {
                "landing-worktree-menu" => Some(gpui::point(px(835.), px(412.))),
                // Settings › Models: the model for new sessions, in the one picker (#975).
                "settings-a-models-picker" => Some(gpui::point(px(1056.), px(196.))),
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
            // Read up a little: the way back to the end shows over the composer.
            // Add Agent's model, picked in the one picker (#975), once its options are read.
            if screen.slug == "market-add-agent-picker" {
                cx.update_window(any, |root, window, cx| {
                    let workspace = root.downcast::<Workspace>().expect("the workspace");
                    let market = workspace.read(cx).marketplace().clone();
                    market.update(cx, |market, cx| market.open_agent_model_picker(window, cx));
                    window.refresh();
                })?;
                cx.run_until_parked();
                cx.update_window(any, |_, window, _| window.refresh())?;
                cx.run_until_parked();
            }
            if screen.slug == "floating-composer-group-scrolled" {
                cx.update_window(any, |root, window, cx| {
                    let workspace = root.downcast::<Workspace>().expect("the workspace");
                    workspace.read(cx).area_view(Area::Bots).and_then(|view| view.read(cx).groups().cloned()).expect("Bots").read(cx).chat("grp_launch").expect("the chat").read(cx).list().scroll_by(px(-360.));
                    window.refresh();
                })?;
                cx.run_until_parked();
                cx.update_window(any, |_, window, _| window.refresh())?;
                cx.run_until_parked();
            }
            if screen.slug == "floating-composer-side-scrolled" {
                cx.update_window(any, |root, window, cx| {
                    let workspace = root.downcast::<Workspace>().expect("the workspace");
                    let scroll = workspace.read(cx).panel().read(cx).side_chat().expect("the side chat").read(cx).scroll().clone();
                    let offset = scroll.offset();
                    scroll.set_offset(gpui::point(offset.x, offset.y + px(360.)));
                    window.refresh();
                })?;
                cx.run_until_parked();
                cx.update_window(any, |_, window, _| window.refresh())?;
                cx.run_until_parked();
            }
            if screen.slug == "floating-composer-scrolled" {
                cx.update_window(any, |root, window, cx| {
                    let workspace = root.downcast::<Workspace>().expect("the workspace");
                    workspace.read(cx).conversation().expect("the conversation").read(cx).list().scroll_by(px(-360.));
                    window.refresh();
                })?;
                cx.run_until_parked();
                cx.update_window(any, |_, window, _| window.refresh())?;
                cx.run_until_parked();
            }
            if screen.slug.starts_with("select-drag-") {
                cx.update_window(any, |root, window, cx| {
                    let workspace = root.downcast::<Workspace>().expect("the workspace");
                    let workspace = workspace.read(cx);
                    let view = workspace.conversation().expect("the conversation").read(cx);
                    let anchor = spot_of(workspace, "Moving", false, cx);
                    let top = view.list().list_state().viewport_bounds().top();
                    // Keep the anchor near the top so the later drag endpoints are drawn too.
                    let scroll = anchor.y - top - px(48.);
                    if scroll > px(0.) {
                        view.list().scroll_by(scroll);
                        window.refresh();
                    }
                })?;
                cx.run_until_parked();
            }
            // A right-click on the selection, a row's session menu, or a drag that's under way, through the window.
            if screen.slug == "select-menu" || screen.slug.starts_with("message-hover-") || screen.slug == "select-finished" || screen.slug.starts_with("select-drag-") || screen.slug == "session-menu-row-menu" || screen.slug == "session-menu-held" {
                use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, PlatformInput};
                cx.update_window(any, |root, window, cx| {
                    let workspace = root.downcast::<Workspace>().expect("the workspace");
                    if screen.slug == "session-menu-row-menu" || screen.slug == "session-menu-held" {
                        let at = gpui::point(px(190.), px(338.));
                        mouse(window, cx, PlatformInput::MouseMove(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() }));
                        mouse(window, cx, PlatformInput::MouseDown(MouseDownEvent { position: at, button: MouseButton::Right, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }));
                        assert!(workspace.read(cx).session_menu().row_menus.get("session-project:ses_dark").is_some_and(|h| h.is_open()), "right-click at {at:?} missed ses_dark");
                    } else if screen.slug.starts_with("message-hover-") {
                        // The pointer over a message of yours, or of the agent's: its time, Copy and Quote show.
                        let at = spot_of(workspace.read(cx), if screen.slug == "message-hover-yours" { "Keep what they typed" } else { "success path" }, false, cx);
                        mouse(window, cx, PlatformInput::MouseMove(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() }));
                    } else if screen.slug == "select-finished" {
                        // A drag finished: the button's up, and Add to Chat shows over the selection's end.
                        use gpui::MouseUpEvent;
                        let from = spot_of(workspace.read(cx), "the reset into the success path", false, cx);
                        let to = spot_of(workspace.read(cx), "sets the error on", true, cx);
                        mouse(window, cx, PlatformInput::MouseMove(MouseMoveEvent { position: from, pressed_button: None, modifiers: Modifiers::default() }));
                        mouse(window, cx, PlatformInput::MouseDown(MouseDownEvent { position: from, button: MouseButton::Left, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }));
                        mouse(window, cx, PlatformInput::MouseMove(MouseMoveEvent { position: to, pressed_button: Some(MouseButton::Left), modifiers: Modifiers::default() }));
                        mouse(window, cx, PlatformInput::MouseUp(MouseUpEvent { position: to, button: MouseButton::Left, modifiers: Modifiers::default(), click_count: 1 }));
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
            if screen.slug == "queue-loading" {
                std::thread::sleep(std::time::Duration::from_millis(250));
                cx.update_window(any, |_, window, _| window.refresh())?;
                cx.run_until_parked();
            }
            // A menu or card that just opened is shown fully in, not on the first frame of its
            // entry motion: its clock is the executor's, moved on by just the motion's length.
            cx.advance_clock(kuvo_ui::motion::MotionDuration::Fast.duration());
            cx.update_window(any, |_, window, _| window.refresh())?;
            cx.run_until_parked();
            if screen.slug == "queue-new-picture" {
                use gpui::{Modifiers, MouseMoveEvent, PlatformInput};
                cx.update_window(any, |_, window, cx| {
                    mouse(window, cx, PlatformInput::MouseMove(MouseMoveEvent {
                        position: gpui::point(px(800.), px(300.)), pressed_button: None, modifiers: Modifiers::default(),
                    }));
                })?;
                cx.run_until_parked();
            }
            use kuvo::queue::{ShotHoverPoint, ShotHoverTarget};
            let hover = match screen.slug {
                "queue-item-hover-card" => Some((ShotHoverTarget::Reference(234), 320)),
                "work-item-hover" => Some((ShotHoverTarget::Reference(840), 320)),
                "reply-items-hover" => Some((ShotHoverTarget::Reference(954), 320)),
                "queue-item-status-tooltip" => Some((ShotHoverTarget::ItemStatus, 520)),
                _ => None,
            };
            if let Some((target, delay_ms)) = hover {
                use gpui::{Modifiers, MouseMoveEvent, PlatformInput};
                cx.update(|cx| cx.set_global(ShotHoverPoint { target, point: Default::default() }));
                cx.update_window(any, |_, window, _| window.refresh())?;
                cx.run_until_parked();
                cx.update_window(any, |_, window, cx| {
                    let point = cx.global::<ShotHoverPoint>().point.get().expect("the Work Queue hover target");
                    mouse(window, cx, PlatformInput::MouseMove(MouseMoveEvent {
                        position: point, pressed_button: None, modifiers: Modifiers::default(),
                    }));
                })?;
                cx.run_until_parked();
                cx.advance_clock(std::time::Duration::from_millis(delay_ms));
                cx.update_window(any, |_, window, _| window.refresh())?;
                cx.run_until_parked();
                cx.advance_clock(kuvo_ui::motion::MotionDuration::Fast.duration());
                cx.update_window(any, |_, window, _| window.refresh())?;
                cx.run_until_parked();
                cx.update(|cx| { cx.remove_global::<ShotHoverPoint>(); });
            }
            let image = cx.capture_screenshot(any)?;
            let name = format!("screens-{}-{}.png", screen.slug, if appearance.is_dark() { "dark" } else { "light" });
            image.save(out.join(&name))?;
            println!("{name}: {}x{}", image.width(), image.height());
            cx.update_window(any, |_, window, cx| {
                if screen.slug == "composer-queue-dragging" {
                    cx.stop_active_drag(window);
                }
                window.remove_window();
            })?;
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
        "launch-reinstall", "launch-setting-up", "launch-setting-up-step", "launch-starting", "launch-done", "launch-failed", "launch-failed-narrow", "launch-not-running",
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
                    "launch-reinstall" => launch.show(Stage::ReinstallOffered, false, cx),
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
                        stand_in.set_permissions(|permissions| *permissions = Permissions { screen_recording: true, accessibility: true, ..Permissions::nothing_allowed() });
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

/// The window over your boxes: a session open on another of them (`elsewhere`), the first Quit's note (`quit-note`), and another computer
/// whose Kuvo is older than this app, its update chip's card open: waiting a long while, with
/// Update Now (`update-waiting`), and being updated (`update-running`); one the box that lists it
/// sets up again (`update-setup`), and one nothing here reaches over SSH (`update-ssh`). And this
/// Mac's own Kuvo waiting for the release this app carries (`update-own-waiting`), and taking it
/// (`update-own-running`).
fn home_shots(cx: &mut HeadlessAppContext, only: &[String], out: &std::path::Path) -> anyhow::Result<()> {
    // (And #772's: an older machine's page with its one card, waiting with Update Now, failing with
    // Try Again, under way, and offered by the box that lists it; a computer that can't be used,
    // older or out of reach, as the window shows it.)
    for slug in ["elsewhere", "quit-note", "update-waiting", "update-running", "update-setup", "update-ssh", "update-own-waiting", "update-own-running", "page-update-waiting", "page-update-failed", "page-update-running", "page-older-offer", "blocked-held", "blocked-unreached", "computer-pairing-waiting", "computer-pairing-readd", "bots-background-offline", "insights-needs-update", "bots-remote-wake", "settings-mac-machine", "fleet-header-updating", "fleet-header-failed", "fleet-header-done", "fleet-overview", "fleet-overview-failed", "fleet-detail-behind", "fleet-detail-failed", "fleet-phase-1", "fleet-phase-2", "fleet-phase-3", "fleet-phase-4", "fleet-phase-5", "fleet-phase-6"] {
        if !only.is_empty() && !only.iter().any(|only| only == slug) {
            continue;
        }
        for appearance in [Appearance::Light, Appearance::Dark] {
            cx.update(|cx| Theme::set_appearance(appearance, cx));
            let stand_in = StandIn::at(Replies::Never, NOW);
            let own = slug.starts_with("update-own-");
            if own {
                stand_in.own_update_waits(if slug == "update-own-running" { "updating" } else { "waiting" }, 2);
            } else if let Some(state) = slug.strip_prefix("page-update-") {
                stand_in.bring_up_to_date("key_devbox", if state == "running" { "updating" } else { state }, false, 2);
            } else if slug == "page-older-offer" {
                // (Shown from devbox's window, whose box can't bring old-mini up: the list's can.)
                stand_in.peer_box("key_devbox").expect("devbox").cant_bring_up("key_old", "Add computers from a Mac for now: on Linux, Kuvo Code can’t yet keep the list of your computers.");
                stand_in.bring_up_to_date("key_old", "waiting", false, 0);
            } else if slug == "blocked-held" {
                stand_in.peer_box("key_devbox").expect("devbox").set_older(true);
                stand_in.bring_up_to_date("key_devbox", "waiting", false, 0);
            } else if slug == "insights-needs-update" {
                stand_in.peer_box("key_devbox").expect("devbox").set_lacks(&["usage.insights"]);
                stand_in.bring_up_to_date("key_devbox", "waiting", false, 0);
            } else if slug == "settings-mac-machine" {
                stand_in.set_computer_system("key_devbox", Some("macOS 15.1"));
            } else if slug.starts_with("update-") {
                stand_in.peer_box("key_devbox").expect("devbox").set_older(true);
                if slug == "update-setup" {
                    stand_in.sets_up_again("key_devbox");
                } else if slug != "update-ssh" {
                    stand_in.bring_up_to_date("key_devbox", if slug == "update-running" { "updating" } else { "waiting" }, slug == "update-waiting", 2);
                }
            }
            // The setup updating to beta.38 (#848's notifier and Computers' update states).
            if let Some(phase) = slug.strip_prefix("fleet-phase-") {
                // (2026-11-23, option A: each of an update's phases, its card open.)
                stand_in.fleet_phase(phase.parse::<usize>().unwrap_or(1) - 1);
            } else if let Some(fleet) = slug.strip_prefix("fleet-") {
                stand_in.fleet_sample(fleet.ends_with("failed"), fleet == "header-done", NOW);
            }
            let devbox = stand_in.peer_box("key_devbox").expect("devbox");
            if slug == "bots-background-offline" {
                devbox.add_bot("bot_scout", "Scout", serde_json::json!({ "color": "mint", "shape": "round", "face": "calm" }));
                devbox.set_bot_now("bot_scout", serde_json::json!({ "working": 1, "asking": 0, "state": "working", "since": NOW }));
            }
            if slug == "bots-remote-wake" {
                stand_in.move_bot("bot_mira", "devbox");
                devbox.add_bot("bot_mira", "VPS Manager", serde_json::json!({ "color": "mint", "shape": "round", "face": "calm" }));
                devbox.set_bot_wakes("bot_mira", vec![serde_json::json!({
                    "id": "at-check", "due": NOW / 1000. + 180., "summary": "Check VPS health", "cancelKey": "check",
                })], vec![]);
            }
            use kuvo_box::boxes::Pairing;
            let pairing = match slug {
                "computer-pairing-waiting" => Some(Pairing::Checking("Checking this app with your other computers…".into())),
                "computer-pairing-readd" => Some(Pairing::Readd(kuvo_box::Refusal::from_answer(409, &serde_json::to_vec(&serde_json::json!({
                    "reason": "pairing-readd",
                    "title": "This app needs to be paired again",
                    "message": "The computer that added this app was removed. Pair this app again from one of your computers.",
                    "action": { "label": "Pair Again…", "route": "kuvo/devices/pair" },
                }))?))),
                _ => None,
            };
            let boxes = kuvo::home::stand_in_boxes(stand_in);
            let first = boxes.nearest().expect("the stand-in is the nearest box");
            let window = cx.open_window(size(px(1280.), px(820.)), |window, cx| cx.new(|cx| kuvo::Home::new(boxes, first, false, window, cx)))?;
            let any = window.into();
            cx.update_window(any, |root, window, cx| {
                let home = root.downcast::<kuvo::Home>().expect("the home");
                home.update(cx, |home, cx| {
                    home.set_now(NOW, cx);
                    home.read_at_once(cx);
                    if let Some(pairing) = pairing.clone() {
                        home.workspace().clone().update(cx, |workspace, cx| {
                            workspace.show_area(Area::Computers, window, cx);
                            let mut switching = workspace.switching().cloned().expect("stand-in computers were read");
                            switching.pairing = pairing;
                            workspace.set_switching(Some(switching), cx);
                        });
                    }
                    if slug == "bots-background-offline" {
                        let nearest = home.workspace().clone();
                        nearest.update(cx, |workspace, cx| workspace.show_area(Area::Bots, window, cx));
                        assert!(home.switch_to("key_devbox", window, cx));
                        home.workspace().clone().update(cx, |workspace, cx| workspace.show_area(Area::Bots, window, cx));
                        nearest.update(cx, |workspace, cx| workspace.connected(Err(kuvo_box::Error::NotRunning("offline".into())), cx));
                    }
                    if slug == "bots-remote-wake" {
                        home.workspace().clone().update(cx, |workspace, cx| workspace.show_area(Area::Bots, window, cx));
                    }
                    if slug == "settings-mac-machine" {
                        home.workspace().clone().update(cx, |workspace, cx| {
                            workspace.show_setting(&kuvo::settings::box_key("key_devbox", "accounts"), window, cx);
                        });
                    }
                    if slug == "insights-needs-update" {
                        assert!(home.switch_to("key_devbox", window, cx));
                        home.workspace().clone().update(cx, |workspace, cx| {
                            workspace.open_insights(
                                Some(kuvo::area::UsageFilter { title: "Claude".into(), provider: "claude".into(), account: None }),
                                window,
                                cx,
                            );
                        });
                    }
                    if slug == "elsewhere" {
                        assert!(home.switch_to("key_devbox", window, cx));
                        home.workspace().clone().update(cx, |workspace, cx| workspace.open_read_at_once("ses_releases", window, cx));
                    }
                    if slug == "page-older-offer" {
                        assert!(home.switch_to("key_devbox", window, cx));
                        home.workspace().clone().update(cx, |workspace, cx| workspace.show_computer("key_old", None, window, cx));
                    } else if slug.starts_with("page-") {
                        home.workspace().clone().update(cx, |workspace, cx| workspace.show_computer("key_devbox", None, window, cx));
                    }
                    if slug.starts_with("blocked-") {
                        assert!(home.switch_to("key_devbox", window, cx));
                    }
                    if slug == "blocked-unreached" {
                        devbox.set_down(true);
                        home.workspace().clone().update(cx, |workspace, cx| workspace.reach_again(window, cx));
                    }
                    if slug.starts_with("update-") {
                        if !own {
                            assert!(home.switch_to("key_devbox", window, cx));
                        }
                        home.workspace().clone().update(cx, |workspace, cx| workspace.toggle_update_card(cx));
                    }
                    match slug {
                        "fleet-header-updating" | "fleet-header-failed" => home.workspace().clone().update(cx, |workspace, cx| workspace.toggle_fleet_card(cx)),
                        _ if slug.starts_with("fleet-phase-") => home.workspace().clone().update(cx, |workspace, cx| workspace.toggle_fleet_card(cx)),
                        "fleet-overview" | "fleet-overview-failed" => home.workspace().clone().update(cx, |workspace, cx| workspace.show_area(Area::Computers, window, cx)),
                        "fleet-detail-behind" => home.workspace().clone().update(cx, |workspace, cx| workspace.show_computer("key_more_0", None, window, cx)),
                        "fleet-detail-failed" => home.workspace().clone().update(cx, |workspace, cx| workspace.show_computer("key_devbox", None, window, cx)),
                        _ => {}
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
            if slug.starts_with("fleet-header-") || slug.starts_with("fleet-phase-") {
                // (The notifier's card, its entry motion run to its end.)
                cx.advance_clock(kuvo_ui::motion::MotionDuration::Fast.duration());
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
