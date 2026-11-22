//! Kuvo's screens as someone uses them, against the stand-in box (nothing real is reached): the
//! sidebar listing the box's sessions and opening one, a conversation following a run and paging
//! back, the composer sending through the box (and putting back what the box says to), and a new
//! conversation started from the landing view.
//!
//! The follows that run on threads of their own in the app are left off here: what they'd hand
//! the screens is read at once from the stand-in and handed over the same way, so nothing waits on
//! a clock or another thread.

use gpui::{AppContext as _, Bounds, Entity, Focusable as _, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::composer::{Composer, Models};
use kuvo::conversation::read_now;
use kuvo::entries::Entry;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::models::{ModelState, StartIn};
use kuvo_box::messages::{Content, Message};
use kuvo_box::{BoxClient, ConversationUpdate, Where};
use kuvo_ui::{Appearance, Theme};
use serde_json::json;

const NOW: f64 = 1_794_650_000_000.0;

fn open(cx: &mut TestAppContext, stand_in: StandIn) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(stand_in);
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1200.), px(800.)));
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
    });
    cx.run_until_parked();
    (workspace, cx)
}

fn bounds(cx: &mut VisualTestContext, selector: &str) -> Option<Bounds<Pixels>> {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
}

fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} isn’t visible"));
    cx.simulate_click(at.center(), Modifiers::default());
    cx.run_until_parked();
}

/// Opens `session` and hands it what its follow would.
fn open_session(workspace: &Entity<Workspace>, session: &str, cx: &mut VisualTestContext) -> Entity<Composer> {
    workspace.update_in(cx, |workspace, window, cx| workspace.open_read_at_once(session, window, cx));
    cx.run_until_parked();
    let composer = workspace.read_with(cx, |workspace, cx| workspace.conversation().unwrap().read(cx).composer().clone());
    let field = composer.read_with(cx, |composer, _| composer.field().clone());
    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    composer
}

fn typed(composer: &Entity<Composer>, cx: &mut VisualTestContext) -> (String, Option<String>) {
    composer.read_with(cx, |composer, cx| {
        let field = composer.field().read(cx);
        (field.text().to_owned(), field.error().map(|error| error.to_string()))
    })
}

#[gpui::test]
fn the_sidebar_shows_the_boxs_sessions_by_project_and_opens_one(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in);

    // Projects newest first, each session under its own; chats after; a subagent's nowhere.
    let website = bounds(cx, "project-Website").expect("Website");
    let api = bounds(cx, "project-API").expect("API");
    let signup = bounds(cx, "session-project-ses_signup").expect("the session at work");
    let limits = bounds(cx, "session-project-ses_limits").expect("the session asking");
    let recipes = bounds(cx, "session-chat-ses_recipes").expect("a chat");
    assert!(website.top() < signup.top() && signup.top() < api.top() && api.top() < limits.top() && limits.top() < recipes.top());
    assert!(bounds(cx, "session-project-ses_sub").is_none());
    assert!(bounds(cx, "project-Infra").is_some(), "a project with no sessions still shows");

    // The landing view shows until one's opened; a click opens it.
    assert!(workspace.read_with(cx, |workspace, _| workspace.is_landing()));
    cx.simulate_click(limits.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)), Some("ses_limits".to_owned()));

    // Its conversation shows what its follow hands it.
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    let conversation = read_now(&client, "ses_limits").unwrap();
    view.update(cx, |view, cx| view.take(ConversationUpdate::Conversation(conversation), cx));
    cx.run_until_parked();
    let asked = bounds(cx, "entry-msg_l1").expect("your message");
    let answered = bounds(cx, "entry-msg_l2").expect("its answer");
    assert!(asked.top() < answered.top());
}

#[gpui::test]
fn a_collapsed_project_hides_its_sessions(cx: &mut TestAppContext) {
    let (_workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    let website = bounds(cx, "project-Website").unwrap();
    cx.simulate_click(website.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(bounds(cx, "session-project-ses_signup").is_none());
    assert!(bounds(cx, "session-project-ses_limits").is_some());
    cx.simulate_click(website.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(bounds(cx, "session-project-ses_signup").is_some());
}

#[gpui::test]
fn a_message_written_goes_to_the_box_as_its_fields(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let composer = open_session(&workspace, "ses_limits", cx);

    cx.simulate_input("Yes, use Redis");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("Keep the window at one minute.");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    let sent = stand_in.seen_at("POST", "kuvo/sessions/ses_limits/send");
    assert_eq!(sent.len(), 1);
    let body = sent[0].body.clone().unwrap();
    assert_eq!(body["words"], json!("Yes, use Redis\nKeep the window at one minute."));
    assert!(body["id"].as_str().is_some_and(|id| id.starts_with("msg_")));
    // A plain message isn't asked about first.
    assert!(stand_in.seen_at("POST", "kuvo/sessions/ses_limits/typed").is_empty());
    assert_eq!(typed(&composer, cx), (String::new(), None));
}

#[gpui::test]
fn a_typed_keyword_goes_to_the_box_first_and_what_it_says_is_shown(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let composer = open_session(&workspace, "ses_limits", cx);

    // "/plan sketch it": the box reads it, switches the mode, and says to send the words with it.
    cx.simulate_input("/plan sketch it");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let asked = stand_in.seen_at("POST", "kuvo/sessions/ses_limits/typed");
    assert_eq!(asked.last().unwrap().body.as_ref().unwrap()["text"], json!("/plan sketch it"));
    let sent = stand_in.seen_at("POST", "kuvo/sessions/ses_limits/send");
    assert_eq!((sent[0].body.as_ref().unwrap()["words"].clone(), sent[0].body.as_ref().unwrap()["keyword"].clone()), (json!("sketch it"), json!("/plan")));
    assert_eq!(composer.read_with(cx, |composer, _| composer.note().map(|note| note.text.to_string())), Some("Switched to Plan.".to_owned()));

    // "/loop" with no schedule: the box says why, and puts it back to finish.
    cx.simulate_input("/loop now");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let (text, error) = typed(&composer, cx);
    assert_eq!(text, "/loop now");
    assert!(error.unwrap().starts_with("Can’t tell how often"));
    assert_eq!(stand_in.seen_at("POST", "kuvo/sessions/ses_limits/send").len(), 1, "nothing more was sent");
}

#[gpui::test]
fn slash_offers_what_the_box_offers_as_its_typed(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    let composer = open_session(&workspace, "ses_limits", cx);

    cx.simulate_input("/");
    let names = |composer: &Entity<Composer>, cx: &mut VisualTestContext| composer.read_with(cx, |composer, cx| composer.slash_offers(cx).into_iter().map(|offer| offer.name).collect::<Vec<_>>());
    // Everything it offers (the list scrolls past eight), by name, as KuvoClient's
    // `SlashCommands.matches` orders them.
    assert_eq!(names(&composer, cx), ["autopilot", "btw", "compact", "goal", "handoff", "init", "loop", "plan", "review", "test"]);
    cx.simulate_input("t");
    // Names starting with it first, then those with it inside, each by name.
    assert_eq!(names(&composer, cx), ["test", "autopilot", "btw", "compact", "init"]);
    cx.simulate_keystrokes("backspace v i e w");
    assert_eq!(names(&composer, cx), ["review"]);
    cx.simulate_keystrokes("backspace backspace backspace backspace");
    cx.simulate_input("t");
    assert_eq!(names(&composer, cx)[0], "test");
    cx.simulate_keystrokes("backspace r");
    assert_eq!(names(&composer, cx), ["review"]);
    // Tab (or Return) takes it, ready for what follows; the list goes.
    cx.simulate_keystrokes("tab");
    assert_eq!(typed(&composer, cx).0, "/review ");
    assert!(names(&composer, cx).is_empty());
    // Down then Return takes the second; Escape puts the list away.
    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("/");
    cx.simulate_keystrokes("down enter");
    assert_eq!(typed(&composer, cx).0, "/btw ");
    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("/c");
    cx.simulate_keystrokes("escape");
    assert!(names(&composer, cx).is_empty());
    assert_eq!(typed(&composer, cx).0, "/c");
}

#[gpui::test]
fn a_model_that_cant_run_keeps_the_message_saying_why(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let composer = open_session(&workspace, "ses_limits", cx);
    let state: ModelState = serde_json::from_value(json!({ "model": { "providerID": "anthropic", "id": "claude-opus-5-5" }, "name": "Claude Opus 5.5", "runs": false, "missing": "signIn", "account": { "id": "anthropic", "name": "Anthropic" }, "refusal": "Claude Opus 5.5 needs a sign-in to Anthropic: sign in, or choose another model.", "action": { "do": "signIn", "label": "Sign In", "help": "Sign in to Anthropic" } })).unwrap();
    composer.update(cx, |composer, cx| composer.take_models(Models { state: Some(state), ..Models::default() }, cx));

    cx.simulate_input("Use Redis");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(stand_in.seen_at("POST", "kuvo/sessions/ses_limits/send").is_empty());
    let (text, error) = typed(&composer, cx);
    assert_eq!(text, "Use Redis");
    // In its box's words, as they are.
    assert_eq!(error.as_deref(), Some("Claude Opus 5.5 needs a sign-in to Anthropic: sign in, or choose another model."));
}

#[gpui::test]
fn a_new_session_starts_in_the_project_picked_and_opens(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    // It starts in the newest project until another's picked.
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.landing_place()), StartIn::Project("/Users/sam/Projects/website".into()));
    workspace.update(cx, |workspace, cx| workspace.pick_place(StartIn::Project("/Users/sam/Projects/api".into()), cx));
    let field = workspace.read_with(cx, |workspace, cx| workspace.landing().read(cx).field().clone());
    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    cx.simulate_input("Add a health check route");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    let started = stand_in.seen_at("POST", "kuvo/sessions");
    assert_eq!(started.len(), 1);
    let body = started[0].body.clone().unwrap();
    assert_eq!(body["project"], json!("/Users/sam/Projects/api"));
    assert_eq!(body["model"], json!({ "providerID": "anthropic", "id": "claude-opus-5-5", "variant": "high" }), "on the model the box starts new ones on");
    let opened = workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)).expect("the new session opens");
    let sent = stand_in.seen_at("POST", &format!("kuvo/sessions/{opened}/send"));
    assert_eq!(sent[0].body.as_ref().unwrap()["words"], json!("Add a health check route"));
}

#[gpui::test]
fn earlier_messages_go_above_without_moving_what_youre_reading(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    open_session(&workspace, "ses_lighthouse", cx);
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let entries = view.read_with(cx, |view, _| view.entries().to_vec());
    assert!(matches!(entries[0], Entry::Earlier), "more than a page: the way back is first");

    // Scrolled back to the top: the way back shows, with the oldest read under it.
    for _ in 0..200 {
        if view.read_with(cx, |view, _| view.list().list_state().logical_scroll_top().item_ix) == 0 {
            break;
        }
        view.read_with(cx, |view, _| view.list().scroll_by(px(-600.)));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }
    let first = entries[1].key().to_owned();
    let before = bounds(cx, &format!("entry-{first}")).expect("the oldest read shows");
    assert!(bounds(cx, "entry-earlier").is_some());

    // The page before comes, as the follow hands it: put above, and what was read stays put.
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    let mut conversation = read_now(&client, "ses_lighthouse").unwrap();
    let cursor = conversation.earlier.clone().unwrap();
    let (older, earlier) = client.earlier_messages("ses_lighthouse", Where::Engine, &cursor, 100).unwrap();
    conversation.take_earlier(older, earlier);
    view.update(cx, |view, cx| view.take(ConversationUpdate::Conversation(conversation.clone()), cx));
    cx.run_until_parked();
    // Not pushed a page down, nor jumped to the newest: where it was, give or take the way back's row.
    let after = bounds(cx, &format!("entry-{first}")).expect("still shows");
    // (GPUI's list keeps its place by an item, and the rows put above aren't measured yet, so what
    // was under the way back moves up by that row's height.)
    let way_back = px(64.);
    assert!(after.top() <= before.top() && after.top() >= before.top() - way_back, "{before:?} → {after:?}");
    let (entries, items) = view.read_with(cx, |view, _| (view.entries().to_vec(), view.list().item_count()));
    assert_eq!(entries.len(), items);
    // A page more, and still more before it (the way back stays first).
    assert!(entries.iter().any(|entry| entry.key() == "msg_h010a"));
    assert!(matches!(entries[0], Entry::Earlier));
}

/// An earlier page that ends in the middle of what the agent did: the steps at the top of what was
/// shown go into the group the page started, and the reader stays where they were reading.
#[gpui::test]
fn an_earlier_page_that_starts_a_group_keeps_the_readers_place(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    open_session(&workspace, "ses_signup", cx);
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let step = |id: &str, at: f64, content: serde_json::Value| json!({ "type": "assistant", "id": id, "time": { "created": at, "completed": at + 1.0 }, "content": content });
    let read = |id: &str, file: &str| json!({ "type": "tool", "id": id, "name": "read", "state": { "status": "completed", "input": { "filePath": file } } });
    let words: String = (0..40).map(|n| format!("Line {n} of what it found.\n\n")).collect();
    let messages = |list: serde_json::Value| -> Vec<Message> {
        let mut newest_first: Vec<Message> = serde_json::from_value(list).unwrap();
        newest_first.reverse();
        newest_first
    };
    let mut conversation = kuvo_box::Conversation::new("ses_signup", Where::Engine);
    conversation.catch_up(messages(json!([step("b", 20.0, json!([read("c2", "/p/b.ts"), read("c3", "/p/c.ts")])), step("c", 30.0, json!([{ "type": "text", "text": words }]))])), Some("cursor".into()), &[], Some(false));
    view.update(cx, |view, cx| view.take(ConversationUpdate::Conversation(conversation.clone()), cx));
    cx.run_until_parked();
    // Reading up, at the top.
    view.read_with(cx, |view, _| view.list().list_state().scroll_to(gpui::ListOffset { item_ix: 1, offset_in_item: px(0.) }));
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    assert!(!view.read_with(cx, |view, _| view.list().is_following()));
    // The page before: its step's tool goes on with the ones shown, as one group.
    conversation.take_earlier(messages(json!([step("a", 10.0, json!([{ "type": "text", "text": "Looking." }, read("c1", "/p/a.ts")]))])), None);
    view.update(cx, |view, cx| view.take(ConversationUpdate::Conversation(conversation), cx));
    cx.run_until_parked();
    let (keys, items) = view.read_with(cx, |view, _| (view.entries().iter().map(|entry| entry.key().to_owned()).collect::<Vec<_>>(), view.list().item_count()));
    assert_eq!(keys, ["a", "a#1", "c"]);
    assert_eq!(items, 3);
    assert!(!view.read_with(cx, |view, _| view.list().is_following()), "not thrown to the end");
}

