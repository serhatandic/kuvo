//! Measures what live bots cost in the app: the real window, its Bots sidebar full (ten bots at
//! 28 pt) beside a bot's conversation of 500 messages (its 22 pt header) or its page (64 pt), on the stand-in box,
//! drawn offscreen with the Mac's own text system. A minute of the test clock at 60 Hz (and one at
//! 120 Hz), a frame delivered each tick: the window is drawn only on a tick where a bot asked for a
//! frame or a bot's timer woke. Prints a table: frames drawn, the longest still gap, and the CPU
//! time of each whole-window draw.
//!
//! `cargo run --release -p kuvo --features shots --bin bot_bench`

use std::{sync::Arc, time::Duration, time::Instant};

use gpui::{AnyWindowHandle, AppContext as _, HeadlessAppContext, px, size};
use kuvo::Workspace;
use kuvo::area::Area;
use kuvo::stand_in::{Replies, StandIn};
use kuvo_box::messages::Message;
use kuvo_box::{BoxClient, Conversation, ConversationUpdate, Where};
use kuvo_ui::{Appearance, Assets, Theme};
use serde_json::json;

const NOW: f64 = 1_794_650_000_000.0;

const REPLY: &str = "I found it. The form clears every field when the server says the email is taken, because the error path calls `reset()` before showing the message.\n\n- Moved the reset into the success path only, so a taken email keeps what was typed.\n- The error now shows **under the email field**, see [the docs](https://kuvo.run).\n\n```ts\nexport async function save(form: Form) {\n  const response = await api.post('/accounts', form);\n  if (response.ok) reset();\n  return response;\n}\n```\n\nBoth test suites pass.";

/// The bot's conversation, long: `count` messages, yours and its replies with lists and code.
fn conversation(count: usize) -> Conversation {
    let messages: Vec<serde_json::Value> = (0..count)
        .map(|n| {
            let time = 1_000.0 + n as f64 * 1_000.0;
            if n % 2 == 0 {
                json!({ "type": "user", "id": format!("m{n:04}"), "time": { "created": time }, "text": format!("Message {n}: can you check the release notes?") })
            } else {
                json!({ "type": "assistant", "id": format!("m{n:04}"), "time": { "created": time }, "content": [{ "type": "text", "text": REPLY }] })
            }
        })
        .collect();
    let mut newest_first: Vec<Message> = serde_json::from_value(json!(messages)).expect("messages");
    newest_first.reverse();
    let mut conversation = Conversation::new("ses_release", Where::Engine);
    conversation.catch_up(newest_first, None, &[], Some(false));
    conversation
}

const MORE: [(&str, &str, &str, &str); 6] = [
    ("bot_ada", "Ada", "mint", "loaf"),
    ("bot_bo", "Bo", "indigo", "capsule"),
    ("bot_cleo", "Cleo", "amber", "dome"),
    ("bot_dex", "Dex", "pink", "bloom"),
    ("bot_echo", "Echo", "orange", "squircle"),
    ("bot_fern", "Fern", "graphite", "round"),
];

#[derive(Clone, Copy)]
enum Shows {
    /// A bot's conversation: its header's copy beside the rows.
    Chat,
    /// A bot's page: its 64 pt copy beside the rows.
    Page,
}

