//! Measures scrolling a long conversation: 500 messages (your words, replies with lists, inline
//! code, links and code blocks), drawn offscreen with the Mac's own text system, scrolled from
//! the latest to the oldest and back 240 px a frame. Prints the frames' times.
//!
//! `cargo run --release -p kuvo --features shots --bin scroll_bench [-- <frames> [selected]]`
//! (`selected`: with the whole conversation selected, as ⌘A twice leaves it.)

use std::{sync::Arc, time::Instant};

use gpui::{AppContext as _, HeadlessAppContext, px, size};
use kuvo::Workspace;
use kuvo::stand_in::{Replies, StandIn};
use kuvo_box::messages::Message;
use kuvo_box::{BoxClient, Conversation, ConversationUpdate, Where};
use kuvo_ui::{Appearance, Assets, Theme};
use serde_json::json;

const NOW: f64 = 1_794_650_000_000.0;

const REPLY: &str = "I found it. The form clears every field when the server says the email is taken, because the error path calls `reset()` before showing the message.\n\n- Moved the reset into the success path only, so a taken email keeps what was typed.\n- The error now shows **under the email field**, see [the docs](https://kuvo.run).\n- Added a test that signs up twice with the same address.\n\n```ts\nexport async function save(form: Form) {\n  const response = await api.post('/accounts', form);\n  if (response.ok) reset();\n  return response;\n}\n```\n\nBoth test suites pass.";

fn conversation(count: usize) -> Conversation {
    let mut messages = Vec::new();
    for n in 0..count {
        let time = 1_000.0 + n as f64 * 1_000.0;
        let message = if n % 2 == 0 {
            json!({ "type": "user", "id": format!("m{n:04}"), "time": { "created": time }, "text": format!("Message {n}: users say the sign-up form gets wiped when the email is taken. Can you look?") })
        } else {
            json!({ "type": "assistant", "id": format!("m{n:04}"), "time": { "created": time }, "content": [{ "type": "text", "text": REPLY }] })
        };
        messages.push(message);
    }
    let mut newest_first: Vec<Message> = serde_json::from_value(json!(messages)).expect("messages");
    newest_first.reverse();
    let mut conversation = Conversation::new("ses_signup", Where::Engine);
    conversation.catch_up(newest_first, None, &[], Some(false));
    conversation
}

fn main() -> anyhow::Result<()> {
    let frames: usize = std::env::args().nth(1).and_then(|n| n.parse().ok()).unwrap_or(1000);
    let text_system = gpui_platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(text_system, Arc::new(Assets), gpui_platform::current_headless_renderer);
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(StandIn::at(Replies::Never, NOW));
    let window = cx.open_window(size(px(1280.), px(820.)), |window, cx| cx.new(|cx| Workspace::new(client, false, window, cx)))?;
    let any = window.into();
    cx.update_window(any, |root, window, cx| {
        let workspace = root.downcast::<Workspace>().expect("the workspace");
        workspace.update(cx, |workspace, cx| {
            workspace.set_now(NOW, cx);
            workspace.read_at_once(cx);
            workspace.open_read_at_once("ses_signup", window, cx);
            let view = workspace.conversation().expect("a conversation").clone();
            view.update(cx, |view, cx| view.take(ConversationUpdate::Conversation(conversation(500)), cx));
            if std::env::args().nth(2).as_deref() == Some("selected") {
                let last = view.read(cx).entries().len();
                let found = view.update(cx, |view, cx| {
                    let units = view.units();
                    let (first, end) = (units.first().map(|(row, _)| row.clone()), units.last().map(|(row, _)| row.clone()));
                    if let (Some(first), Some(end)) = (first, end) {
                        view.selection().select(kuvo_ui::selection::Spot::row_start(first), kuvo_ui::selection::Spot::row_end(end));
                    }
                    cx.notify();
                    !view.selection().is_empty()
                });
                println!("{last} messages, all selected: {found}");
            }
        });
    })?;
    cx.run_until_parked();
    let mut times = Vec::with_capacity(frames);
    for frame in 0..frames {
        // Up through the conversation for half the frames, then back down.
        let step = if frame < frames / 2 { px(-240.) } else { px(240.) };
        let started = Instant::now();
        cx.update_window(any, |root, window, cx| {
            let workspace = root.downcast::<Workspace>().expect("the workspace");
            let view = workspace.read(cx).conversation().expect("a conversation").clone();
            view.read(cx).list().scroll_by(step);
            window.refresh();
            window.draw(cx).clear(cx);
        })?;
        times.push(started.elapsed().as_secs_f64() * 1000.0);
        if frame + 1 == frames / 2 || frame + 1 == frames {
            cx.update_window(any, |root, _, cx| {
                let workspace = root.downcast::<Workspace>().expect("the workspace");
                let view = workspace.read(cx).conversation().expect("a conversation").clone();
                let top = view.read(cx).list().list_state().logical_scroll_top();
                println!("after {} frames: at message {} of {}", frame + 1, top.item_ix, view.read(cx).entries().len());
            })?;
        }
    }
    // The first frames measure every message they meet for the first time; both are shown.
    let report = |label: &str, times: &[f64]| {
        let mut sorted = times.to_vec();
        sorted.sort_by(f64::total_cmp);
        let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
        let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
        println!("{label}: {} frames, mean {mean:.2} ms, median {:.2} ms, p95 {:.2} ms, max {:.2} ms", sorted.len(), at(0.5), at(0.95), at(1.0));
    };
    report("up (first sight of each message)", &times[..frames / 2]);
    report("down (already measured)", &times[frames / 2..]);
    Ok(())
}