#[gpui::test]
fn a_run_streams_into_the_conversation_and_its_end_shows(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    open_session(&workspace, "ses_signup", cx);
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    assert!(bounds(cx, "entry-working").is_some(), "at work: it says so at the end");

    // Words stream in: the step grows, and the list keeps following it.
    let mut conversation = read_now(&client, "ses_signup").unwrap();
    conversation.apply(&json!({ "type": "session.text.delta", "data": { "sessionID": "ses_signup", "assistantMessageID": "msg_s5", "ordinal": 0, "delta": " now. All 14 pass." } }));
    view.update(cx, |view, cx| view.take(ConversationUpdate::Conversation(conversation.clone()), cx));
    cx.run_until_parked();
    let streamed = view.read_with(cx, |view, _| {
        view.entries().iter().find_map(|entry| match entry {
            Entry::Words { step, parts, .. } if step.id == "msg_s5" => parts.iter().find_map(|n| match &step.content[*n] {
                Content::Text(text) => Some(text.clone()),
                _ => None,
            }),
            _ => None,
        })
    });
    assert!(streamed.unwrap().ends_with("checking the lint now. All 14 pass."));
    assert!(view.read_with(cx, |view, _| view.list().is_following()));
    let step = bounds(cx, "entry-msg_s5#5").unwrap();
    let working = bounds(cx, "entry-working").unwrap();
    assert!(step.bottom() <= working.top());

    // The run ends: Working goes, and the composer's Send is back.
    conversation.apply(&json!({ "type": "session.execution.succeeded", "data": { "sessionID": "ses_signup" } }));
    view.update(cx, |view, cx| view.take(ConversationUpdate::Conversation(conversation), cx));
    cx.run_until_parked();
    assert!(bounds(cx, "entry-working").is_none());
    let items = view.read_with(cx, |view, _| (view.entries().len(), view.list().item_count()));
    assert_eq!(items.0, items.1);
}

#[gpui::test]
fn a_conversation_that_isnt_there_says_so(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    workspace.update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_missing", window, cx));
    cx.run_until_parked();
    let problem = workspace.read_with(cx, |workspace, cx| workspace.conversation().unwrap().read(cx).problem().cloned());
    assert_eq!(problem, Some(kuvo::conversation::Problem::Gone("This conversation isn’t on Stand-in Mac any more.".into())));
}


/// A conversation of the coding agents' while they're down says the box's sentence as it is, with
/// its Try Again; while they restart, nothing (what's shown stays, and it's read again).
#[gpui::test]
fn the_coding_agents_down_are_said_over_the_composer_and_restarting_isnt(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let problem = |workspace: &Entity<Workspace>, cx: &mut VisualTestContext| workspace.read_with(cx, |workspace, cx| workspace.conversation().unwrap().read(cx).problem().cloned());
    // One the engine doesn't have, asked of the coding agents while they restart: no error.
    stand_in.agents_restarting();
    workspace.update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_agent", window, cx));
    cx.run_until_parked();
    assert_eq!(problem(&workspace, cx), None);
    assert!(bounds(cx, "conversation-problem-action").is_none());

    // Down: the box's sentence, as it is, with Try Again, which posts the box's route.
    stand_in.agents_running(false);
    workspace.update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_agent", window, cx));
    cx.run_until_parked();
    let Some(kuvo::conversation::Problem::Failed(words, Some(action))) = problem(&workspace, cx) else { panic!("{:?}", problem(&workspace, cx)) };
    assert_eq!((words.as_str(), action.label.as_str()), ("The coding agents haven't started.", "Try Again"));
    let again = bounds(cx, "conversation-problem-action").expect("Try Again");
    cx.simulate_click(again.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(stand_in.seen_at("POST", "kuvo/agents/start").len(), 1);
    assert_eq!(problem(&workspace, cx), None, "running: the line goes, and it's read again");

    // Over a conversation already shown: the line above the composer, then gone as they restart.
    open_session(&workspace, "ses_limits", cx);
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let down = br#"{"message":"The coding agents didn't finish starting.","reason":"agentsDown","action":{"label":"Try Again","route":"kuvo/agents/start"}}"#;
    view.update(cx, |view, cx| view.take(ConversationUpdate::Failed(kuvo_box::Error::Refused(kuvo_box::Refusal::from_answer(503, down))), cx));
    cx.run_until_parked();
    assert!(bounds(cx, "conversation-problem").is_some() && bounds(cx, "conversation-problem-action").is_some());
    // The one notice over the composer (#339), its glyph and its Try Again in it.
    let notice = bounds(cx, "conversation-problem").unwrap();
    assert!(bounds(cx, "conversation-problem-icon").is_some_and(|icon| notice.contains(&icon.center())));
    assert!(bounds(cx, "conversation-problem-action").is_some_and(|action| notice.contains(&action.center())));
    let restarting = br#"{"message":"The coding agents are restarting. This takes a few seconds.","reason":"agentsRestarting"}"#;
    view.update(cx, |view, cx| view.take(ConversationUpdate::Failed(kuvo_box::Error::Refused(kuvo_box::Refusal::from_answer(503, restarting))), cx));
    cx.run_until_parked();
    assert!(bounds(cx, "conversation-problem").is_none());
    assert!(!view.read_with(cx, |view, _| view.entries().is_empty()), "what was shown stays");
}

#[gpui::test]
fn a_shell_command_runs_on_the_box_and_clears_the_composer(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let composer = open_session(&workspace, "ses_limits", cx);
    let where_gets_before = stand_in.seen_at("GET", "api/session/ses_limits").len();
    // The box decides what to run; the client sends it and follows the conversation.
    cx.simulate_input("!ls -la");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(typed(&composer, cx), (String::new(), None));
    assert!(composer.read_with(cx, |composer, _| composer.note().is_none()));
    assert_eq!(stand_in.seen_at("POST", "api/session/ses_limits/shell").last().and_then(|seen| seen.body.as_ref()).map(|body| &body["command"]), Some(&json!("ls -la")));
    assert_eq!(stand_in.seen_at("GET", "api/session/ses_limits").len(), where_gets_before, "place is known; no extra where_is GET");

    // Re-read into ConversationView and verify the rendered shell entry and output well.
    workspace.update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_limits", window, cx));
    cx.run_until_parked();
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let shell_id = view.read_with(cx, |view, _| {
        view.entries().iter().find_map(|entry| match entry {
            Entry::Message(message) => match message.as_ref() {
                Message::Shell(run) if run.command == "ls -la" => {
                    assert_eq!(run.status, "completed");
                    assert!(run.output.as_ref().is_some_and(|out| out.output.contains("Cargo.toml")));
                    Some(run.id.clone())
                }
                _ => None,
            },
            _ => None,
        })
    }).expect("shell entry in ConversationView");
    assert!(bounds(cx, &format!("entry-{shell_id}")).is_some(), "shell row rendered");
    assert!(bounds(cx, &format!("shell-{shell_id}")).is_some(), "shell header rendered");
    assert!(bounds(cx, &format!("shell-output-{shell_id}")).is_some(), "shell output well rendered");
    let units = view.read_with(cx, |view, _| view.units());
    assert!(units.iter().any(|(row, items)| row == &shell_id && items.iter().any(|unit| unit.text.contains("Cargo.toml"))), "shell output is in rendered units");

    // A /shell refusal puts the command text and the box's error back in the composer.
    stand_in.fail_sends();
    cx.simulate_input("!pwd");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(typed(&composer, cx), ("!pwd".to_owned(), Some("The engine isn't answering. Try again in a moment.".to_owned())));
}

#[gpui::test]
fn a_draft_stays_with_its_conversation(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    let limits = open_session(&workspace, "ses_limits", cx);
    cx.simulate_input("Half a thought");
    open_session(&workspace, "ses_recipes", cx);
    let again = open_session(&workspace, "ses_limits", cx);
    assert_eq!(again, limits);
    assert_eq!(typed(&again, cx).0, "Half a thought");
}

#[gpui::test]
fn a_new_session_whose_message_didnt_go_opens_with_it_and_why(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    stand_in.fail_sends();
    let field = workspace.read_with(cx, |workspace, cx| workspace.landing().read(cx).field().clone());
    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    cx.simulate_input("Add a health check route");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let opened = workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)).expect("the new session opens");
    assert!(opened.starts_with("ses_new"));
    let composer = workspace.read_with(cx, |workspace, cx| workspace.conversation().unwrap().read(cx).composer().clone());
    assert_eq!(typed(&composer, cx), ("Add a health check route".to_owned(), Some("The engine isn't answering. Try again in a moment.".to_owned())));
}

#[gpui::test]
fn with_no_model_to_run_a_new_session_waits_for_one(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.nothing_runs();
    let (workspace, cx) = open(cx, stand_in.clone());
    let landing = workspace.read_with(cx, |workspace, _| workspace.landing().clone());
    assert!(landing.read_with(cx, |composer, _| composer.needs_model()));
    let field = landing.read_with(cx, |composer, _| composer.field().clone());
    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    cx.simulate_input("Anything");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(stand_in.seen_at("POST", "kuvo/sessions").is_empty(), "nothing started");
    assert_eq!(typed(&landing, cx).0, "Anything");
}

#[gpui::test]
fn a_conversations_model_menu_offers_its_own_sides_models(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    // An engine conversation: the coding agents' models (another side) aren't offered in place.
    let composer = open_session(&workspace, "ses_limits", cx);
    let (offered, left_out, efforts) = composer.read_with(cx, |composer, _| {
        (composer.menu_models().iter().map(|model| model.id.clone()).collect::<Vec<_>>(), composer.left_out(), composer.efforts())
    });
    assert_eq!(offered, ["claude-opus-5-5", "claude-sonnet-5-5", "gpt-6", "gemini-3-pro"]);
    assert!(left_out);
    // Its efforts, as the box offers them with it.
    assert_eq!(efforts, ["low", "medium", "high", "max"]);
    // A new one may start on any, the agents' too.
    let landing = workspace.read_with(cx, |workspace, _| workspace.landing().clone());
    assert!(landing.read_with(cx, |composer, _| composer.menu_models().iter().any(|model| model.provider_id == "claude-code")));
}

#[gpui::test]
fn an_unverified_agent_catalog_is_left_out_and_the_conversation_chip_stays(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let composer = open_session(&workspace, "ses_limits", cx);
    let landing = workspace.read_with(cx, |workspace, _| workspace.landing().clone());
    assert!(landing.read_with(cx, |composer, _| composer.menu_models().iter().any(|model| model.provider_id == "claude-code")));
    // The bridge starts again without confirming which accounts are in use.
    stand_in.agents_known(false);
    stand_in.models_readable(false);
    composer.update(cx, |composer, cx| composer.read_models(cx));
    landing.update(cx, |composer, cx| composer.read_models(cx));
    cx.run_until_parked();
    // The picker leaves out the unverified catalog, while the conversation keeps its model chip.
    assert!(landing.read_with(cx, |composer, _| composer.menu_models().iter().all(|model| model.provider_id != "claude-code")));
    assert_eq!(composer.read_with(cx, |composer, _| composer.chip_label()), Some("Claude Opus 5.5".to_owned()));
    assert_eq!(composer.read_with(cx, |composer, _| composer.efforts()).len(), 4);
}

#[gpui::test]
fn a_model_that_changed_elsewhere_is_read_again(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let composer = open_session(&workspace, "ses_limits", cx);
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    // Its account was signed out on another computer: the box hears its accounts changed.
    stand_in.set_model_state("ses_limits", json!({ "model": { "providerID": "anthropic", "id": "claude-opus-5-5" }, "name": "Claude Opus 5.5", "runs": false, "missing": "signIn", "account": { "id": "anthropic", "name": "Anthropic" }, "refusal": "Claude Opus 5.5 needs a sign-in to Anthropic: sign in, or choose another model.", "action": { "do": "signIn", "label": "Sign In", "help": "Sign in to Anthropic" } }));
    let mut conversation = read_now(&client, "ses_limits").unwrap();
    conversation.apply(&json!({ "type": "integration.updated", "data": {} }));
    view.update(cx, |view, cx| view.take(ConversationUpdate::Conversation(conversation), cx));
    cx.run_until_parked();
    // Read again: a message on it is kept, saying what to do.
    cx.simulate_input("Use Redis");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(stand_in.seen_at("POST", "kuvo/sessions/ses_limits/send").is_empty());
    assert_eq!(typed(&composer, cx).1.as_deref(), Some("Claude Opus 5.5 needs a sign-in to Anthropic: sign in, or choose another model."));
}

#[gpui::test]
fn what_the_box_says_of_a_landing_send_goes_with_the_new_conversation(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let landing = workspace.read_with(cx, |workspace, _| workspace.landing().clone());
    let field = landing.read_with(cx, |composer, _| composer.field().clone());
    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    cx.simulate_input("/loop now and then");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    // The session started; the box's answer (put back, and why) is in its own composer.
    let composer = workspace.read_with(cx, |workspace, cx| workspace.conversation().expect("opened").read(cx).composer().clone());
    let (text, error) = typed(&composer, cx);
    assert_eq!(text, "/loop now and then");
    assert!(error.unwrap().starts_with("Can’t tell how often"));
    // The landing composer is left empty for the next one.
    assert_eq!(typed(&landing, cx), (String::new(), None));
}

#[gpui::test]
fn the_landing_project_doesnt_move_while_you_write(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let website = StartIn::Project("/Users/sam/Projects/website".into());
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.landing_place()), website);
    // A session in API works on: API is now the newest project.
    stand_in.set_facts("ses_webhook", |facts| facts["updated"] = json!(NOW + 60_000.0));
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    let mut listing = kuvo_box::Listing::new();
    listing.take(client.session_list(None).unwrap());
    workspace.update(cx, |workspace, cx| workspace.take_list(kuvo_box::ListUpdate::Listing(listing), cx));
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.landing_place()), website, "it stays while you write");
    // Opened again, it starts in the newest.
    workspace.update_in(cx, |workspace, window, cx| workspace.new_session(None, window, cx));
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.landing_place()), StartIn::Project("/Users/sam/Projects/api".into()));
}

