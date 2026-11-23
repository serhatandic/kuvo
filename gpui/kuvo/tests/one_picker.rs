//! The one model picker, laid out as T3 Code's (#975), as someone uses it against the stand-in box
//! (nothing real is reached): the composer's one chip says the model and its effort; the picker
//! opens with its providers, the provider's models and Reasoning side by side; a search finds
//! across providers; ⌘2 picks the second; ☆ keeps a model in ★ through the box; Standard | Fast
//! and the permissions sit along its bottom, set on the box; Marketplace › Add Agent opens the same
//! picker; and Send stays inside the composer however narrow it gets. Clicks and keys go through
//! the window as AppKit hands them on.

use gpui::{Bounds, Entity, Focusable as _, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::stand_in::marketplace::Sample;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme};
use serde_json::{Value, json};

/// 2026-11-14, 09:53 UTC.
const NOW: f64 = 1_794_650_000_000.0;

fn window(cx: &mut TestAppContext, stand_in: StandIn, width: f32) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(stand_in);
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(width), px(1000.)));
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
    });
    cx.run_until_parked();
    (workspace, cx)
}

/// A conversation open, on Claude Opus 5.5.
fn conversation(cx: &mut TestAppContext, stand_in: StandIn) -> (Entity<Workspace>, &mut VisualTestContext) {
    let (workspace, cx) = window(cx, stand_in, 1280.);
    workspace.update_in(cx, |workspace, window, cx| workspace.open_session_now("ses_signup", window, cx));
    cx.run_until_parked();
    (workspace, cx)
}

fn bounds(cx: &mut VisualTestContext, selector: &str) -> Option<Bounds<Pixels>> {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
}

fn shown(cx: &mut VisualTestContext, selector: &str) -> bool {
    bounds(cx, selector).is_some()
}

fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} isn't on screen"));
    cx.simulate_click(at.center(), Modifiers::default());
    cx.run_until_parked();
}

/// The models the conversation was put on, in order.
fn picked(stand_in: &StandIn) -> Vec<Value> {
    stand_in.seen_at("POST", "api/session/ses_signup/model").into_iter().map(|seen| seen.body.unwrap_or(Value::Null)["model"].clone()).collect()
}

#[gpui::test]
fn the_chip_says_the_model_and_its_effort_and_an_effort_picked_in_reasoning_goes_to_the_box(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_, cx) = conversation(cx, stand_in.clone());
    click(cx, "model-chip");
    click(cx, "model-effort-low");
    assert!(!shown(cx, "model-picker"), "picking an effort closes it");
    assert_eq!(picked(&stand_in), vec![json!({ "providerID": "anthropic", "id": "claude-opus-5-5", "variant": "low" })]);
    // One chip: the model and its effort, with no mode or effort chips beside it.
    assert!(shown(cx, "model-chip-name-Claude Opus 5.5") && shown(cx, "model-chip-effort-Low"));
    assert!(!shown(cx, "effort-chip") && !shown(cx, "mode-chip"));
}

#[gpui::test]
fn it_opens_with_the_providers_the_models_and_reasoning_side_by_side(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.offers_codex();
    // (A new conversation: every side's models; one open offers its own side's.)
    let (_, cx) = window(cx, stand_in.clone(), 1280.);
    click(cx, "model-chip");
    let rail = bounds(cx, "model-rail-anthropic").expect("the providers, down the left");
    let row = bounds(cx, "model-row-anthropic/claude-opus-5-5").expect("the provider's models, in the middle");
    let reasoning = bounds(cx, "model-reasoning").expect("Reasoning, on the right");
    assert!(rail.right() <= row.left() && row.right() <= reasoning.left(), "{rail:?}, {row:?}, {reasoning:?}");
    // ★ at the top of the providers, then each provider's mark; the current model checked.
    assert!(bounds(cx, "model-rail-starred").unwrap().top() < rail.top());
    assert!(shown(cx, "model-rail-codex") && shown(cx, "model-current"));
    // The current model's efforts, as the box lists them.
    for effort in ["low", "medium", "high", "max"] {
        assert!(shown(cx, &format!("model-effort-{effort}")), "{effort}");
    }
    assert!(!shown(cx, "model-effort-xhigh"));
}