fn main() -> anyhow::Result<()> {
    let text_system = gpui_platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(text_system, Arc::new(Assets), gpui_platform::current_headless_renderer);
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let idle = json!({ "working": 0, "asking": 0, "state": "idle", "since": NOW });
    let scenarios: [(&str, Option<(&str, serde_json::Value)>, Shows, bool, u32); 6] = [
        ("All idle", None, Shows::Chat, false, 60),
        ("One working", Some(("bot_release", json!({ "working": 1, "asking": 0, "state": "working", "since": NOW }))), Shows::Chat, false, 60),
        ("One working, 120 Hz display", Some(("bot_release", json!({ "working": 1, "asking": 0, "state": "working", "since": NOW }))), Shows::Chat, false, 120),
        ("One thinking (Orbit)", Some(("bot_release", json!({ "working": 1, "asking": 0, "thinking": 1, "state": "thinking", "since": NOW }))), Shows::Chat, false, 60),
        ("One asleep, on its 64 pt page", Some(("bot_release", json!({ "working": 0, "asking": 0, "state": "asleep", "since": NOW }))), Shows::Page, false, 60),
        ("All idle, Reduce Motion", None, Shows::Chat, true, 60),
    ];
    println!("| Scenario | Frames drawn in 60 s | Longest still gap | Draw (CPU) avg / p95 / max |\n|---|---|---|---|");
    for (label, change, shows, reduced, hz) in scenarios {
        let stand_in = StandIn::at(Replies::Never, NOW);
        for (id, name, color, shape) in MORE {
            stand_in.add_bot(id, name, json!({ "color": color, "shape": shape, "face": "calm" }));
        }
        for bot in ["bot_mira", "bot_inbox", "bot_release", "bot_docs"] {
            stand_in.set_bot_now(bot, idle.clone());
        }
        if let Some((bot, now)) = &change {
            stand_in.set_bot_now(bot, now.clone());
        }
        cx.update(|cx| cx.set_reduce_motion(reduced));
        let client = BoxClient::new(stand_in);
        let window = cx.open_window(size(px(1280.), px(820.)), |window, cx| cx.new(|cx| Workspace::new(client, false, window, cx)))?;
        let any: AnyWindowHandle = window.into();
        cx.update_window(any, |root, window, cx| {
            let workspace = root.downcast::<Workspace>().expect("the workspace");
            workspace.update(cx, |workspace, cx| {
                workspace.set_now(NOW, cx);
                workspace.read_at_once(cx);
                workspace.show_area(Area::Bots, window, cx);
                if let Shows::Chat = shows {
                    workspace.open_bot_chat("bot_release", "ses_release", None, window, cx);
                    // Its conversation long, as a bot's gets: what a frame of the window redraws.
                    if let Some((_, view)) = workspace.bot_chat() {
                        view.clone().update(cx, |view, cx| view.take(ConversationUpdate::Conversation(conversation(500)), cx));
                    }
                }
            });
        })?;
        if let Shows::Page = shows {
            cx.update_window(any, |root, _, cx| {
                let workspace = root.downcast::<Workspace>().expect("the workspace");
                let view = workspace.read(cx).area_view(Area::Bots).cloned().expect("Bots");
                view.update(cx, |view, cx| view.select(Some("bot:bot_release".into()), cx));
            })?;
        }
        cx.run_until_parked();
        cx.update_window(any, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })?;
        // Two seconds to settle in (what's first drawn starts settled; this passes its first glance).
        let tick = Duration::from_secs_f64(1. / hz as f64);
        let counted = |cx: &mut HeadlessAppContext| cx.update(|cx| {
            let faces = kuvo::bot_face::faces(cx);
            let faces = faces.borrow();
            faces.frames + faces.wakes
        });
        let step = |cx: &mut HeadlessAppContext| -> anyhow::Result<Option<f64>> {
            let before = counted(cx);
            cx.advance_clock(tick);
            cx.run_until_parked();
            cx.update_window(any, |_, window, cx| window.simulate_next_frame(cx))?;
            cx.run_until_parked();
            if counted(cx) == before {
                return Ok(None);
            }
            let started = Instant::now();
            cx.update_window(any, |_, window, cx| window.draw(cx).clear(cx))?;
            Ok(Some(started.elapsed().as_secs_f64() * 1000.))
        };
        for _ in 0..2 * hz {
            step(&mut cx)?;
        }
        let (mut times, mut gap, mut longest) = (Vec::new(), 0u32, 0u32);
        for _ in 0..60 * hz {
            match step(&mut cx)? {
                Some(ms) => {
                    times.push(ms);
                    longest = longest.max(gap);
                    gap = 0;
                }
                None => gap += 1,
            }
        }
        longest = longest.max(gap);
        times.sort_by(f64::total_cmp);
        let avg = if times.is_empty() { 0. } else { times.iter().sum::<f64>() / times.len() as f64 };
        let p95 = times.get(times.len().saturating_sub(1) * 95 / 100).copied().unwrap_or(0.);
        let max = times.last().copied().unwrap_or(0.);
        println!("| {label} | {} ({:.1}/s) | {:.1} s | {avg:.2} / {p95:.2} / {max:.2} ms |", times.len(), times.len() as f64 / 60., longest as f64 / hz as f64);
        cx.update_window(any, |_, window, _| window.remove_window())?;
        cx.run_until_parked();
    }
    Ok(())
}