/// Opens `session`, which waits on you, as you'd come to it: the keyboard goes where it's drawn.
fn open_asking(workspace: &Entity<Workspace>, session: &str, cx: &mut VisualTestContext) -> (Entity<kuvo::conversation::ConversationView>, Entity<kuvo::ask_card::AskCard>) {
    workspace.update_in(cx, |workspace, window, cx| workspace.open_read_at_once(session, window, cx));
    cx.run_until_parked();
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let card = view.read_with(cx, |view, _| view.asks().clone());
    (view, card)
}

/// Whether the keyboard is on the card (or its line).
fn card_has_keys(card: &Entity<kuvo::ask_card::AskCard>, cx: &mut VisualTestContext) -> bool {
    let focus = card.read_with(cx, |card, cx| card.focus_handle_any(cx));
    cx.update(|window, cx| focus.contains_focused(window, cx))
}

/// The moment a card waits, still, before its keys answer.
fn quiet(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(kuvo::ask_card::QUIET);
    cx.run_until_parked();
}

#[gpui::test]
fn a_permission_shows_in_the_composers_place_and_is_answered_through_the_box(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_asks("ses_limits", vec![kuvo::stand_in::redis_permission()]);
    let (workspace, cx) = open(cx, stand_in.clone());
    let (_, card) = open_asking(&workspace, "ses_limits", cx);

    // The card, as the box words it, in the composer's place; the keyboard's on it.
    assert!(bounds(cx, "ask-card").is_some());
    assert!(bounds(cx, "composer-card").is_none());
    assert!(bounds(cx, "ask-choice-always").is_some());
    assert!(card_has_keys(&card, cx));

    // Escape opens the line for your words; what's written goes as Don't Allow, with them.
    quiet(cx);
    cx.simulate_keystrokes("escape");
    assert!(bounds(cx, "ask-instead").is_some());
    // A number typed there is words, never an answer (pressed as keys, as the keyboard sends them).
    cx.simulate_keystrokes("1 2 space");
    cx.run_until_parked();
    assert!(stand_in.seen_at("POST", "kuvo/sessions/ses_limits/asks/per_redis").is_empty(), "nothing was answered");
    cx.simulate_input("Use the redis package we already have");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let answered = stand_in.seen_at("POST", "kuvo/sessions/ses_limits/asks/per_redis");
    assert_eq!(answered.last().and_then(|seen| seen.body.clone()), Some(json!({ "answer": "instead", "message": "12 Use the redis package we already have" })));

    // Answered: the composer is back, with the keyboard.
    assert!(bounds(cx, "ask-card").is_none());
    assert!(bounds(cx, "composer-card").is_some());
    let field = workspace.read_with(cx, |workspace, cx| workspace.conversation().unwrap().read(cx).composer().read(cx).field().clone());
    assert!(cx.update(|window, cx| field.focus_handle(cx).is_focused(window)));
}

#[gpui::test]
fn a_cards_keys_wait_until_its_been_still_a_moment_and_a_held_key_answers_nothing(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let mut second = kuvo::stand_in::redis_permission();
    second["id"] = json!("per_push");
    second["command"] = json!("git push origin rate-limits");
    stand_in.set_asks("ses_limits", vec![kuvo::stand_in::redis_permission(), second]);
    let (workspace, cx) = open(cx, stand_in.clone());
    let (_, card) = open_asking(&workspace, "ses_limits", cx);
    assert_eq!(card.read_with(cx, |card, _| card.place().words()), Some("1 of 2".into()));

    // As it comes, a key (a Return meant for the composer) answers nothing.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(stand_in.seen_at("POST", "kuvo/sessions/ses_limits/asks/per_redis").is_empty());

    // Typing (a "run step 2" meant for elsewhere) keeps it waiting: the 2 after "step" answers
    // nothing, however long the card has been there.
    quiet(cx);
    cx.simulate_keystrokes("s t e p");
    cx.executor().advance_clock(kuvo::ask_card::QUIET / 2);
    cx.simulate_keystrokes("2");
    cx.run_until_parked();
    assert!(stand_in.seen_at("POST", "kuvo/sessions/ses_limits/asks/per_redis").is_empty());

    // Still a moment: 2, Always Allow, for the first; the second shows next.
    quiet(cx);
    cx.simulate_keystrokes("2");
    cx.run_until_parked();
    let answered = stand_in.seen_at("POST", "kuvo/sessions/ses_limits/asks/per_redis");
    assert_eq!(answered.last().and_then(|seen| seen.body.clone()), Some(json!({ "answer": "always" })));
    assert_eq!(card.read_with(cx, |card, _| card.place().shown().map(str::to_owned)), Some("per_push".into()));

    // The 2 held down, repeating, doesn't answer the one that took its place: neither at once, nor
    // after a moment.
    let held = gpui::KeyDownEvent { keystroke: gpui::Keystroke::parse("2").unwrap(), is_held: true, prefer_character_input: false };
    cx.simulate_event(held.clone());
    quiet(cx);
    cx.simulate_event(held);
    cx.run_until_parked();
    assert!(stand_in.seen_at("POST", "kuvo/sessions/ses_limits/asks/per_push").is_empty());
    cx.simulate_keystrokes("1");
    cx.run_until_parked();
    let answered = stand_in.seen_at("POST", "kuvo/sessions/ses_limits/asks/per_push");
    assert_eq!(answered.last().and_then(|seen| seen.body.clone()), Some(json!({ "answer": "once" })));
}

#[gpui::test]
fn a_key_meant_for_an_ask_answered_elsewhere_doesnt_answer_the_one_that_takes_its_place(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let mut second = kuvo::stand_in::redis_permission();
    second["id"] = json!("per_push");
    stand_in.set_asks("ses_limits", vec![kuvo::stand_in::redis_permission(), second.clone()]);
    let (workspace, cx) = open(cx, stand_in.clone());
    let (view, card) = open_asking(&workspace, "ses_limits", cx);
    quiet(cx);

    // The first is answered on the phone: the second takes its place, and a Return meant for the
    // first, pressed as it does, answers nothing.
    let asks: kuvo_box::asks::Asks = serde_json::from_value(json!({ "asks": [second] })).unwrap();
    view.update(cx, |view, cx| view.take_asks(asks, cx));
    assert!(!card.read_with(cx, |card, cx| card.takes_keys(cx)));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(stand_in.seen_at("POST", "kuvo/sessions/ses_limits/asks/per_push").is_empty());
    // Once it's been there a moment, Return answers it.
    quiet(cx);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let answered = stand_in.seen_at("POST", "kuvo/sessions/ses_limits/asks/per_push");
    assert_eq!(answered.last().and_then(|seen| seen.body.clone()), Some(json!({ "answer": "once" })));
}

/// Answers the first of two asks, the box's word that it's gone coming `before` its own reply or
/// after it; either way the second shows, and takes no key until it's been there a moment.
fn answer_racing_the_boxs_word(before: bool, cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let mut second = kuvo::stand_in::redis_permission();
    second["id"] = json!("per_push");
    stand_in.set_asks("ses_limits", vec![kuvo::stand_in::redis_permission(), second.clone()]);
    let (workspace, cx) = open(cx, stand_in.clone());
    let (view, card) = open_asking(&workspace, "ses_limits", cx);
    quiet(cx);
    let gone: kuvo_box::asks::Asks = serde_json::from_value(json!({ "asks": [second] })).unwrap();
    // Allow Once, picked: its reply is on its way until the window runs on.
    card.update_in(cx, |card, window, cx| card.choose(0, window, cx));
    if before {
        view.update(cx, |view, cx| view.take_asks(gone.clone(), cx));
    }
    cx.run_until_parked();
    if !before {
        view.update(cx, |view, cx| view.take_asks(gone, cx));
        cx.run_until_parked();
    }
    assert_eq!(stand_in.seen_at("POST", "kuvo/sessions/ses_limits/asks/per_redis").len(), 1);
    assert_eq!(card.read_with(cx, |card, _| card.place().shown().map(str::to_owned)), Some("per_push".into()));
    assert!(card.read_with(cx, |card, _| card.failed().is_none()), "nothing said of the next one");
    // Its keys wait (whichever came first), then the first key pressed answers it: none is
    // swallowed by what became of the one before.
    assert!(!card.read_with(cx, |card, cx| card.takes_keys(cx)));
    quiet(cx);
    cx.simulate_keystrokes("1");
    cx.run_until_parked();
    assert_eq!(stand_in.seen_at("POST", "kuvo/sessions/ses_limits/asks/per_push").len(), 1);
}

#[gpui::test]
fn your_answer_whose_reply_comes_after_the_boxs_word_leaves_the_next_ask_as_it_should_be(cx: &mut TestAppContext) {
    answer_racing_the_boxs_word(true, cx);
}

#[gpui::test]
fn your_answer_whose_reply_comes_before_the_boxs_word_leaves_the_next_ask_as_it_should_be(cx: &mut TestAppContext) {
    answer_racing_the_boxs_word(false, cx);
}

#[gpui::test]
fn after_an_answer_in_your_own_words_the_next_card_has_the_keyboard(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let mut second = kuvo::stand_in::redis_permission();
    second["id"] = json!("per_push");
    stand_in.set_asks("ses_limits", vec![kuvo::stand_in::redis_permission(), second]);
    let (workspace, cx) = open(cx, stand_in.clone());
    let (_, card) = open_asking(&workspace, "ses_limits", cx);
    quiet(cx);
    // Tell It What to Do Instead, in your words.
    cx.simulate_keystrokes("escape");
    cx.simulate_input("Not yet");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(card.read_with(cx, |card, _| card.place().shown().map(str::to_owned)), Some("per_push".into()));
    // The next card has the keyboard: its 1, once it's been still, answers it.
    assert!(card_has_keys(&card, cx));
    quiet(cx);
    cx.simulate_keystrokes("1");
    cx.run_until_parked();
    let answered = stand_in.seen_at("POST", "kuvo/sessions/ses_limits/asks/per_push");
    assert_eq!(answered.last().and_then(|seen| seen.body.clone()), Some(json!({ "answer": "once" })));
}

#[gpui::test]
fn after_a_question_answered_in_your_own_words_the_next_one_takes_its_keys(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let mut two = kuvo::stand_in::cms_question();
    let mut next = two["questions"][0].clone();
    next["key"] = json!("q1");
    next["question"] = json!("And the drafts?");
    two["questions"].as_array_mut().unwrap().push(next);
    stand_in.set_asks("ses_cms", vec![two]);
    let (workspace, cx) = open(cx, stand_in.clone());
    let (_, card) = open_asking(&workspace, "ses_cms", cx);
    quiet(cx);
    // The first in your words, in its line.
    let line = bounds(cx, "ask-words").unwrap();
    cx.simulate_click(line.center(), Modifiers::default());
    cx.simulate_keystrokes("k e e p enter");
    cx.run_until_parked();
    // The second: the card has the keyboard (not the line), so its 2 is a pick, once it's still.
    assert!(card_has_keys(&card, cx));
    cx.simulate_keystrokes("2");
    cx.run_until_parked();
    assert!(stand_in.seen_at("POST", "kuvo/sessions/ses_cms/asks/frm_cms").is_empty(), "too soon: nothing, and nothing typed in its line");
    quiet(cx);
    cx.simulate_keystrokes("2");
    cx.run_until_parked();
    let answered = stand_in.seen_at("POST", "kuvo/sessions/ses_cms/asks/frm_cms");
    assert_eq!(answered.last().and_then(|seen| seen.body.clone()), Some(json!({ "answers": { "q0": "keep", "q1": "keep" } })));
}

#[gpui::test]
fn an_ask_takes_the_keyboard_from_wherever_it_was_in_the_conversation(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let (view, card) = open_asking(&workspace, "ses_limits", cx);
    // The keyboard nowhere (a click on the conversation itself), then an ask comes.
    cx.update(|window, cx| window.blur(cx));
    let asks: kuvo_box::asks::Asks = serde_json::from_value(json!({ "asks": [kuvo::stand_in::redis_permission()] })).unwrap();
    view.update(cx, |view, cx| view.take_asks(asks, cx));
    cx.run_until_parked();
    assert!(card_has_keys(&card, cx));
}

