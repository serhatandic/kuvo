//! A message is never a dead end (docs/architecture/message-delivery.md, rule 1): written while its
//! computer drops every request, each is taken at once, waits in order under its own id, and goes
//! by itself, once, when the computer answers. Send is never held for one on its way, and nothing
//! asks you to Retry. The boxes are stand-ins, with the owner made to drop every request; keys go through the
//! window, and time is the test's own clock.

use std::time::Duration;

use gpui::{Entity, Focusable as _, TestAppContext, VisualTestContext, px, size};
use kuvo::composer::Composer;
use kuvo::conversation::ConversationView;
use kuvo::entries::Entry;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme};
use serde_json::Value;

const NOW: f64 = 1_794_650_000_000.0;
const SESSION: &str = "ses_releases";

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

fn open_session(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> (Entity<ConversationView>, Entity<Composer>) {
    workspace.update_in(cx, |workspace, window, cx| workspace.open_read_at_once(SESSION, window, cx));
    cx.run_until_parked();
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let composer = view.read_with(cx, |view, _| view.composer().clone());
    let field = composer.read_with(cx, |composer, _| composer.field().clone());
    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    (view, composer)
}

/// The words its box was sent, in order.
fn delivered(stand_in: &StandIn) -> Vec<String> {
    stand_in.seen_at("POST", &format!("kuvo/sessions/{SESSION}/send")).into_iter()
        .filter_map(|seen| seen.body)
        .map(|body: Value| body["words"].as_str().unwrap_or_default().to_owned())
        .collect()
}

fn wait(cx: &mut VisualTestContext, long: Duration) {
    cx.executor().advance_clock(long);
    cx.run_until_parked();
}

fn read_queue(stand_in: &StandIn, view: &Entity<ConversationView>, cx: &mut VisualTestContext) {
    let client = BoxClient::new(stand_in.clone());
    let queue = client.get(&format!("kuvo/sessions/{SESSION}/delivery?computer=key_devbox")).unwrap();
    view.update(cx, |view, cx| view.take_queue(queue, cx));
    cx.run_until_parked();
}

/// Nothing in the conversation says a message of yours didn't go.
fn none_failed(view: &Entity<ConversationView>, cx: &mut VisualTestContext) -> bool {
    view.read_with(cx, |view, _| view.entries().iter().all(|entry| !matches!(entry, Entry::Outgoing { failed: Some(_), .. })))
}

#[gpui::test]
fn five_messages_written_while_its_computer_drops_every_request_all_go_once_in_order(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.share_setup_lists();
    stand_in.enable_delivery();
    let remote = stand_in.peer_box("key_devbox").unwrap();
    let (workspace, cx) = open(cx, stand_in.clone());
    let (view, composer) = open_session(&workspace, cx);
    remote.set_down(true);

    // Two minutes of its computer dropping every request, five messages written meanwhile.
    for n in 1..=5 {
        cx.simulate_input(&format!("Message {n}"));
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        // Taken at once, Send free for the next: never held for the one before.
        let (written, sending) = composer.read_with(cx, |composer, cx| (composer.field().read(cx).text().to_owned(), composer.is_sending()));
        assert_eq!((written.as_str(), sending), ("", false), "message {n} was taken and Send is free");
        stand_in.delivery_round();
        read_queue(&stand_in, &view, cx);
        wait(cx, Duration::from_secs(24));
    }
    assert!(delivered(&remote).is_empty(), "nothing reached it");
    let local = stand_in.seen_at("POST", &format!("kuvo/sessions/{SESSION}/delivery"));
    assert_eq!(local.len(), 5, "each message was handed to the local box once");
    assert!(local.iter().all(|request| request.path.ends_with("?computer=key_devbox")));
    assert_eq!(view.read_with(cx, |view, _| view.queued().iter().map(|row| row.words.clone()).collect::<Vec<_>>()),
        (1..=5).map(|n| format!("Message {n}")).collect::<Vec<_>>());
    assert!(none_failed(&view, cx), "nothing says Not sent");
    let note = composer.read_with(cx, |composer, _| composer.note().map(|note| note.text.to_string()));
    assert!(note.as_deref().is_some_and(|note| note.ends_with("isn’t answering · your messages will go when it’s back")), "one quiet line: {note:?}");

    // It answers again: every one goes by itself, once, in the order written.
    remote.set_down(false);
    stand_in.delivery_round();
    read_queue(&stand_in, &view, cx);
    for _ in 0..4 {
        wait(cx, Duration::from_secs(30));
    }
    assert_eq!(delivered(&remote), (1..=5).map(|n| format!("Message {n}")).collect::<Vec<_>>());
    assert!(none_failed(&view, cx));
    assert_eq!(composer.read_with(cx, |composer, _| composer.note().cloned()), None, "the line goes once it answers");
}
