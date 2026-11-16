//! Writing to a conversation while its agent works, as 0.7 offered it (#776), against the stand-in
//! box (nothing real is reached): Return steers the message into the run under way, ⌘Return queues
//! it for after the run, the menu beside Send offers both and Send Now; each queued message can be
//! steered in, sent now, edited or removed, and a queue a Stop holds resumed. All through the box's
//! routes, so every client shows the same queue. Keys and clicks go through the window.

use gpui::{Bounds, Entity, Focusable as _, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::composer::Composer;
use kuvo::conversation::ConversationView;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme};
use serde_json::Value;

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
    let at = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} isn't on screen"));
    cx.simulate_click(at.center(), Modifiers::default());
    cx.run_until_parked();
}

/// Opens `session`, the keyboard in its composer.
fn open_session(workspace: &Entity<Workspace>, session: &str, cx: &mut VisualTestContext) -> (Entity<ConversationView>, Entity<Composer>) {
    workspace.update_in(cx, |workspace, window, cx| workspace.open_read_at_once(session, window, cx));
    cx.run_until_parked();
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let composer = view.read_with(cx, |view, _| view.composer().clone());
    let field = composer.read_with(cx, |composer, _| composer.field().clone());
    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    (view, composer)
}

/// The queue as its follow would hand it over now.
fn follow_queue(view: &Entity<ConversationView>, workspace: &Entity<Workspace>, session: &str, cx: &mut VisualTestContext) {
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    let queue = client.queue(session).expect("the queue");
    view.update(cx, |view, cx| view.take_queue(queue, cx));
    cx.run_until_parked();
}

/// What was sent to the conversation, in order: each message's words and how it was to go.
fn sent(stand_in: &StandIn, session: &str) -> Vec<(String, String)> {
    stand_in
        .seen_at("POST", &format!("kuvo/sessions/{session}/send"))
        .into_iter()
        .filter_map(|seen| seen.body)
        .map(|body: Value| (body["words"].as_str().unwrap_or_default().to_owned(), body["delivery"].as_str().unwrap_or_default().to_owned()))
        .collect()
}

#[gpui::test]
fn while_it_works_return_steers_and_cmd_return_queues_for_after_the_run(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let (view, _) = open_session(&workspace, "ses_signup", cx);
    // While it works, the field says what's written goes into its task (0.7's words).
    let placeholder = view.read_with(cx, |view, cx| view.composer().read(cx).field().read(cx).placeholder_text().to_owned());
    assert_eq!(placeholder, "Add to the task…");

    // Return: into the run under way, at the agent's next step.
    cx.simulate_input("Also keep the email field's focus");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    // ⌘Return: for after this run, in the queue over the composer.
    cx.simulate_input("Then write the changelog line");
    cx.simulate_keystrokes("cmd-enter");
    cx.run_until_parked();
    assert_eq!(sent(&stand_in, "ses_signup"), vec![("Also keep the email field's focus".into(), "steer".into()), ("Then write the changelog line".into(), "queue".into())]);
    follow_queue(&view, &workspace, "ses_signup", cx);
    let queued = stand_in.queue_of("ses_signup").last().cloned().expect("queued");
    assert_eq!(queued["words"], "Then write the changelog line");
    assert!(bounds(cx, &format!("queued-{}", queued["id"].as_str().unwrap())).is_some(), "shown in the queue, last");
}

#[gpui::test]
fn with_nothing_running_or_waiting_cmd_return_goes_at_once(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    open_session(&workspace, "ses_recipes", cx);
    cx.simulate_input("Something with lentils?");
    cx.simulate_keystrokes("cmd-enter");
    cx.run_until_parked();
    // Asked for after the run; its box, with nothing to wait for, sends it at once.
    assert_eq!(sent(&stand_in, "ses_recipes"), vec![("Something with lentils?".into(), "queue".into())]);
    assert!(stand_in.queue_of("ses_recipes").is_empty(), "nothing to wait for: it didn't");
    assert!(bounds(cx, "send-choices").is_none(), "no other way to send while nothing runs");
}

#[gpui::test]
fn the_menu_beside_send_queues_or_sends_now(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    open_session(&workspace, "ses_signup", cx);
    assert!(bounds(cx, "send-choices").is_none(), "only once something's written");
    cx.simulate_input("Stop and use the new copy instead");
    cx.run_until_parked();
    click(cx, "send-choices");
    assert!(bounds(cx, "menu-item-Queue for After This Run").is_some());
    click(cx, "menu-item-Send Now");
    // Send Now: one send its box takes as that (for the next step, the step under way stopped for it).
    assert_eq!(sent(&stand_in, "ses_signup"), vec![("Stop and use the new copy instead".into(), "now".into())]);
}

#[gpui::test]
fn a_queued_message_is_steered_in_or_sent_now_through_the_box(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    open_session(&workspace, "ses_signup", cx);
    // Yours, queued in the engine: Steer, while the run goes on.
    assert!(bounds(cx, "go-msg_q1").is_some());
    click(cx, "go-msg_q1");
    assert_eq!(stand_in.queue_asked(), vec!["steer msg_q1"]);
    // For its next step now: it says so, with Send Now.
    assert!(bounds(cx, "next-step-msg_q1").is_some());
    click(cx, "go-msg_q1");
    assert_eq!(stand_in.queue_asked(), vec!["steer msg_q1", "now msg_q1"]);
    assert!(bounds(cx, "queued-msg_q1").is_none(), "gone to the agent");
    // Another agent's letter, held on the box: Send Now.
    click(cx, "go-msg_q2");
    assert_eq!(stand_in.queue_asked().last().map(String::as_str), Some("now msg_q2"));
    assert!(bounds(cx, "queue").is_none(), "nothing left waiting");
}

#[gpui::test]
fn a_queue_a_stop_holds_says_so_and_resumes(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_running("ses_signup", false);
    stand_in.pause_queue("ses_signup", true);
    let (workspace, cx) = open(cx, stand_in.clone());
    let (view, composer) = open_session(&workspace, "ses_signup", cx);
    follow_queue(&view, &workspace, "ses_signup", cx);
    assert!(bounds(cx, "queue-resume").is_some());
    // Nothing running: each goes with Send, not Steer.
    assert!(bounds(cx, "next-step-msg_q1").is_none());
    click(cx, "queue-resume");
    assert_eq!(stand_in.seen_at("POST", "kuvo/sessions/ses_signup/queue/resume").len(), 1);
    assert!(bounds(cx, "queue-resume").is_none(), "going on");
    // With nothing running and messages still waiting, ⌘Return queues it after them.
    let field = composer.read_with(cx, |composer, _| composer.field().clone());
    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    cx.simulate_input("And the docs");
    cx.simulate_keystrokes("cmd-enter");
    cx.run_until_parked();
    assert_eq!(sent(&stand_in, "ses_signup"), vec![("And the docs".into(), "queue".into())]);
}

#[gpui::test]
fn a_right_click_on_a_queued_message_offers_what_can_be_done_with_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    open_session(&workspace, "ses_signup", cx);
    let at = bounds(cx, "queued-msg_q1").expect("the row").center();
    cx.simulate_mouse_down(at, gpui::MouseButton::Right, Modifiers::default());
    cx.simulate_mouse_up(at, gpui::MouseButton::Right, Modifiers::default());
    cx.run_until_parked();
    for item in ["Steer", "Send Now", "Edit", "Remove"] {
        assert!(bounds(cx, &format!("menu-item-{item}")).is_some(), "{item}");
    }
    click(cx, "menu-item-Send Now");
    assert_eq!(stand_in.queue_asked(), vec!["now msg_q1"]);
}