#[gpui::test]
fn the_agents_question_takes_a_choice_or_your_own_words(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let (_, card) = open_asking(&workspace, "ses_cms", cx);
    assert!(bounds(cx, "ask-option-0").is_some());
    assert!(bounds(cx, "ask-words").is_some(), "it takes words of your own too");
    assert!(card_has_keys(&card, cx));

    // A Return as it comes, meant for the composer, answers nothing; the keyboard is on the card,
    // not its line, so what you were typing doesn't land there.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(stand_in.seen_at("POST", "kuvo/sessions/ses_cms/asks/frm_cms").is_empty());

    // In its line: Return with nothing written sends nothing (nor puts it away); typing there makes
    // Return wait a moment too, then it answers in your words.
    let line = bounds(cx, "ask-words").unwrap();
    cx.simulate_click(line.center(), Modifiers::default());
    quiet(cx);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(stand_in.seen_at("POST", "kuvo/sessions/ses_cms/asks/frm_cms").is_empty());
    // (It needs an answer: Skip is off.)
    let skip = bounds(cx, "ask-skip").expect("Skip");
    cx.simulate_click(skip.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(stand_in.seen_at("POST", "kuvo/sessions/ses_cms/asks/frm_cms").is_empty(), "Skip is off for one that needs an answer");
    // Written in its line, Return answers in your words.
    let words = bounds(cx, "ask-words").unwrap();
    cx.simulate_click(words.center(), Modifiers::default());
    cx.simulate_keystrokes("k e e p enter");
    cx.run_until_parked();
    let answered = stand_in.seen_at("POST", "kuvo/sessions/ses_cms/asks/frm_cms");
    assert_eq!(answered.last().and_then(|seen| seen.body.clone()), Some(json!({ "answers": { "q0": "keep" } })));
    assert!(bounds(cx, "ask-card").is_none());
}

#[gpui::test]
fn a_click_on_a_choice_answers_the_question(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    open_asking(&workspace, "ses_cms", cx);
    let redirect = bounds(cx, "ask-option-0").unwrap();
    cx.simulate_click(redirect.center(), Modifiers::default());
    cx.run_until_parked();
    let answered = stand_in.seen_at("POST", "kuvo/sessions/ses_cms/asks/frm_cms");
    assert_eq!(answered.last().and_then(|seen| seen.body.clone()), Some(json!({ "answers": { "q0": "redirect" } })));
    assert!(bounds(cx, "ask-card").is_none());
}

#[gpui::test]
fn a_command_outside_the_sandbox_shows_as_its_card_and_is_answered_through_the_box(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let mut second = kuvo::stand_in::sandbox_ask();
    second["id"] = json!("sq_again");
    stand_in.set_asks("ses_limits", vec![kuvo::stand_in::sandbox_ask(), second]);
    let (workspace, cx) = open(cx, stand_in.clone());
    let (_, card) = open_asking(&workspace, "ses_limits", cx);

    // In the composer's place, as the box words it: what was stopped, and its three answers.
    assert!(bounds(cx, "ask-card").is_some());
    assert!(bounds(cx, "composer-card").is_none(), "it holds the conversation up, as a permission does");
    assert!(bounds(cx, "ask-stopped").is_some());
    for each in ["ask-choice-once", "ask-choice-always", "ask-choice-reject"] {
        assert!(bounds(cx, each).is_some(), "{each}");
    }
    assert!(card_has_keys(&card, cx));

    // Return answers nothing here (it would run the command outside the sandbox); its number key
    // picks Always, sent as the box offers it.
    quiet(cx);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(stand_in.seen_at("POST", "kuvo/sessions/ses_limits/asks/sq_redis").is_empty(), "Return answers nothing");
    cx.simulate_keystrokes("2");
    cx.run_until_parked();
    let answered = stand_in.seen_at("POST", "kuvo/sessions/ses_limits/asks/sq_redis");
    assert_eq!(answered.last().and_then(|seen| seen.body.clone()), Some(json!({ "answer": "always" })));

    // The next: Escape says no, as the Mac's card did.
    assert_eq!(card.read_with(cx, |card, _| card.place().shown().map(str::to_owned)), Some("sq_again".to_owned()));
    quiet(cx);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let answered = stand_in.seen_at("POST", "kuvo/sessions/ses_limits/asks/sq_again");
    assert_eq!(answered.last().and_then(|seen| seen.body.clone()), Some(json!({ "answer": "reject" })));
    assert!(bounds(cx, "composer-card").is_some());
}

#[gpui::test]
fn an_item_the_vault_is_asked_for_is_given_by_name_and_a_grant_is_left_to_a_device_that_checks_its_you(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_asks("ses_limits", vec![kuvo::stand_in::vault_ask()]);
    let (workspace, cx) = open(cx, stand_in.clone());
    let _ = open_asking(&workspace, "ses_limits", cx);

    // Give for each item; Always Give, a grant, isn't offered here, and the card says where it is.
    assert!(bounds(cx, "ask-choice-once-itm_redis").is_some());
    assert!(bounds(cx, "ask-choice-once-itm_cloud").is_some());
    assert!(bounds(cx, "ask-choice-always-itm_redis").is_none());
    assert!(bounds(cx, "ask-grants-elsewhere").is_some());

    // The second item, by its number among those shown: given with its item.
    quiet(cx);
    cx.simulate_keystrokes("2");
    cx.run_until_parked();
    let answered = stand_in.seen_at("POST", "kuvo/sessions/ses_limits/asks/q_redis");
    assert_eq!(answered.last().and_then(|seen| seen.body.clone()), Some(json!({ "answer": "once", "item": "itm_cloud" })));
}

#[gpui::test]
fn a_vault_this_app_cant_answer_says_where_to_and_leaves_the_composer(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let mut vault = kuvo::stand_in::vault_ask();
    vault["choices"] = json!([]);
    vault["note"] = json!("Answer it on your iPhone: only a device this computer’s vault knows can answer it.");
    stand_in.set_asks("ses_limits", vec![vault]);
    let (workspace, cx) = open(cx, stand_in.clone());
    let (_, card) = open_asking(&workspace, "ses_limits", cx);
    assert!(bounds(cx, "ask-note").is_some());
    assert!(bounds(cx, "ask-choice-reject").is_none());
    assert!(bounds(cx, "composer-card").is_some(), "the composer stays, under it");
    assert!(!card_has_keys(&card, cx));
}

#[gpui::test]
fn an_ask_this_app_cant_answer_leaves_the_composer_usable_under_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_asks("ses_limits", vec![json!({ "id": "frm_gh", "kind": "other", "session": "ses_limits", "title": "GitHub", "message": "Which repository?", "dismiss": false })]);
    let (workspace, cx) = open(cx, stand_in.clone());
    let (_, card) = open_asking(&workspace, "ses_limits", cx);
    assert!(bounds(cx, "ask-card").is_some());
    assert!(bounds(cx, "composer-card").is_some(), "the composer stays, under it");
    assert!(bounds(cx, "ask-dismiss").is_none(), "one that can't be put away from here");
    assert!(!card_has_keys(&card, cx));
}

#[gpui::test]
fn a_page_that_couldnt_be_read_isnt_asked_again_as_words_stream(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    open_session(&workspace, "ses_lighthouse", cx);
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    view.update(cx, |view, cx| view.take(ConversationUpdate::EarlierFailed(kuvo_box::Error::Unreachable("The box didn’t answer.".into())), cx));
    // Another read of the newest (words streaming): still said, not asked again.
    let conversation = read_now(&client, "ses_lighthouse").unwrap();
    view.update(cx, |view, cx| view.take(ConversationUpdate::Conversation(conversation), cx));
    assert_eq!(view.read_with(cx, |view, _| view.earlier_failed().map(str::to_owned)), Some("The box didn’t answer.".to_owned()));
}

#[gpui::test]
fn a_side_the_box_cant_say_now_is_asked_again_until_it_can(cx: &mut TestAppContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let stand_in = StandIn::at(Replies::Never, NOW);
    let client = BoxClient::new(stand_in.clone());
    // A composer that reads the box itself, as the app's does.
    let composer = cx.new(|cx| Composer::new(client, kuvo::composer::Target::Session { id: "ses_limits".into(), place: Where::Engine }, true, cx));
    cx.run_until_parked();
    let asked = || stand_in.seen_at("GET", "kuvo/models/offered").len();
    assert_eq!(asked(), 1);
    // The bridge starts again: its side isn't said, though what it said before is kept.
    stand_in.agents_known(false);
    composer.update(cx, |composer, cx| composer.read_models(cx));
    cx.run_until_parked();
    assert_eq!(asked(), 2);
    // Asked again in a while, and again (longer each time) while it still can't say.
    cx.executor().advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(asked(), 3);
    cx.executor().advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(asked(), 3, "the second wait is longer");
    cx.executor().advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(asked(), 4);
    // It can say again: no more asking.
    stand_in.agents_known(true);
    cx.executor().advance_clock(std::time::Duration::from_secs(4));
    cx.run_until_parked();
    assert_eq!(asked(), 5);
    cx.executor().advance_clock(std::time::Duration::from_secs(60));
    cx.run_until_parked();
    assert_eq!(asked(), 5);
}

#[gpui::test]
fn a_model_named_by_another_id_is_the_one_offered(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    // A coding agent's conversation on Claude Code's `default`, which it lists as `opus`.
    stand_in.set_model_state("ses_limits", json!({ "model": { "providerID": "claude-code", "id": "default" }, "name": "Opus", "runs": true }));
    let (workspace, cx) = open(cx, stand_in);
    let composer = open_session(&workspace, "ses_limits", cx);
    assert_eq!(composer.read_with(cx, |composer, _| composer.efforts()), ["low", "medium", "high"]);
}

#[gpui::test]
fn a_session_its_box_leaves_out_is_refused_nothing(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_model_state("ses_limits", json!({ "model": { "providerID": "claude-code", "id": "opus" }, "name": "Opus", "runs": false, "missing": "signIn", "account": { "id": "claude-code", "name": "Claude Code" }, "refusal": "Opus needs a sign-in to Claude Code: sign in, or choose another model.", "action": { "do": "signIn", "label": "Sign In", "help": "Sign in to Claude Code" } }));
    let (workspace, cx) = open(cx, stand_in.clone());
    let composer = open_session(&workspace, "ses_limits", cx);
    // Signed in elsewhere; its list still being read, so the box leaves the session out for now.
    stand_in.set_model_state("ses_limits", serde_json::Value::Null);
    composer.update(cx, |composer, cx| composer.read_models(cx));
    cx.run_until_parked();
    // Its model is still named, and nothing's refused on what was said before.
    assert_eq!(composer.read_with(cx, |composer, _| composer.chip_label()), Some("Opus".to_owned()));
    cx.simulate_input("Go on");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(stand_in.seen_at("POST", "kuvo/sessions/ses_limits/send").len(), 1);
}

#[gpui::test]
fn the_landing_view_reads_whats_offered_each_time_it_opens(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.agents_known(false);
    let (workspace, cx) = open(cx, stand_in.clone());
    let landing = workspace.read_with(cx, |workspace, _| workspace.landing().clone());
    let offers_agents = |cx: &mut VisualTestContext| landing.read_with(cx, |composer, _| composer.menu_models().iter().any(|model| model.provider_id == "claude-code"));
    assert!(!offers_agents(cx));
    // The coding agents' bridge is back; New Session again: they're offered.
    stand_in.agents_known(true);
    open_session(&workspace, "ses_recipes", cx);
    workspace.update_in(cx, |workspace, window, cx| workspace.new_session(None, window, cx));
    cx.run_until_parked();
    assert!(offers_agents(cx));
}

#[gpui::test]
fn a_conversations_model_waits_to_be_changed_until_its_known_where_it_runs(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    workspace.update_in(cx, |workspace, window, cx| workspace.open_session("ses_limits", window, cx));
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let composer = view.read_with(cx, |view, _| view.composer().clone());
    assert!(!composer.read_with(cx, |composer, _| composer.can_pick()));
    // Its follow finds it on the engine.
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    let conversation = read_now(&client, "ses_limits").unwrap();
    view.update(cx, |view, cx| view.take(ConversationUpdate::Conversation(conversation), cx));
    assert!(composer.read_with(cx, |composer, _| composer.can_pick()));
}

#[gpui::test]
fn a_new_session_whose_message_didnt_go_while_you_were_elsewhere_is_marked(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    stand_in.fail_sends();
    let landing = workspace.read_with(cx, |workspace, _| workspace.landing().clone());
    let field = landing.read_with(cx, |composer, _| composer.field().clone());
    field.update(cx, |field, cx| field.set_text("Add a health check route", cx));
    // Sent, and before it's done, another conversation opened.
    landing.update(cx, |composer, cx| composer.send(cx));
    workspace.update_in(cx, |workspace, window, cx| workspace.open_session("ses_recipes", window, cx));
    cx.run_until_parked();
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)), Some("ses_recipes".to_owned()), "you stay where you went");
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    let started = client.session_list(None).unwrap().sessions.into_keys().find(|id| id.starts_with("ses_new")).expect("it was started");
    assert!(workspace.read_with(cx, |workspace, _| workspace.is_unsent(&started)));
    // Opened, what didn't go is there, with why; the mark goes.
    let composer = open_session(&workspace, &started, cx);
    assert!(!workspace.read_with(cx, |workspace, _| workspace.is_unsent(&started)));
    assert_eq!(typed(&composer, cx), ("Add a health check route".to_owned(), Some("The engine isn't answering. Try again in a moment.".to_owned())));
}

#[gpui::test]
fn what_is_said_of_the_model_before_a_pick_comes_back_is_left(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    let composer = open_session(&workspace, "ses_limits", cx);
    let sonnet = kuvo_box::typed::ModelRef { provider_id: "anthropic".into(), id: "claude-sonnet-5-5".into(), variant: None };
    let opus: ModelState = serde_json::from_value(json!({ "model": { "providerID": "anthropic", "id": "claude-opus-5-5", "variant": "high" }, "name": "Claude Opus 5.5", "runs": true })).unwrap();
    // Sonnet picked; before its change comes back, a read still says Opus: left.
    composer.update(cx, |composer, cx| {
        composer.pick(sonnet.clone(), cx);
        composer.take_state(opus.clone(), cx);
    });
    assert_eq!(composer.read_with(cx, |composer, _| composer.chip_label()), Some("Claude Sonnet 5.5".to_owned()));
    // Once it's come back, what the box says goes.
    cx.run_until_parked();
    composer.update(cx, |composer, cx| composer.take_state(opus, cx));
    assert_eq!(composer.read_with(cx, |composer, _| composer.chip_label()), Some("Claude Opus 5.5".to_owned()));
}

#[gpui::test]
fn a_tool_step_opens_to_what_it_printed_and_long_output_unfolds(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in);
    open_session(&workspace, "ses_signup", cx);

    // What it did between its words is one line until it's opened; a step that failed shows anyway.
    assert!(bounds(cx, "tool-msg_s5:4").is_none(), "folded in its group");
    assert!(bounds(cx, "tool-msg_s5:3").is_some(), "the failed check shows outside the fold");
    let group = bounds(cx, "group-msg_s5").expect("the group's line");
    cx.simulate_click(group.center(), Modifiers::default());
    cx.run_until_parked();

    // Closed, a step is one line; a click opens it to the command and what it printed, folded.
    assert!(bounds(cx, "output-msg_s5:4").is_none());
    let row = bounds(cx, "tool-msg_s5:4").expect("the test run's row");
    cx.simulate_click(row.center(), Modifiers::default());
    cx.run_until_parked();
    let folded = bounds(cx, "output-msg_s5:4").expect("its output, under it");

    // Show all: the rest of it, then folded again.
    let fold = bounds(cx, "output-msg_s5:4-fold").expect("the fold's button");
    cx.simulate_click(fold.center(), Modifiers::default());
    cx.run_until_parked();
    let whole = bounds(cx, "output-msg_s5:4").expect("its output, whole");
    assert!(whole.size.height > folded.size.height + px(100.), "{whole:?} vs {folded:?}");
    let fold = bounds(cx, "output-msg_s5:4-fold").expect("Show less");
    cx.simulate_click(fold.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(bounds(cx, "output-msg_s5:4").map(|b| b.size.height), Some(folded.size.height));

    // An edit opens to its changes; a step still running has nothing to open.
    let edit = bounds(cx, "tool-msg_s5:2").expect("the edit's row");
    cx.simulate_click(edit.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(bounds(cx, "diff-msg_s5:2-0").is_some());
    let lint = bounds(cx, "group-msg_s5#6").expect("the group under way");
    cx.simulate_click(lint.center(), Modifiers::default());
    cx.run_until_parked();
    let running = bounds(cx, "tool-msg_s5:7").expect("the lint running");
    cx.simulate_click(running.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(bounds(cx, "output-msg_s5:7").is_none());
}

#[gpui::test]
fn another_agents_message_is_a_card_from_it_that_opens_whole_and_to_its_conversation(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    open_session(&workspace, "ses_signup", cx);
    // (Brought into view: the conversation opens at its newest.)
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let at = view.read_with(cx, |view, _| view.entries().iter().position(|entry| entry.key() == "msg_s3a").unwrap());
    view.read_with(cx, |view, _| view.list().list_state().scroll_to(gpui::ListOffset { item_ix: at - 2, offset_in_item: px(0.) }));
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    // A card from it, not a bubble of yours, with its first line; and a session's news, short, whole.
    let folded = bounds(cx, "letter-msg_s3a-0").expect("the card from “Check the other forms”");
    assert!(bounds(cx, "letter-msg_s3b-0").is_some(), "the news of a session it started");
    cx.simulate_click(folded.center(), Modifiers::default());
    cx.run_until_parked();
    let whole = bounds(cx, "letter-msg_s3a-0").unwrap();
    assert!(whole.size.height > folded.size.height + px(20.), "{folded:?} → {whole:?}");
    // Open: its conversation.
    let open_it = bounds(cx, "open-msg_s3a-0").expect("Open");
    cx.simulate_click(open_it.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)), Some("ses_sub".to_owned()));
}