#[gpui::test]
fn a_search_finds_models_across_every_provider(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.offers_codex();
    // (A new conversation: every side's models; one open offers its own side's.)
    let (_, cx) = window(cx, stand_in.clone(), 1280.);
    click(cx, "model-chip");
    assert!(!shown(cx, "model-row-openai/gpt-6"), "Anthropic's shown first");
    cx.simulate_input("gpt 6");
    cx.run_until_parked();
    // OpenAI's through the engine, and Codex's, together.
    assert!(shown(cx, "model-row-openai/gpt-6") && shown(cx, "model-row-codex-cli/gpt-6-luna"));
    assert!(!shown(cx, "model-row-anthropic/claude-opus-5-5"));
}

#[gpui::test]
fn command_2_picks_the_second_of_the_providers_models(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.offers_codex();
    let (workspace, cx) = window(cx, stand_in.clone(), 1280.);
    click(cx, "model-chip");
    click(cx, "model-rail-codex");
    // Codex's first seven, then "Show more" for the rest.
    assert!(shown(cx, "model-show-more") && !shown(cx, "model-row-codex-cli/gpt-5.5"));
    cx.simulate_keystrokes("cmd-2");
    cx.run_until_parked();
    assert!(!shown(cx, "model-picker"));
    let landing = workspace.read_with(cx, |workspace, _| workspace.landing().clone());
    let model = landing.read_with(cx, |composer, _| composer.model()).expect("a model picked");
    assert_eq!((model.provider_id.as_str(), model.id.as_str()), ("codex-cli", "gpt-6.2-astra"));
    assert!(shown(cx, "model-chip-name-GPT-6.2-Astra"));
}

#[gpui::test]
fn a_model_starred_from_a_search_is_kept_in_favourites_by_the_box(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.offers_codex();
    // (A new conversation: every side's models; one open offers its own side's.)
    let (_, cx) = window(cx, stand_in.clone(), 1280.);
    click(cx, "model-chip");
    cx.simulate_input("luna");
    cx.run_until_parked();
    click(cx, "model-star-codex-cli/gpt-6-luna");
    assert!(shown(cx, "model-picker"), "starring isn't picking");
    assert_eq!(stand_in.starred(), vec![("codex-cli".to_owned(), "gpt-6-luna".to_owned())]);
    cx.simulate_keystrokes("cmd-a backspace");
    click(cx, "model-rail-starred");
    assert!(shown(cx, "model-row-codex-cli/gpt-6-luna") && !shown(cx, "model-row-anthropic/claude-opus-5-5"));
    assert!(stand_in.seen_at("POST", "kuvo/sessions").is_empty(), "nothing started");
}

#[gpui::test]
fn fast_is_offered_where_the_provider_has_it_and_kept_by_the_box(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.offers_codex();
    let (_, cx) = window(cx, stand_in.clone(), 1280.);
    // Claude Opus 5.5 through the engine has no fast mode: no Standard | Fast.
    click(cx, "model-chip");
    assert!(shown(cx, "model-permissions") && !shown(cx, "model-speed"));
    click(cx, "model-rail-codex");
    click(cx, "model-row-codex-cli/gpt-6-luna");
    click(cx, "model-chip");
    assert!(shown(cx, "model-speed-standard"));
    click(cx, "model-speed-priority");
    assert!(shown(cx, "model-picker"), "a speed isn't a pick");
    assert_eq!(stand_in.speed_of("codex-cli").as_deref(), Some("priority"));
    click(cx, "model-speed-standard");
    assert_eq!(stand_in.speed_of("codex-cli"), None);
}

