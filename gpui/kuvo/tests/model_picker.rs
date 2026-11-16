//! The model picker, as someone uses it from a conversation's composer against the stand-in box
//! (nothing real is reached): it opens on the current model's provider, a search finds by name,
//! ⌘1… and Return pick, a star is kept by the box (and shows in Starred), a box that keeps no stars
//! shows none, hundreds of models open and search, and the composer's picks sit on one line with
//! the paperclip and Send. Clicks and keys go through the window as AppKit hands them on.

use gpui::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme};
use serde_json::{Value, json};

/// 2026-11-14, 09:53 UTC.
const NOW: f64 = 1_794_650_000_000.0;

fn conversation(cx: &mut TestAppContext, stand_in: StandIn) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(stand_in);
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(1000.)));
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
    });
    cx.run_until_parked();
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

fn chip(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<String> {
    workspace.read_with(cx, |workspace, cx| workspace.conversation().and_then(|view| view.read(cx).composer().read(cx).chip_label()))
}

#[gpui::test]
fn it_opens_on_the_models_provider_and_a_pick_keeps_the_effort_the_new_one_offers(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = conversation(cx, stand_in.clone());
    assert_eq!(chip(&workspace, cx).as_deref(), Some("Claude Opus 5.5"));
    click(cx, "model-chip");
    assert!(shown(cx, "model-picker"));
    // Anthropic's models, the conversation's own among them; not OpenAI's until its mark is picked.
    assert!(shown(cx, "model-row-anthropic/claude-opus-5-5") && shown(cx, "model-row-anthropic/claude-sonnet-5-5"));
    assert!(!shown(cx, "model-row-openai/gpt-6"));
    click(cx, "model-rail-openai");
    click(cx, "model-row-openai/gpt-6");
    assert!(!shown(cx, "model-picker"), "a pick closes it");
    // At the effort it had (high), which GPT-6 offers too.
    assert_eq!(picked(&stand_in), vec![json!({ "providerID": "openai", "id": "gpt-6", "variant": "high" })]);
    assert_eq!(chip(&workspace, cx).as_deref(), Some("GPT-6"));
}

#[gpui::test]
fn shift_command_m_opens_it_and_what_is_typed_is_found_and_picked_with_return(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_, cx) = conversation(cx, stand_in.clone());
    cx.simulate_keystrokes("cmd-shift-m");
    cx.run_until_parked();
    assert!(shown(cx, "model-picker"));
    // Every provider's, searched as typed: "gem" is Gemini, wherever it is.
    cx.simulate_input("gem");
    cx.run_until_parked();
    assert!(shown(cx, "model-row-google/gemini-3-pro") && !shown(cx, "model-row-anthropic/claude-opus-5-5"));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    // Gemini offers no efforts: none is sent.
    assert_eq!(picked(&stand_in), vec![json!({ "providerID": "google", "id": "gemini-3-pro" })]);
}

#[gpui::test]
fn the_arrows_and_command_numbers_pick_from_the_list_shown_and_escape_closes(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_, cx) = conversation(cx, stand_in.clone());
    click(cx, "model-chip");
    // ⌘2: the second of Anthropic's.
    cx.simulate_keystrokes("cmd-2");
    cx.run_until_parked();
    assert_eq!(picked(&stand_in), vec![json!({ "providerID": "anthropic", "id": "claude-sonnet-5-5", "variant": "high" })]);
    click(cx, "model-chip");
    // Opened on the one now (the second): Up goes to the first, Return picks it.
    cx.simulate_keystrokes("up enter");
    cx.run_until_parked();
    assert_eq!(picked(&stand_in).last(), Some(&json!({ "providerID": "anthropic", "id": "claude-opus-5-5", "variant": "high" })));
    click(cx, "model-chip");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!shown(cx, "model-picker"));
    assert_eq!(picked(&stand_in).len(), 2, "closing picks nothing");
}

#[gpui::test]
fn a_star_is_kept_by_the_box_and_starred_models_show_together(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_, cx) = conversation(cx, stand_in.clone());
    click(cx, "model-chip");
    click(cx, "model-rail-openai");
    click(cx, "model-star-openai/gpt-6");
    assert!(shown(cx, "model-picker"), "starring isn't picking");
    assert_eq!(stand_in.starred(), vec![("openai".to_owned(), "gpt-6".to_owned())]);
    assert!(picked(&stand_in).is_empty());
    click(cx, "model-rail-starred");
    assert!(shown(cx, "model-row-openai/gpt-6") && !shown(cx, "model-row-anthropic/claude-opus-5-5"));
    // Taken away again from Starred: the list says how to star one.
    click(cx, "model-star-openai/gpt-6");
    assert!(stand_in.starred().is_empty());
    assert!(shown(cx, "model-picker-empty"));
}

#[gpui::test]
fn stars_set_on_another_computer_or_client_show_here(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_starred(vec![("google", "gemini-3-pro")]);
    let (_, cx) = conversation(cx, stand_in.clone());
    click(cx, "model-chip");
    click(cx, "model-rail-starred");
    assert!(shown(cx, "model-row-google/gemini-3-pro"));
}

#[gpui::test]
fn a_box_that_keeps_no_stars_shows_none_to_set(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.keeps_no_stars();
    let (_, cx) = conversation(cx, stand_in.clone());
    click(cx, "model-chip");
    assert!(shown(cx, "model-row-anthropic/claude-opus-5-5"));
    assert!(!shown(cx, "model-rail-starred") && !shown(cx, "model-star-anthropic/claude-opus-5-5"));
}