fn type_in(field: &Entity<kuvo_ui::TextField>, text: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    cx.simulate_input(text);
    cx.run_until_parked();
}

#[gpui::test]
fn the_sidebar_search_finds_sessions_by_name_and_by_what_was_said(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    let search = workspace.read_with(cx, |workspace, _| workspace.search().clone());

    // By name as you type: only the sessions whose titles have it.
    type_in(&search, "fo", cx);
    assert!(bounds(cx, "session-found-ses_signup").is_some(), "“Fix the sign-up form…”");
    assert!(bounds(cx, "session-project-ses_dark").is_none(), "the rest of the list goes");
    assert!(bounds(cx, "header-In Conversations").is_none(), "two characters: names only");

    // From three, what was said too: the box searches conversations, less those found by name.
    search.update(cx, |field, cx| field.set_text("", cx));
    type_in(&search, "redis", cx);
    assert!(bounds(cx, "found-ses_limits").is_some(), "the rate limits conversation mentions Redis");
    assert!(bounds(cx, "session-found-ses_signup").is_none());
    assert!(bounds(cx, "header-In Conversations").is_some());

    // Nothing anywhere: said so.
    search.update(cx, |field, cx| field.set_text("", cx));
    type_in(&search, "zzzz", cx);
    assert!(workspace.read_with(cx, |workspace, cx| kuvo::sidebar::lines(workspace, cx).contains(&kuvo::sidebar::Line::Quiet("Nothing matches.".into()))));

    // Escape clears it, and the list is back.
    cx.simulate_keystrokes("escape");
    assert!(bounds(cx, "session-project-ses_dark").is_some());
}

#[gpui::test]
fn the_keyboard_walks_the_sidebar_and_opens_what_its_on(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    workspace.update_in(cx, |workspace, window, cx| workspace.focus_search(window, cx));

    // ↓ from the search steps into the list: what needs you first, then the first project, then its
    // first session.
    cx.simulate_keystrokes("down");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.walked().map(str::to_owned)), Some("needs:ses_limits".into()));
    cx.simulate_keystrokes("down down");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.walked().map(str::to_owned)), Some(format!("project:{}", "/Users/sam/Projects/website")));
    // (Its sessions as its box orders them: ses_signup at work and ses_cms waiting on you first.)
    cx.simulate_keystrokes("down down down");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.walked().map(str::to_owned)), Some("session-project:ses_dark".into()));

    // Return opens it, and the keyboard goes to its composer.
    cx.simulate_keystrokes("enter");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)), Some("ses_dark".into()));
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.walked().map(str::to_owned)), None);

    // Return on a project folds it; ↑ from the first row goes back to the search.
    workspace.update_in(cx, |workspace, window, cx| workspace.focus_search(window, cx));
    cx.simulate_keystrokes("down down down enter");
    assert!(workspace.read_with(cx, |workspace, _| workspace.is_collapsed("/Users/sam/Projects/website")));
    assert!(bounds(cx, "session-project-ses_signup").is_none());
    cx.simulate_keystrokes("up up up");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.walked().map(str::to_owned)), None);
    let search = workspace.read_with(cx, |workspace, _| workspace.search().clone());
    assert!(cx.update(|window, cx| search.focus_handle(cx).is_focused(window)));

    // Return in an empty search opens nothing (nor folds anything).
    cx.simulate_keystrokes("enter");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)), Some("ses_dark".into()));
    assert!(workspace.read_with(cx, |workspace, _| workspace.is_collapsed("/Users/sam/Projects/website")));

    // From the search, Return opens the first that's found.
    cx.simulate_input("lifetimes");
    cx.simulate_keystrokes("enter");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)), Some("ses_lifetimes".into()));
}