#[gpui::test]
fn the_permissions_show_the_boxs_words_and_set_the_conversations_mode(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_approvals(true, None);
    let (_, cx) = conversation(cx, stand_in.clone());
    click(cx, "model-chip");
    click(cx, "model-permissions");
    assert!(shown(cx, "model-permission-title-Auto") && shown(cx, "model-permission-detail-Works on its own inside the sandbox; asks before anything beyond it."));
    click(cx, "model-permission-acceptEdits");
    assert_eq!(stand_in.approval_of("ses_signup").as_deref(), Some("acceptEdits"));
    assert!(!shown(cx, "model-permissions-popover") && shown(cx, "model-picker"), "the popover closes; the picker stays");
    click(cx, "model-permissions");
    assert!(shown(cx, "model-permission-title-Accept Edits"));
}

#[gpui::test]
fn add_agent_picks_its_model_in_the_same_picker(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.marketplace_sample(Sample::Fresh);
    let (_, cx) = window(cx, stand_in.clone(), 1320.);
    cx.simulate_resize(size(px(1320.), px(1600.)));
    cx.run_until_parked();
    click(cx, "rail-marketplace");
    click(cx, "market-section-agents");
    click(cx, "market-button-coding-reviewer");
    click(cx, "market-model");
    assert!(shown(cx, "model-picker") && shown(cx, "model-rail-anthropic"), "the one picker, not a list");
    // An agent's model takes no effort here, and it has no permissions or speed.
    assert!(!shown(cx, "model-reasoning") && !shown(cx, "model-picker-bar"));
    click(cx, "model-rail-openai");
    click(cx, "model-row-openai/gpt-6");
    click(cx, "market-add-agent");
    let sent: Vec<Value> = stand_in.seen_at("POST", "kuvo/marketplace/agents/coding-reviewer/add").into_iter().filter_map(|seen| seen.body).collect();
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0]["model"]["providerID"].clone(), sent[0]["model"]["id"].clone()), (json!("openai"), json!("gpt-6")));
}

/// Send stays inside the composer at every width: the chip gives up its room first.
#[gpui::test]
fn send_stays_inside_the_composer_however_narrow(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.offers_codex();
    stand_in.offers_a_long_name();
    let (workspace, cx) = conversation(cx, stand_in.clone());
    // A model with a long name, at an effort.
    click(cx, "model-chip");
    cx.simulate_input("qwen 72b");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    click(cx, "model-chip");
    click(cx, "model-effort-xhigh");
    assert!(shown(cx, "model-chip-effort-Extra"));
    // As the user saw it: the agent at work, more typed (Send's ⌄ beside it), the side panel open.
    let field = workspace.read_with(cx, |workspace, cx| workspace.conversation().unwrap().read(cx).composer().read(cx).focus_handle(cx));
    cx.update(|window, cx| window.focus(&field, cx));
    cx.simulate_input("hello");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_input("where is the browser option?");
    workspace.update_in(cx, |workspace, _, cx| workspace.toggle_panel(cx));
    cx.run_until_parked();
    assert!(shown(cx, "send-choices"), "the agent's at work");
    let mut narrowest = f32::MAX;
    for width in [1280., 1000., 860., 760., 680., 620., 560., 520., 480.] {
        cx.simulate_resize(size(px(width), px(900.)));
        cx.run_until_parked();
        let card = bounds(cx, "composer-card").expect("the composer");
        let send = bounds(cx, "send").expect("Send");
        assert!(send.right() <= card.right() && send.left() >= card.left(), "at {width}: Send {send:?} outside {card:?}");
        narrowest = narrowest.min(f32::from(card.size.width));
    }
    assert!(narrowest <= 320., "the composer got as narrow as {narrowest}");
    // (The model's name gave up its room, not Send.)
    assert!(bounds(cx, "model-chip").unwrap().right() <= bounds(cx, "send").unwrap().left());
    // A new conversation's row has more besides (where it runs, Local or Worktree).
    workspace.update_in(cx, |workspace, window, cx| workspace.new_session(None, window, cx));
    cx.run_until_parked();
    let field = workspace.read_with(cx, |workspace, _| workspace.landing().clone());
    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    cx.simulate_input("hello");
    cx.run_until_parked();
    let card = bounds(cx, "composer-card").expect("the composer");
    let send = bounds(cx, "send").expect("Send");
    assert!(send.right() <= card.right(), "Send {send:?} outside {card:?}");
}