#[gpui::test]
fn hundreds_of_models_open_with_only_those_in_sight_drawn_and_any_is_found(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.more_models(600);
    let (_, cx) = conversation(cx, stand_in.clone());
    click(cx, "model-chip");
    click(cx, "model-rail-openrouter");
    // The first of OpenRouter's 150 drawn, the last not (it's far below, unscrolled).
    assert!(shown(cx, "model-row-openrouter/model-0"));
    assert!(!shown(cx, "model-row-openrouter/model-596"));
    cx.simulate_input("model 596");
    cx.run_until_parked();
    assert!(shown(cx, "model-row-openrouter/model-596"));
    // ⌘1 picks the best match, at the top.
    cx.simulate_keystrokes("cmd-1");
    cx.run_until_parked();
    assert_eq!(picked(&stand_in), vec![json!({ "providerID": "openrouter", "id": "model-596" })]);
}

/// The paperclip, the composer's picks and Send share one centre line (they were drawn at the
/// row's top, Send in the middle).
#[gpui::test]
fn the_composers_bottom_row_is_on_one_line(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_approvals(true, None);
    let (_, cx) = conversation(cx, stand_in.clone());
    cx.simulate_input("hello");
    cx.run_until_parked();
    let middle = |cx: &mut VisualTestContext, selector: &str| bounds(cx, selector).unwrap_or_else(|| panic!("{selector} isn't on screen")).center().y;
    let line = middle(cx, "attach");
    for each in ["mode-chip", "model-chip", "effort-chip", "send"] {
        assert!((middle(cx, each) - line).abs() < px(0.5), "{each} is off the paperclip's line");
    }
}

/// How long it takes, with hundreds of models (not an assertion: a machine's speed varies, so it's
/// run on purpose, `--ignored --nocapture`, and its numbers reported): opening to its first frame
/// drawn, a keystroke's search, an arrow's step, and a frame while scrolling.
#[gpui::test]
#[ignore]
fn timings(cx: &mut TestAppContext) {
    use std::time::{Duration, Instant};
    for count in [600usize, 2000] {
        let stand_in = StandIn::at(Replies::Never, NOW);
        stand_in.more_models(count);
        let (workspace, cx) = conversation(cx, stand_in.clone());
        let composer = workspace.read_with(cx, |workspace, cx| workspace.conversation().unwrap().read(cx).composer().clone());
        // The window's own frame, the picker closed: what every frame here costs anyway.
        let mut baseline = Vec::new();
        for _ in 0..20 {
            baseline.push(cx.update(|window, cx| {
                window.refresh();
                let start = Instant::now();
                window.draw(cx).clear(cx);
                start.elapsed()
            }));
        }
        let mut opens = Vec::new();
        let mut picker = None;
        for _ in 0..20 {
            cx.simulate_keystrokes("escape");
            cx.run_until_parked();
            let took = cx.update(|window, cx| {
                let start = Instant::now();
                picker = composer.update(cx, |composer, cx| composer.open_model_picker(window, cx));
                window.draw(cx).clear(cx);
                start.elapsed()
            });
            opens.push(took);
            cx.run_until_parked();
        }
        let picker = picker.unwrap();
        // A keystroke: the search, as the background runs it.
        let catalog = std::sync::Arc::new(kuvo::model_picker::Catalog::new(&kuvo_box::BoxClient::new(stand_in.clone()).offered_models(None, None).unwrap(), |_| true));
        let mut searches = Vec::new();
        for query in ["m", "mo", "mod", "mode", "model", "model 1", "model 15", "model 159", "gpt", "op"] {
            let start = Instant::now();
            let found = kuvo::model_picker::matching(&catalog, query, &std::sync::atomic::AtomicBool::new(false));
            searches.push(start.elapsed());
            assert!(found.is_some());
        }
        // An arrow: the step and the frame after it.
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|window, cx| composer.update(cx, |composer, cx| composer.open_model_picker(window, cx)));
        cx.run_until_parked();
        picker.update(cx, |picker, cx| picker.show_rail("openrouter", cx));
        let mut steps = Vec::new();
        for _ in 0..30 {
            let took = cx.update(|window, cx| {
                let start = Instant::now();
                window.dispatch_action(Box::new(kuvo::model_picker::Next), cx);
                window.draw(cx).clear(cx);
                start.elapsed()
            });
            steps.push(took);
        }
        // Scrolling: a wheel event's frame.
        let list = bounds(cx, "model-picker").unwrap();
        let mut scrolls = Vec::new();
        for _ in 0..30 {
            cx.simulate_event(gpui::ScrollEvent { position: list.center(), delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-40.))), modifiers: Modifiers::default(), touch_phase: gpui::TouchPhase::Moved, source: gpui::ScrollSource::Pointer });
            let took = cx.update(|window, cx| {
                let start = Instant::now();
                window.draw(cx).clear(cx);
                start.elapsed()
            });
            scrolls.push(took);
        }
        let say = |name: &str, mut all: Vec<Duration>| {
            all.sort();
            let median = all[all.len() / 2];
            let worst = *all.last().unwrap();
            println!("{count} models · {name}: median {:.2} ms, worst {:.2} ms", median.as_secs_f64() * 1000., worst.as_secs_f64() * 1000.);
        };
        say("window frame, picker closed", baseline);
        say("open to first frame", opens);
        say("search a keystroke (background)", searches);
        say("arrow and frame", steps);
        say("scroll frame", scrolls);
    }
}