#[gpui::test]
fn files_attached_go_with_the_message_and_show_on_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let composer = open_session(&workspace, "ses_recipes", cx);
    let folder = std::env::temp_dir().join(format!("kuvo-attach-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let (note, picture) = (folder.join("shopping list.txt"), folder.join("fridge.png"));
    std::fs::write(&note, "rice").unwrap();
    std::fs::write(&picture, b"not really a png").unwrap();

    // Attached (as the picker or a drop hands them), each shows; a folder isn't a file to send.
    composer.update(cx, |composer, cx| composer.attach([note.clone(), picture.clone(), folder.clone(), note.clone()], cx));
    cx.run_until_parked();
    assert_eq!(composer.read_with(cx, |composer, _| composer.attached().to_vec()), [note.clone(), picture.clone()]);
    assert!(bounds(cx, "attachments").is_some());

    // × takes one away; Send takes them with what's written, as files on this Mac's box.
    composer.update(cx, |composer, cx| composer.detach(1, cx));
    cx.simulate_input("What can I make with these?");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let sent = stand_in.seen_at("POST", "kuvo/sessions/ses_recipes/send");
    let body = sent.last().and_then(|seen| seen.body.clone()).expect("sent");
    let uri = format!("file://{}", note.to_string_lossy().replace(' ', "%20"));
    assert_eq!(body["files"], json!([{ "uri": uri, "name": "shopping list.txt" }]));
    assert!(composer.read_with(cx, |composer, _| composer.attached().is_empty()));

    // Files alone can be sent.
    composer.update(cx, |composer, cx| composer.attach([picture.clone()], cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(stand_in.seen_at("POST", "kuvo/sessions/ses_recipes/send").len(), 2);
    std::fs::remove_dir_all(&folder).ok();
}

#[gpui::test]
fn sent_while_the_agent_works_it_waits_for_its_next_step_and_can_be_taken_back(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    open_session(&workspace, "ses_recipes", cx);
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    // The agent at work: what's said of the conversation before the box hands it what's sent.
    let mut working = read_now(&client, "ses_recipes").unwrap();
    working.running = true;
    view.update(cx, |view, cx| view.take(ConversationUpdate::Conversation(working.clone()), cx));
    cx.run_until_parked();

    cx.simulate_input("Also check the oven");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let id = stand_in.seen_at("POST", "kuvo/sessions/ses_recipes/send").last().and_then(|seen| seen.body.clone()).unwrap()["id"].as_str().unwrap().to_owned();
    // Sent: in the conversation at once, where it stays (#809).
    let at_once = view.read_with(cx, |view, _| view.entries().iter().position(|entry| entry.key() == id)).expect("drawn at once");
    // Its box holds it for the agent's next step: it stays where it is, saying so, never moved to
    // the queue over the composer.
    view.update(cx, |view, cx| view.take(ConversationUpdate::Conversation(working.clone()), cx));
    view.update(cx, |view, cx| view.take_queue(serde_json::from_value(json!({ "paused": false, "items": [{ "id": id, "words": "Also check the oven", "files": [], "keyword": null, "from": null, "on": "engine", "delivery": "steer", "held": false }] })).unwrap(), cx));
    cx.run_until_parked();
    assert!(bounds(cx, "at-next-step").is_some());
    assert!(bounds(cx, &format!("queued-{id}")).is_none(), "not in the queue too");
    assert!(bounds(cx, "queue").is_none());
    assert_eq!(view.read_with(cx, |view, _| view.entries().iter().position(|entry| entry.key() == id)), Some(at_once), "where it was");
    // Handed to the agent: the box's queue no longer lists it, and the conversation doesn't have
    // it yet. It stays, as it was, until its copy comes.
    view.update(cx, |view, cx| view.take_queue(serde_json::from_value(json!({ "paused": false, "items": [] })).unwrap(), cx));
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |view, _| view.entries().iter().position(|entry| entry.key() == id)), Some(at_once), "never gone between the queue and the conversation");
    assert!(bounds(cx, "at-next-step").is_none(), "nothing to take back now");

    // Another, still waiting for the next step: its Remove asks its box to take it back.
    cx.simulate_input("And the timer");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let other = stand_in.seen_at("POST", "kuvo/sessions/ses_recipes/send").last().and_then(|seen| seen.body.clone()).unwrap()["id"].as_str().unwrap().to_owned();
    view.update(cx, |view, cx| view.take_queue(serde_json::from_value(json!({ "paused": false, "items": [{ "id": other, "words": "And the timer", "files": [], "keyword": null, "from": null, "on": "engine", "delivery": "steer", "held": false }] })).unwrap(), cx));
    cx.run_until_parked();
    let remove = bounds(cx, &format!("remove-waiting-{other}")).expect("Remove");
    cx.simulate_click(remove.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(stand_in.seen_at("DELETE", &format!("kuvo/sessions/ses_recipes/queue/{other}")).len(), 1);
}

#[gpui::test]
fn files_sent_to_a_conversation_on_another_computer_go_with_their_bytes_and_pictures_are_named(cx: &mut TestAppContext) {
    let png = |width: u32| {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(width, 4).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    };
    let stand_in = StandIn::at(Replies::Never, NOW);
    // The conversation runs on another computer: no path on this Mac means anything there.
    stand_in.set_elsewhere(true);
    let (workspace, cx) = open(cx, stand_in.clone());
    let composer = open_session(&workspace, "ses_recipes", cx);
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    let folder = tempfile::tempdir().unwrap();
    let (first, notes, second) = (folder.path().join("fridge.png"), folder.path().join("list.txt"), folder.path().join("shelf.png"));
    std::fs::write(&first, png(3)).unwrap();
    std::fs::write(&notes, "rice").unwrap();
    std::fs::write(&second, png(5)).unwrap();

    // Attached as you pick or drop them: each picture named where you're writing.
    cx.simulate_input("Compare");
    composer.update(cx, |composer, cx| composer.attach_named([first.clone(), notes.clone(), second.clone()], cx));
    cx.run_until_parked();
    assert_eq!(typed(&composer, cx).0, "Compare [Image 1] [Image 2] ");
    // Taking a picture off takes its name, and the later one moves down.
    composer.update(cx, |composer, cx| composer.detach(0, cx));
    assert_eq!(typed(&composer, cx).0, "Compare [Image 1] ");
    composer.update(cx, |composer, cx| composer.attach_named([first.clone()], cx));
    cx.simulate_input("please");
    assert_eq!(typed(&composer, cx).0, "Compare [Image 1] [Image 2] please");

    // Sent: each file with its bytes, as a phone sends them; the words keep the names.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let body = stand_in.seen_at("POST", "kuvo/sessions/ses_recipes/send").last().and_then(|seen| seen.body.clone()).expect("sent");
    assert_eq!(body["words"], "Compare [Image 1] [Image 2] please");
    let files = body["files"].as_array().unwrap();
    assert_eq!(files.iter().map(|file| file["name"].as_str().unwrap()).collect::<Vec<_>>(), ["list.txt", "shelf.png", "fridge.png"]);
    assert!(files[1]["uri"].as_str().unwrap().starts_with("data:image/png;base64,"));
    assert!(files[0]["uri"].as_str().unwrap().starts_with("data:text/plain;base64,"));
    assert!(!body.to_string().contains(&folder.path().to_string_lossy().into_owned()), "no path on this Mac goes");

    // Its box's copy carries them: it takes the row's place, its pictures shown under it.
    let id = body["id"].as_str().unwrap().to_owned();
    view.update(cx, |view, cx| view.take(ConversationUpdate::Conversation(read_now(&client, "ses_recipes").unwrap()), cx));
    cx.run_until_parked();
    assert!(composer.read_with(cx, |composer, _| composer.outgoing().is_empty()));
    assert!(bounds(cx, &format!("picture-{id}#1")).is_some(), "its picture, from its copy");

    // A file that can't be read as it goes: not sent, saying which, with Retry.
    let gone = folder.path().join("gone.png");
    std::fs::write(&gone, png(2)).unwrap();
    composer.update(cx, |composer, cx| composer.attach_named([gone.clone()], cx));
    std::fs::remove_file(&gone).unwrap();
    let field = composer.read_with(cx, |composer, _| composer.field().clone());
    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(matches!(view.read_with(cx, |view, _| view.entries().last().cloned()), Some(Entry::Outgoing { failed: Some(why), .. }) if why == "“gone.png” couldn’t be read."));
    assert!(bounds(cx, "not-sent").is_some());
}

/// A picture you send is a small thumbnail on your message in every state it's in (sending,
/// waiting for the agent's next step, not sent, its copy come without it, its copy with it), never a
/// file's name; a file that isn't a picture keeps its name. Each is among the conversation's pictures.
#[gpui::test]
fn a_picture_you_send_is_a_thumbnail_on_your_message_in_every_state(cx: &mut TestAppContext) {
    let png = |width: u32, height: u32| {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(width, height).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    };
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let composer = open_session(&workspace, "ses_recipes", cx);
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    let folder = tempfile::tempdir().unwrap();
    let (note, shot, tall) = (folder.path().join("list.txt"), folder.path().join("Pasted Image.png"), folder.path().join("tall.png"));
    std::fs::write(&note, "rice").unwrap();
    std::fs::write(&shot, png(2880, 1800)).unwrap();
    std::fs::write(&tall, png(300, 1200)).unwrap();
    let send = |files: Vec<std::path::PathBuf>, words: &str, cx: &mut VisualTestContext| {
        composer.update(cx, |composer, cx| composer.attach_named(files, cx));
        let field = composer.read_with(cx, |composer, _| composer.field().clone());
        cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
        cx.simulate_input(words);
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        stand_in.seen_at("POST", "kuvo/sessions/ses_recipes/send").last().and_then(|seen| seen.body.clone()).unwrap()["id"].as_str().unwrap().to_owned()
    };
    let size_of = |id: &str, cx: &mut VisualTestContext| bounds(cx, &format!("picture-{id}")).map(|b| (f32::from(b.size.width).round(), f32::from(b.size.height).round()));

    // (Its box's copies come without the files sent, as a box before files were kept.)
    stand_in.drop_files();
    // Sending: one picture, at most 240 on its longest side, its shape kept; the note by its name.
    let id = send(vec![note.clone(), shot.clone()], "Which shelf?", cx);
    assert!(bounds(cx, "sending").is_some());
    assert_eq!(size_of(&format!("{id}#1"), cx), Some((240., 150.)));
    let names = view.read_with(cx, |view, _| view.entries().iter().find_map(|entry| match entry {
        Entry::Outgoing { files, .. } => Some(files.clone()),
        _ => None,
    }));
    assert_eq!(names, Some(vec!["list.txt".to_owned()]), "only what isn't a picture is a name");
    // Its copy came without them: still shown, from here.
    view.update(cx, |view, cx| view.take(ConversationUpdate::Conversation(read_now(&client, "ses_recipes").unwrap()), cx));
    cx.run_until_parked();
    assert!(matches!(view.read_with(cx, |view, _| view.entries().iter().find(|entry| entry.key() == id).cloned()), Some(Entry::Message(_))));
    assert_eq!(size_of(&format!("{id}#1"), cx), Some((240., 150.)), "its copy shows it as it went");

    // Waiting for the agent's next step: several, smaller, a tall one as tall as it may be.
    let mut working = read_now(&client, "ses_recipes").unwrap();
    working.running = true;
    view.update(cx, |view, cx| view.take(ConversationUpdate::Conversation(working.clone()), cx));
    cx.run_until_parked();
    let waiting = send(vec![shot.clone(), tall.clone()], "And these?", cx);
    view.update(cx, |view, cx| view.take(ConversationUpdate::Conversation(working.clone()), cx));
    cx.run_until_parked();
    assert!(bounds(cx, "at-next-step").is_some());
    assert_eq!(size_of(&format!("{waiting}#0"), cx), Some((160., 100.)));
    assert_eq!(size_of(&format!("{waiting}#1"), cx), Some((40., 160.)));

    // Not sent: still there, as it was.
    stand_in.fail_sends();
    let failed = send(vec![shot.clone()], "Once more", cx);
    assert!(bounds(cx, "not-sent").is_some());
    assert_eq!(size_of(&format!("{failed}#0"), cx), Some((240., 150.)));

    // Every one of them is among the conversation's pictures, for the viewer to go through.
    let all: Vec<String> = view.read_with(cx, |view, _| view.all_pictures().into_iter().map(|picture| picture.id).collect());
    for id in [format!("{id}#1"), format!("{waiting}#0"), format!("{waiting}#1"), format!("{failed}#0")] {
        assert!(all.contains(&id), "{id} in {all:?}");
    }
    let shown = bounds(cx, &format!("picture-{failed}#0")).unwrap();
    cx.simulate_click(shown.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |view, _| view.viewing().map(str::to_owned)), Some(format!("{failed}#0")));
}

/// Its box's copy takes over from what was read here: this Mac's box names a picture sent from here
/// by its path, and it's read from the box, as small as it went.
#[gpui::test]
fn a_picture_you_sent_shows_from_its_box_copy_once_it_comes(cx: &mut TestAppContext) {
    let png = |width: u32, height: u32| {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(width, height).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    };
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let composer = open_session(&workspace, "ses_recipes", cx);
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    let folder = tempfile::tempdir().unwrap();
    let (note, shot) = (folder.path().join("list.txt"), folder.path().join("shelf.png"));
    std::fs::write(&note, "rice").unwrap();
    std::fs::write(&shot, png(480, 300)).unwrap();
    composer.update(cx, |composer, cx| composer.attach_named([note.clone(), shot.clone()], cx));
    cx.simulate_input("Which shelf?");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let body = stand_in.seen_at("POST", "kuvo/sessions/ses_recipes/send").last().and_then(|seen| seen.body.clone()).unwrap();
    let id = body["id"].as_str().unwrap().to_owned();
    let size = |cx: &mut VisualTestContext| bounds(cx, &format!("picture-{id}#1")).map(|b| (f32::from(b.size.width).round(), f32::from(b.size.height).round()));
    assert_eq!(size(cx), Some((240., 150.)), "read here while it goes");
    // Its copy has it, by its path on the box's computer (this Mac): read from there.
    stand_in.put_file(&shot.to_string_lossy(), png(480, 300));
    view.update(cx, |view, cx| view.take(ConversationUpdate::Conversation(read_now(&client, "ses_recipes").unwrap()), cx));
    cx.run_until_parked();
    assert!(matches!(view.read_with(cx, |view, _| view.entries().iter().find(|entry| entry.key() == id).cloned()), Some(Entry::Message(_))));
    let copy = bounds(cx, &format!("picture-{id}@{}", shot.to_string_lossy())).expect("its copy's picture");
    assert_eq!((f32::from(copy.size.width).round(), f32::from(copy.size.height).round()), (240., 150.));
    assert!(!stand_in.seen_at("GET", "kuvo/file").is_empty(), "asked of its box");
}

#[gpui::test]
fn a_box_copy_without_the_files_sent_keeps_them_shown(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.drop_files();
    let (workspace, cx) = open(cx, stand_in.clone());
    let composer = open_session(&workspace, "ses_recipes", cx);
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    let folder = tempfile::tempdir().unwrap();
    let note = folder.path().join("list.txt");
    std::fs::write(&note, "rice").unwrap();
    composer.update(cx, |composer, cx| composer.attach_named([note.clone()], cx));
    cx.simulate_input("Here");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let id = stand_in.seen_at("POST", "kuvo/sessions/ses_recipes/send").last().and_then(|seen| seen.body.clone()).unwrap()["id"].as_str().unwrap().to_owned();
    view.update(cx, |view, cx| view.take(ConversationUpdate::Conversation(read_now(&client, "ses_recipes").unwrap()), cx));
    cx.run_until_parked();
    // Its copy in its place, once, and the file sent with it still shown under it.
    assert_eq!(view.read_with(cx, |view, _| view.entries().iter().filter(|entry| entry.key() == id).count()), 1);
    assert!(matches!(view.read_with(cx, |view, _| view.entries().iter().find(|entry| entry.key() == id).cloned()), Some(Entry::Message(_))));
    assert!(bounds(cx, &format!("kept-files-{id}")).is_some(), "the files sent stay in sight");
}

#[gpui::test]
fn a_message_of_only_pictures_from_the_phone_shows_them_and_one_that_fails_says_so_with_try_again(cx: &mut TestAppContext) {
    use base64::Engine as _;
    let png = |width: u32| {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(width, 10).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    };
    let stand_in = StandIn::at(Replies::Never, NOW);
    // As the box keeps a phone's photos: one the model reads, carried in the message; one kept on
    // its disk, named in the message's parts. No words.
    let kept = "/home/sam/.kuvo-code/attachments/ses_recipes/b10bd6c5/Photo 2.jpeg";
    stand_in.add_message("ses_recipes", json!({ "type": "user", "id": "msg_ph", "time": { "created": NOW - 60_000.0 }, "text": "",
        "files": [{ "mime": "image/png", "name": "Photo 1.png", "data": base64::engine::general_purpose::STANDARD.encode(png(30)) }],
        "metadata": { "kuvo": { "parts": { "words": "", "phone": true, "paths": [kept] } } } }));
    let (workspace, cx) = open(cx, stand_in.clone());
    open_session(&workspace, "ses_recipes", cx);
    cx.run_until_parked();
    assert!(bounds(cx, "picture-msg_ph#0").is_some(), "the photo it carried");
    // The one kept on its computer couldn't be read: said plainly, with Try Again.
    let failed = bounds(cx, &format!("picture-msg_ph@{kept}")).expect("a tile in its place");
    assert!(failed.size.width > px(0.));
    let again = bounds(cx, &format!("picture-again-msg_ph@{kept}")).expect("Try Again");
    stand_in.put_file(kept, png(40));
    cx.simulate_click(again.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(bounds(cx, &format!("picture-again-msg_ph@{kept}")).is_none());
    let shown = bounds(cx, &format!("picture-msg_ph@{kept}")).unwrap();
    assert_eq!((shown.size.width, shown.size.height), (px(40.), px(10.)), "shown once read");
}

#[gpui::test]
fn pictures_in_a_conversation_show_in_it_and_open_larger(cx: &mut TestAppContext) {
    use base64::Engine as _;
    let png = |width: u32, height: u32| {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(width, height).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    };
    let stand_in = StandIn::at(Replies::Never, NOW);
    // Yours, with a picture it carried; and the agent's reply, showing you a file on its computer.
    let yours = base64::engine::general_purpose::STANDARD.encode(png(40, 30));
    stand_in.add_message("ses_recipes", json!({ "type": "user", "id": "msg_p1", "time": { "created": NOW - 60_000.0 }, "text": "What's this?", "files": [{ "mime": "image/png", "name": "fridge.png", "data": yours }] }));
    stand_in.add_message("ses_recipes", json!({ "type": "assistant", "id": "msg_p2", "time": { "created": NOW - 50_000.0, "completed": NOW - 40_000.0 }, "agent": "build", "finish": "stop", "content": [
        { "type": "tool", "id": "prt_p", "name": "kuvo-browser_show_file", "state": { "status": "completed", "input": { "path": "/home/sam/plan one.png", "caption": "This week's plan" }, "content": [] } },
        { "type": "tool", "id": "prt_s", "name": "kuvo-browser_browser_screenshot", "state": { "status": "completed", "input": {}, "content": [{ "type": "file", "uri": format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png(20, 10))), "mime": "image/png" }] } },
        { "type": "text", "text": "Here's the plan, and last week's:\n\n![](/home/sam/last-week.png)\n\n![The site](https://example.com/shot.png)" },
    ] }));
    stand_in.put_file("/home/sam/plan one.png", png(1200, 600));
    stand_in.put_file("/home/sam/last-week.png", png(100, 900));
    let (workspace, cx) = open(cx, stand_in.clone());
    // (Tall enough to show the whole conversation.)
    cx.simulate_resize(size(px(1200.), px(2400.)));
    open_session(&workspace, "ses_recipes", cx);
    cx.run_until_parked();
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let all: Vec<String> = view.read_with(cx, |view, _| view.all_pictures().into_iter().map(|picture| picture.id).collect());
    assert_eq!(all, ["msg_p1#0", "msg_p2:0-shown", "msg_p2:2~0"], "a step's own screenshot isn't among them while it's folded");

    // Yours, under your words, at its own size; the one shown, fitted to 560 × 420, from its computer.
    let mine = bounds(cx, "picture-msg_p1#0").expect("your picture is drawn");
    assert_eq!((mine.size.width, mine.size.height), (px(40.), px(30.)));
    let shown = bounds(cx, "picture-msg_p2:0-shown").expect("the picture shown is drawn");
    assert_eq!((shown.size.width, shown.size.height), (px(560.), px(280.)));
    // One written in its words, from its computer too; a web address isn't fetched.
    let written = bounds(cx, "picture-msg_p2:2~0").expect("the picture in its words is drawn");
    assert_eq!(written.size.height, px(420.));
    assert!((f32::from(written.size.width) - 100. * 420. / 900.).abs() < 0.5, "fitted, as it is: {:?}", written.size);
    let asked = stand_in.seen_at("GET", "kuvo/file");
    assert_eq!(asked.len(), 2, "each read once, from the conversation's computer");

    // The screenshot the agent took for itself: inside its step, once that's opened.
    assert!(bounds(cx, "picture-msg_p2:1-0").is_none());
    view.update(cx, |view, cx| view.open_step("msg_p2", 1, cx));
    cx.run_until_parked();
    assert!(bounds(cx, "picture-msg_p2:1-0").is_some(), "shown once the step is opened");

    // A click opens it over the conversation; → and ← go among its pictures; Escape closes.
    cx.simulate_click(shown.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |view, _| view.viewing().map(str::to_owned)), Some("msg_p2:0-shown".into()));
    assert!(bounds(cx, "picture-viewer").is_some());
    cx.simulate_keystrokes("left");
    assert_eq!(view.read_with(cx, |view, _| view.viewing().map(str::to_owned)), Some("msg_p1#0".into()));
    cx.simulate_keystrokes("right");
    assert_eq!(view.read_with(cx, |view, _| view.viewing().map(str::to_owned)), Some("msg_p2:0-shown".into()));
    cx.simulate_keystrokes("escape");
    assert!(view.read_with(cx, |view, _| view.viewing().is_none()));
    assert!(bounds(cx, "picture-viewer").is_none());
}

#[gpui::test]
fn what_you_send_shows_at_once_and_the_boxs_copy_takes_its_place(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    open_session(&workspace, "ses_recipes", cx);
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    let rows = |cx: &mut VisualTestContext| view.read_with(cx, |view, _| view.entries().iter().map(|entry| entry.key().to_owned()).collect::<Vec<_>>());
    let before = rows(cx);

    // Sent: in the conversation at once, as you wrote it, before its box's word of it has come.
    cx.simulate_input("Something with lentils?");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let sent = stand_in.seen_at("POST", "kuvo/sessions/ses_recipes/send");
    let id = sent.last().and_then(|seen| seen.body.as_ref()).and_then(|body| body["id"].as_str()).expect("sent with its id").to_owned();
    let now = rows(cx);
    assert_eq!(now.len(), before.len() + 1);
    assert_eq!(now.last(), Some(&id));
    assert!(matches!(view.read_with(cx, |view, _| view.entries().last().cloned()), Some(Entry::Outgoing { words, failed: None, .. }) if words == "Something with lentils?"));
    // On its way with nothing under it to wait on (#809): the bubble as it will be once sent (its
    // words wrapping where they will then), and its row no taller.
    let bubble = format!("bubble-{}", now.len() - 1);
    let sending_bubble = bounds(cx, &bubble).expect("its bubble");
    let sending_row = bounds(cx, "sending").expect("on its way");
    assert!(sending_row.bottom() <= sending_bubble.bottom() + px(1.), "no Sending… line under it");

    // The box's copy comes (the same id): it takes the row's place, once.
    let conversation = read_now(&client, "ses_recipes").unwrap();
    view.update(cx, |view, cx| view.take(ConversationUpdate::Conversation(conversation), cx));
    cx.run_until_parked();
    assert_eq!(bounds(cx, &bubble).map(|sent| sent.size), Some(sending_bubble.size), "the bubble doesn't change shape as it's sent");
    let after = rows(cx);
    assert_eq!(after.iter().filter(|key| **key == id).count(), 1, "no copy twice");
    assert!(matches!(view.read_with(cx, |view, _| view.entries().iter().find(|entry| entry.key() == id).cloned()), Some(Entry::Message(_))));
    assert!(bounds(cx, "sending").is_none());

    // One that couldn't go stays in the conversation, saying so, with Retry; the composer is free.
    stand_in.fail_sends();
    cx.simulate_input("And with rice?");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(matches!(view.read_with(cx, |view, _| view.entries().last().cloned()), Some(Entry::Outgoing { failed: Some(why), .. }) if why == "The engine isn't answering. Try again in a moment."));
    assert!(bounds(cx, "not-sent").is_some());
    let composer = view.read_with(cx, |view, _| view.composer().clone());
    assert_eq!(typed(&composer, cx), (String::new(), None));
    let failed = stand_in.seen_at("POST", "kuvo/sessions/ses_recipes/send").last().and_then(|seen| seen.body.clone()).unwrap();
    // Said as the one notice under its message (#339): its glyph, under the bubble, Retry in it.
    let notice = bounds(cx, "not-sent").unwrap();
    let last = rows(cx).len() - 1;
    let unsent = bounds(cx, &format!("bubble-{last}")).expect("its bubble");
    assert!(notice.top() >= unsent.bottom(), "under its message");
    assert!(bounds(cx, "not-sent-icon").is_some_and(|icon| notice.contains(&icon.center())), "the notice's glyph");
    assert!(bounds(cx, &format!("retry-{}", failed["id"].as_str().unwrap())).is_some_and(|retry| notice.contains(&retry.center())), "Retry in the notice");

    // Retry sends it again, under the same id, and it's on its way again.
    stand_in.sends_go();
    let retry = bounds(cx, &format!("retry-{}", failed["id"].as_str().unwrap())).expect("Retry");
    cx.simulate_click(retry.center(), Modifiers::default());
    cx.run_until_parked();
    let again = stand_in.seen_at("POST", "kuvo/sessions/ses_recipes/send").last().and_then(|seen| seen.body.clone()).unwrap();
    assert_eq!(again["id"], failed["id"]);
    assert_eq!(again["words"], "And with rice?");
    assert!(bounds(cx, "not-sent").is_none());
    assert!(bounds(cx, "sending").is_some());

    // Edit, on one that couldn't go, puts it back in the composer to change.
    stand_in.fail_sends();
    let field = composer.read_with(cx, |composer, _| composer.field().clone());
    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    cx.simulate_input("Or with beans?");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let beans = view.read_with(cx, |view, _| view.entries().iter().rev().find_map(|entry| match entry {
        Entry::Outgoing { id, failed: Some(_), .. } => Some(id.clone()),
        _ => None,
    })).expect("not sent");
    let edit = bounds(cx, &format!("edit-unsent-{beans}")).expect("Edit");
    cx.simulate_click(edit.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(typed(&composer, cx).0, "Or with beans?");
    assert!(!rows(cx).contains(&beans));
}

#[gpui::test]
fn pasting_a_screenshot_or_copied_files_attaches_them_and_text_still_pastes(cx: &mut TestAppContext) {
    use gpui::{ClipboardEntry, ClipboardItem, ExternalPaths, Image, ImageFormat};
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let pasted = tempfile::tempdir().unwrap();
    cx.update(|_, cx| cx.set_global(kuvo::pasted::PastedFolder(pasted.path().to_path_buf())));
    let composer = open_session(&workspace, "ses_recipes", cx);
    let screenshot = {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(3, 2).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    };

    // A copied screenshot (a picture, no text): ⌘V attaches it, kept as a file, and names it.
    cx.write_to_clipboard(ClipboardItem::from(Image::from_bytes(ImageFormat::Png, screenshot.clone())));
    cx.simulate_keystrokes("cmd-v");
    cx.run_until_parked();
    let attached = composer.read_with(cx, |composer, _| composer.attached().to_vec());
    assert_eq!(attached.len(), 1);
    assert_eq!(std::fs::read(&attached[0]).unwrap(), screenshot);
    assert!(attached[0].file_name().unwrap().to_string_lossy().starts_with("Pasted Image "));
    assert_eq!(typed(&composer, cx).0, "[Image 1] ", "named where you're writing, as 0.7 did");
    assert!(bounds(cx, "attachments").is_some());

    // Files copied in the Finder (their names come as text too): attached, the names not typed.
    let folder = tempfile::tempdir().unwrap();
    let (note, photo) = (folder.path().join("list.txt"), folder.path().join("fridge.jpg"));
    std::fs::write(&note, "rice").unwrap();
    std::fs::write(&photo, b"jpeg").unwrap();
    cx.write_to_clipboard(ClipboardItem {
        entries: vec![ClipboardEntry::ExternalPaths(ExternalPaths([note.clone(), photo.clone()].into_iter().collect())), ClipboardEntry::from("list.txt\nfridge.jpg".to_owned())],
    });
    cx.simulate_keystrokes("cmd-v");
    cx.run_until_parked();
    assert_eq!(composer.read_with(cx, |composer, _| composer.attached().to_vec()), [attached[0].clone(), note.clone(), photo.clone()]);
    assert_eq!(typed(&composer, cx).0, "[Image 1] [Image 2] ", "the copied picture named; the names of the files not pasted as text");

    // Text pastes as text, as ever.
    cx.write_to_clipboard(ClipboardItem::new_string("What's in the fridge?".into()));
    cx.simulate_keystrokes("cmd-v");
    cx.run_until_parked();
    assert_eq!(typed(&composer, cx).0, "[Image 1] [Image 2] What's in the fridge?");
    assert_eq!(composer.read_with(cx, |composer, _| composer.attached().len()), 3);

    // Send takes the pasted picture with the rest, as a file on this Mac's box.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let sent = stand_in.seen_at("POST", "kuvo/sessions/ses_recipes/send");
    let body = sent.last().and_then(|seen| seen.body.clone()).expect("sent");
    assert_eq!(body["files"].as_array().map(Vec::len), Some(3));
    assert!(body["files"][0]["name"].as_str().unwrap().starts_with("Pasted Image "));
}

#[gpui::test]
fn at_and_ampersand_offer_what_the_box_suggests_and_an_agent_picked_is_named(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let composer = open_session(&workspace, "ses_recipes", cx);

    // "@": the files the box finds for what's typed; Return puts the one picked in.
    cx.simulate_input("Check @sche");
    let offers = composer.read_with(cx, |composer, cx| composer.shown_offers(cx));
    assert_eq!((offers.0.iter().map(|offer| offer.name.as_str()).collect::<Vec<_>>(), offers.1), (vec!["schema.ts"], true));
    cx.simulate_keystrokes("enter");
    assert_eq!(typed(&composer, cx).0, "Check @web/src/signup/schema.ts ");

    // "&": bots and sessions; the one picked is named when it's sent.
    cx.simulate_input("with &mi");
    assert_eq!(composer.read_with(cx, |composer, cx| composer.shown_offers(cx).0.first().map(|offer| offer.name.clone())), Some("Mira".into()));
    cx.simulate_keystrokes("enter");
    assert_eq!(typed(&composer, cx).0, "Check @web/src/signup/schema.ts with &Mira ");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let sent = stand_in.seen_at("POST", "kuvo/sessions/ses_recipes/send").last().and_then(|seen| seen.body.clone()).expect("sent");
    assert_eq!(sent["named"], json!(["ses_mira"]));
    assert_eq!(sent["words"], json!("Check @web/src/signup/schema.ts with &Mira "));
}

#[gpui::test]
fn the_queue_shows_what_waits_and_a_message_taken_back_comes_to_the_composer(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let composer = open_session(&workspace, "ses_signup", cx);
    assert!(bounds(cx, "queue").is_some());
    assert!(bounds(cx, "queued-msg_q1").is_some() && bounds(cx, "queued-msg_q2").is_some());

    // Edit: taken out of the queue, its words back in the composer to change.
    click(cx, "queue-more-msg_q2");
    assert!(bounds(cx, "menu-item-Edit").is_none(), "another agent's letter can't be edited");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    click(cx, "queue-more-msg_q1");
    click(cx, "menu-item-Edit");
    assert_eq!(stand_in.seen_at("DELETE", "kuvo/sessions/ses_signup/queue/msg_q1").len(), 1);
    assert_eq!(typed(&composer, cx).0, "Then do the same for the password field: keep it, and say what's wrong with it");
    assert!(bounds(cx, "queued-msg_q1").is_none());

    // Remove: gone, and the queue with it once nothing waits.
    let remove = bounds(cx, "remove-msg_q2").expect("Remove");
    cx.simulate_click(remove.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(stand_in.seen_at("DELETE", "kuvo/sessions/ses_signup/queue/msg_q2").len(), 1);
    assert!(bounds(cx, "queue").is_none());
}

#[gpui::test]
fn a_command_taken_back_to_edit_comes_back_as_its_box_says(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    // A command queued: its box says Edit puts it back with the command in front, to run again.
    stand_in.set_queue("ses_signup", vec![json!({ "id": "msg_c1", "words": "the tests", "files": [], "keyword": "/review", "from": null, "on": "engine", "delivery": "queue", "held": false, "draft": "/review the tests" })]);
    let (workspace, cx) = open(cx, stand_in.clone());
    let composer = open_session(&workspace, "ses_signup", cx);
    click(cx, "queue-more-msg_c1");
    click(cx, "menu-item-Edit");
    assert_eq!(typed(&composer, cx).0, "/review the tests");
}

/// Runs the window until `done`, waiting (on no clock) for what the follows' own threads hand on:
/// each time it's still not done, until the conversation is told something again.
fn until(cx: &mut VisualTestContext, view: &Entity<kuvo::conversation::ConversationView>, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
    use futures::StreamExt as _;
    cx.executor().allow_parking();
    let mut told = cx.notifications(view);
    // A guard, not a clock the test leans on: a follow that never hands anything on would leave it
    // waiting for good, so the run is ended, saying so, rather than hanging unseen.
    let (finished, guard) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        if guard.recv_timeout(std::time::Duration::from_secs(60)) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
            eprintln!("the live follow handed nothing on for a minute: the test stops here");
            std::process::exit(101);
        }
    });
    let _finished = finished;
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        cx.foreground_executor().block_test(told.next());
    }
}

#[gpui::test]
fn followed_live_an_ask_takes_the_keyboard_as_it_comes_and_the_queue_shows_as_it_changes(cx: &mut TestAppContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let stand_in = StandIn::at(Replies::Never, NOW);
    let mut second = kuvo::stand_in::redis_permission();
    second["id"] = json!("per_push");
    let client = BoxClient::new(stand_in.clone());
    // Live: the box is followed on threads of their own, as the app follows it.
    let window = cx.add_window(move |window, cx| Workspace::new(client, true, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1200.), px(800.)));
    workspace.update_in(cx, |workspace, window, cx| workspace.open_session("ses_limits", window, cx));
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let card = view.read_with(cx, |view, _| view.asks().clone());
    let field = view.read_with(cx, |view, cx| view.composer().read(cx).field().clone());
    until(cx, &view, |cx| view.read_with(cx, |view, _| !view.entries().is_empty()));
    assert!(cx.update(|window, cx| field.focus_handle(cx).is_focused(window)), "nothing asked: the composer has the keyboard");

    // Asks come while you're in the composer: the card takes its place, and the keyboard.
    stand_in.set_asks("ses_limits", vec![kuvo::stand_in::redis_permission(), second]);
    until(cx, &view, |cx| card.read_with(cx, |card, _| card.is_asking()));
    assert!(card_has_keys(&card, cx));

    // Answered: whether the box's word that it's gone comes before the answer's own reply or
    // after it, the next one shows, and takes no key until it's been there a moment.
    quiet(cx);
    cx.simulate_keystrokes("1");
    until(cx, &view, |cx| card.read_with(cx, |card, _| card.place().shown() == Some("per_push")) && stand_in.seen_at("POST", "kuvo/sessions/ses_limits/asks/per_redis").len() == 1);
    cx.simulate_keystrokes("1");
    cx.run_until_parked();
    assert!(stand_in.seen_at("POST", "kuvo/sessions/ses_limits/asks/per_push").is_empty());
    quiet(cx);
    cx.simulate_keystrokes("3");
    until(cx, &view, |cx| !card.read_with(cx, |card, _| card.is_asking()));
    let answered = stand_in.seen_at("POST", "kuvo/sessions/ses_limits/asks/per_push");
    assert_eq!(answered.last().and_then(|seen| seen.body.clone()), Some(json!({ "answer": "reject" })));
    assert!(cx.update(|window, cx| field.focus_handle(cx).is_focused(window)), "the keyboard is back in the composer");

    // The queue, as the box changes it.
    stand_in.set_queue("ses_limits", vec![json!({ "id": "msg_q9", "words": "Then add a test for the header", "files": [], "keyword": null, "from": null, "on": "engine", "delivery": "queue", "held": false })]);
    until(cx, &view, |cx| view.read_with(cx, |view, _| view.queued().len() == 1));
    assert!(bounds(cx, "queued-msg_q9").is_some());
}

/// What's offered is read in the conversation's folder, and in the project a new one starts in, so a
/// model only that project's own settings add is offered there, with its efforts
/// (`models.offered.folder`); a chat's, with no project, is the box's own.
#[gpui::test]
fn whats_offered_is_read_in_the_conversations_folder(cx: &mut TestAppContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let stand_in = StandIn::at(Replies::Never, NOW);
    let folders = |stand_in: &StandIn| -> Vec<Option<String>> {
        stand_in
            .seen_at("GET", "kuvo/models/offered")
            .into_iter()
            .map(|seen| seen.path.split_once('?').and_then(|(_, query)| query.split('&').find_map(|pair| pair.strip_prefix("folder=")).map(|folder| folder.replace("%2F", "/"))))
            .collect()
    };
    let client = BoxClient::new(stand_in.clone());
    let api = Some("/Users/sam/Projects/api".to_owned());
    let conversation = cx.new(|cx| Composer::in_folder(client.clone(), kuvo::composer::Target::Session { id: "ses_limits".into(), place: Where::Engine }, api.clone(), true, cx));
    cx.run_until_parked();
    assert_eq!(folders(&stand_in), vec![api.clone()], "read there from the first read");
    // The same folder again reads nothing more; another (a conversation moved) reads there.
    assert!(!conversation.update(cx, |composer, cx| composer.set_folder(api.clone(), cx)));
    assert!(conversation.update(cx, |composer, cx| composer.set_folder(Some("/Users/sam/Projects/web".into()), cx)));
    cx.run_until_parked();
    assert_eq!(folders(&stand_in), vec![api, Some("/Users/sam/Projects/web".to_owned())]);

    let stand_in = StandIn::at(Replies::Never, NOW);
    let client = BoxClient::new(stand_in.clone());
    let landing = cx.new(|cx| Composer::new(client, kuvo::composer::Target::New(StartIn::Project("/Users/sam/Projects/website".into())), true, cx));
    cx.run_until_parked();
    assert_eq!(folders(&stand_in), vec![Some("/Users/sam/Projects/website".to_owned())]);
    landing.update(cx, |composer, cx| {
        composer.set_start(StartIn::Chat, cx);
        composer.read_models(cx);
    });
    cx.run_until_parked();
    assert_eq!(folders(&stand_in), vec![Some("/Users/sam/Projects/website".to_owned()), None], "a chat: the box's own lists");
}

/// A model gone: Send waits for one for a message, but a "!" command or a goal's pause goes to
/// the box, which says whether it needs the model (they don't).
#[gpui::test]
fn with_a_model_to_choose_a_command_still_goes_to_the_box(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let composer = open_session(&workspace, "ses_limits", cx);
    let gone: ModelState = serde_json::from_value(json!({ "model": { "providerID": "openai", "id": "gpt-4" }, "runs": false, "missing": "gone", "refusal": "gpt-4 is no longer available: choose another model.", "choose": true, "action": { "do": "choose", "label": "Choose a Model", "help": "Choose another model" } })).unwrap();
    composer.update(cx, |composer, cx| composer.take_models(Models { state: Some(gone), ..Models::default() }, cx));

    cx.simulate_input("Use Redis");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(stand_in.seen_at("POST", "kuvo/sessions/ses_limits/send").is_empty());
    assert_eq!(composer.read_with(cx, |composer, _| composer.note().map(|note| note.text.to_string())).as_deref(), Some("gpt-4 is no longer available: choose another model."), "in its box's words");

    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("!ls");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(stand_in.seen_at("POST", "kuvo/sessions/ses_limits/typed").len(), 1, "asked of the box");
}

/// A conversation's composer follows its folder as the box's list has it: what's offered is read
/// there (a conversation moved, or one opened before the list knew it).
#[gpui::test]
fn a_composer_follows_its_conversations_folder_from_the_list(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let composer = open_session(&workspace, "ses_limits", cx);
    let folder = || composer.read_with(cx, |composer, _| composer.folder().map(str::to_owned));
    let was = folder();
    assert!(was.is_some(), "its folder, as the list had it when it opened");
    // Moved to another folder: the list says so, and the composer reads there.
    stand_in.set_facts("ses_limits", |facts| facts["directory"] = json!("/Users/sam/Projects/api-moved"));
    workspace.update(cx, |workspace, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    assert_eq!(composer.read_with(cx, |composer, _| composer.folder().map(str::to_owned)), Some("/Users/sam/Projects/api-moved".to_owned()));
}

#[gpui::test]
fn compaction_shows_its_row_active_status_and_refusals_in_composer(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let composer = open_session(&workspace, "ses_recipes", cx);
    let where_gets_before = stand_in.seen_at("GET", "api/session/ses_recipes").len();

    // Idle conversation: /compact succeeds, shows its note, and adds a compaction row.
    cx.simulate_input("/compact");
    cx.simulate_keystrokes("escape enter");
    cx.run_until_parked();
    assert_eq!(typed(&composer, cx), (String::new(), None));
    assert_eq!(composer.read_with(cx, |composer, _| composer.note().cloned()), Some(kuvo::composer::Note { text: "Asked to summarize the conversation.".into(), warning: false }));
    assert_eq!(stand_in.seen_at("POST", "api/session/ses_recipes/compact").len(), 1);
    assert_eq!(stand_in.seen_at("GET", "api/session/ses_recipes").len(), where_gets_before, "place is known; no extra where_is GET");

    workspace.update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_recipes", window, cx));
    cx.run_until_parked();
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let compact_id = view.read_with(cx, |view, _| {
        view.entries().iter().find_map(|entry| match entry {
            Entry::Message(message) => match message.as_ref() {
                Message::Other { id, kind, raw, .. } if kind == "compaction" => {
                    assert_eq!(raw["status"], "completed");
                    Some(id.clone())
                }
                _ => None,
            },
            _ => None,
        })
    }).expect("compaction entry in ConversationView");
    assert!(bounds(cx, &format!("entry-{compact_id}")).is_some(), "compaction row rendered");

    // While a compaction since the last idle is running, the bottom status row reads "Summarizing the conversation…".
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    let mut active = read_now(&client, "ses_recipes").unwrap();
    let running_compaction: Message = serde_json::from_value(json!({
        "type": "compaction", "id": "msg_cmp_run", "time": { "created": NOW + 100.0 }, "status": "running", "workingTitle": "Summarizing the conversation…", "reason": "manual"
    })).unwrap();
    active.catch_up(vec![running_compaction], None, &[], Some(true));
    view.update(cx, |view, cx| view.take(ConversationUpdate::Conversation(active), cx));
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |view, _| view.entries().last().cloned()), Some(Entry::Working { title: "Summarizing the conversation…".into() }));
    assert!(bounds(cx, "entry-working").is_some(), "summarizing row rendered");

    // A /compact refusal restores the command and shows the error in the composer.
    stand_in.fail_sends();
    cx.simulate_input("/compact");
    cx.simulate_keystrokes("escape enter");
    cx.run_until_parked();
    assert_eq!(typed(&composer, cx), ("/compact".to_owned(), Some("The engine isn't answering. Try again in a moment.".to_owned())));
}

#[gpui::test]
fn a_step_this_app_cant_take_yet_keeps_what_was_written(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let composer = open_session(&workspace, "ses_limits", cx);

    cx.simulate_input("/autopilot");
    cx.simulate_keystrokes("escape enter");
    cx.run_until_parked();
    assert_eq!(typed(&composer, cx), ("/autopilot".to_owned(), None));
    assert_eq!(composer.read_with(cx, |composer, _| composer.note().cloned()), Some(kuvo::composer::Note { text: "Turning Autopilot on or off isn’t in this app yet.".into(), warning: true }));
    assert!(stand_in.seen_at("POST", "kuvo/sessions/ses_limits/send").is_empty());
}

#[gpui::test]
fn attachments_stay_for_non_file_taking_sends_and_new_session_failures(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let folder = std::env::temp_dir().join(format!("kuvo-attach-keep-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let note = folder.join("spec.md");
    std::fs::write(&note, "keep me").unwrap();

    // Existing session: "!" command, /compact, typed refusal, and client-todo keep attached files.
    let recipes = open_session(&workspace, "ses_recipes", cx);
    recipes.update(cx, |composer, cx| composer.attach([note.clone()], cx));
    cx.run_until_parked();

    cx.simulate_input("!ls");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(recipes.read_with(cx, |composer, _| composer.attached().to_vec()), std::slice::from_ref(&note), "! command keeps files");

    cx.simulate_input("/compact");
    cx.simulate_keystrokes("escape enter");
    cx.run_until_parked();
    assert_eq!(recipes.read_with(cx, |composer, _| composer.attached().to_vec()), std::slice::from_ref(&note), "/compact keeps files");

    cx.simulate_input("/loop now");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(recipes.read_with(cx, |composer, _| composer.attached().to_vec()), std::slice::from_ref(&note), "typed refusal keeps files");
    assert_eq!(typed(&recipes, cx).0, "/loop now");
    cx.simulate_keystrokes("cmd-a backspace");

    cx.simulate_input("/autopilot");
    cx.simulate_keystrokes("escape enter");
    cx.run_until_parked();
    assert_eq!(recipes.read_with(cx, |composer, _| composer.attached().to_vec()), std::slice::from_ref(&note), "client-todo keeps files");
    cx.simulate_keystrokes("cmd-a backspace");

    // A typed answer that sends (`/plan ...`) consumes the attached files.
    cx.simulate_input("/plan ship it");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(recipes.read_with(cx, |composer, _| composer.attached().is_empty()), "send consumes files");

    // Target::New: /compact fails after start_session (empty session -> 400); files carry to the opened session.
    workspace.update_in(cx, |workspace, window, cx| workspace.new_session(None, window, cx));
    cx.run_until_parked();
    let landing = workspace.read_with(cx, |workspace, _| workspace.landing().clone());
    let landing_field = landing.read_with(cx, |composer, _| composer.field().clone());
    cx.update(|window, cx| window.focus(&landing_field.focus_handle(cx), cx));
    landing.update(cx, |composer, cx| composer.attach([note.clone()], cx));
    cx.simulate_input("/compact");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(landing.read_with(cx, |composer, _| composer.attached().is_empty()), "landing composer cleared after Started");
    let opened = workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)).expect("new session opens");
    assert!(opened.starts_with("ses_new"));
    let new_composer = workspace.read_with(cx, |workspace, cx| workspace.conversation().unwrap().read(cx).composer().clone());
    assert_eq!(typed(&new_composer, cx), ("/compact".to_owned(), Some("There’s nothing to summarize yet.".to_owned())));
    assert_eq!(new_composer.read_with(cx, |composer, _| composer.attached().to_vec()), std::slice::from_ref(&note), "failed /compact on Target::New restores files");

    // Target::New: "!" command succeeds after start_session; files still stay attached on the opened session.
    workspace.update_in(cx, |workspace, window, cx| workspace.new_session(None, window, cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.focus(&landing_field.focus_handle(cx), cx));
    landing.update(cx, |composer, cx| composer.attach([note.clone()], cx));
    cx.simulate_input("!ls");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let shell_composer = workspace.read_with(cx, |workspace, cx| workspace.conversation().unwrap().read(cx).composer().clone());
    assert_eq!(typed(&shell_composer, cx), (String::new(), None));
    assert_eq!(shell_composer.read_with(cx, |composer, _| composer.attached().to_vec()), std::slice::from_ref(&note), "! command on Target::New keeps files");

    // Target::New: failed plain send after start_session restores both text and files.
    stand_in.fail_sends();
    workspace.update_in(cx, |workspace, window, cx| workspace.new_session(None, window, cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.focus(&landing_field.focus_handle(cx), cx));
    landing.update(cx, |composer, cx| composer.attach([note.clone()], cx));
    cx.simulate_input("Check this file");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let failed_composer = workspace.read_with(cx, |workspace, cx| workspace.conversation().unwrap().read(cx).composer().clone());
    assert_eq!(typed(&failed_composer, cx), ("Check this file".to_owned(), Some("The engine isn't answering. Try again in a moment.".to_owned())));
    assert_eq!(failed_composer.read_with(cx, |composer, _| composer.attached().to_vec()), std::slice::from_ref(&note), "failed send on Target::New restores files");

    std::fs::remove_dir_all(&folder).ok();
}

/// The user's 11-21 case (#948): a bot ran a command, then showed several pictures with show_file,
/// drawn as a strip under the step. A click on one opens it in the viewer, and ← → go among that
/// message's pictures.
#[gpui::test]
fn pictures_shown_after_a_command_open_in_the_viewer(cx: &mut TestAppContext) {
    let png = |width: u32, height: u32| {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(width, height).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    };
    let stand_in = StandIn::at(Replies::Never, NOW);
    let shown = |n: usize, path: &str| json!({ "type": "tool", "id": format!("prt_{n}"), "name": "kuvo-browser_show_file", "state": { "status": "completed", "input": { "path": path }, "content": [] } });
    stand_in.add_message("ses_mira", json!({ "type": "assistant", "id": "msg_d", "time": { "created": NOW - 50_000.0, "completed": NOW - 40_000.0 }, "agent": "build", "finish": "stop", "content": [
        { "type": "tool", "id": "prt_0", "name": "bash", "state": { "status": "completed", "input": { "command": "printf done" }, "output": "done", "metadata": {} } },
        shown(1, "/home/sam/insights-light.png"),
        shown(2, "/home/sam/insights-dark.png"),
        shown(3, "/home/sam/insights-wide.png"),
        { "type": "text", "text": "The drafts are above." },
    ] }));
    for (path, width) in [("/home/sam/insights-light.png", 800), ("/home/sam/insights-dark.png", 640), ("/home/sam/insights-wide.png", 1000)] {
        stand_in.put_file(path, png(width, 500));
    }
    let (workspace, cx) = open(cx, stand_in.clone());
    cx.simulate_resize(size(px(1200.), px(2400.)));
    // In a bot's own conversation, as the user saw it (Kuvo Foreman's).
    for selector in ["rail-bots", "bot-bot_mira"] {
        let at = bounds(cx, selector).unwrap_or_else(|| panic!("{selector}"));
        cx.simulate_click(at.center(), Modifiers::default());
        cx.run_until_parked();
    }
    let view = workspace.read_with(cx, |workspace, _| workspace.bot_chat().expect("Mira's conversation").1.clone());
    let tile = bounds(cx, "picture-msg_d:2-shown").expect("the second picture is drawn in the strip");
    cx.simulate_click(tile.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(bounds(cx, "picture-viewer").is_some(), "a click opens it in the viewer");
    assert_eq!(view.read_with(cx, |view, _| view.viewing().map(str::to_owned)), Some("msg_d:2-shown".into()));
    cx.simulate_keystrokes("right");
    assert_eq!(view.read_with(cx, |view, _| view.viewing().map(str::to_owned)), Some("msg_d:3-shown".into()));
    cx.simulate_keystrokes("left");
    cx.simulate_keystrokes("left");
    assert_eq!(view.read_with(cx, |view, _| view.viewing().map(str::to_owned)), Some("msg_d:1-shown".into()));
    cx.simulate_keystrokes("escape");
    assert!(bounds(cx, "picture-viewer").is_none());
}
