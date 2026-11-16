//! The bots as the app draws them, on the stand-in box: its Bots sidebar full (ten bots, each in
//! a state its box says, one on a computer that's away) beside a bot's conversation. Saves the
//! window light and dark to `gpui/shots/`, and, given a folder, the frames of a recording at 60 fps
//! (the sidebar and the conversation's header, cropped) while one bot goes from idle to work, to
//! thinking, back to work and to done, is seen, and another is blocked.
//!
//! `cargo run --release -p kuvo --features shots --bin bot_shots [-- <frames folder>]`
//! then `gpui/scripts/frames-to-mp4.swift` makes the frames an MP4.

use std::{path::PathBuf, sync::Arc, time::Duration};

use gpui::{AnyWindowHandle, AppContext as _, HeadlessAppContext, px, size};
use kuvo::Workspace;
use kuvo::area::Area;
use kuvo::stand_in::{Replies, StandIn};
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Assets, Theme};
use serde_json::{Value, json};

const NOW: f64 = 1_794_650_000_000.0;

fn now(state: &str, more: Value) -> Value {
    let mut now = json!({ "working": 0, "asking": 0, "state": state, "since": NOW });
    if let (Some(now), Some(more)) = (now.as_object_mut(), more.as_object()) {
        now.extend(more.clone());
    }
    now
}

/// The box: ten bots in the states a busy afternoon has.
fn stand_in() -> StandIn {
    let stand_in = StandIn::at(Replies::Never, NOW);
    for (id, name, color, shape, face) in [
        ("bot_ada", "Ada", "mint", "loaf", "curious"),
        ("bot_bo", "Bo", "indigo", "capsule", "calm"),
        ("bot_cleo", "Cleo", "amber", "dome", "sleepy"),
        ("bot_dex", "Dex", "pink", "bloom", "bright"),
        ("bot_echo", "Echo", "orange", "squircle", "wink"),
        ("bot_fern", "Fern", "graphite", "round", "happy"),
    ] {
        stand_in.add_bot(id, name, json!({ "color": color, "shape": shape, "face": face }));
    }
    stand_in.set_bot_now("bot_inbox", now("working", json!({ "working": 1 })));
    stand_in.set_bot_now("bot_release", now("idle", json!({})));
    stand_in.set_bot_now("bot_ada", now("thinking", json!({ "working": 1, "thinking": 1 })));
    stand_in.set_bot_now("bot_bo", now("blocked", json!({ "blocked": 1, "blockedBy": "limit" })));
    stand_in.set_bot_now("bot_cleo", now("asleep", json!({})));
    stand_in.set_bot_now("bot_dex", now("working", json!({ "working": 2 })));
    stand_in
}

fn open(cx: &mut HeadlessAppContext, stand_in: &StandIn) -> anyhow::Result<AnyWindowHandle> {
    let client = BoxClient::new(stand_in.clone());
    let window = cx.open_window(size(px(1100.), px(760.)), |window, cx| cx.new(|cx| Workspace::new(client, false, window, cx)))?;
    let any: AnyWindowHandle = window.into();
    cx.update_window(any, |root, window, cx| {
        let workspace = root.downcast::<Workspace>().expect("the workspace");
        workspace.update(cx, |workspace, cx| {
            workspace.set_now(NOW, cx);
            workspace.read_at_once(cx);
            workspace.show_area(Area::Bots, window, cx);
            workspace.open_bot_chat("bot_release", "ses_release", None, window, cx);
        });
    })?;
    cx.run_until_parked();
    Ok(any)
}

/// One frame on: the clock a 60th of a second on, its frame delivered and drawn.
fn tick(cx: &mut HeadlessAppContext, any: AnyWindowHandle) -> anyhow::Result<()> {
    cx.advance_clock(Duration::from_micros(16_667));
    cx.run_until_parked();
    cx.update_window(any, |_, window, cx| window.simulate_next_frame(cx))?;
    cx.run_until_parked();
    cx.update_window(any, |_, window, cx| window.draw(cx).clear(cx))?;
    Ok(())
}

/// What the box says now: Bots reads it again, as on its next line.
fn says(cx: &mut HeadlessAppContext, any: AnyWindowHandle, stand_in: &StandIn, bot: &str, now: Value) -> anyhow::Result<()> {
    stand_in.set_bot_now(bot, now);
    cx.update_window(any, |root, _, cx| {
        let workspace = root.downcast::<Workspace>().expect("the workspace");
        if let Some(view) = workspace.read(cx).area_view(Area::Bots).cloned() {
            view.update(cx, |view, cx| view.shown(cx));
        }
    })?;
    cx.run_until_parked();
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let frames = std::env::args().nth(1).map(PathBuf::from);
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../shots");
    let text_system = gpui_platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(text_system, Arc::new(Assets), gpui_platform::current_headless_renderer);
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    for appearance in [Appearance::Light, Appearance::Dark] {
        cx.update(|cx| Theme::set_appearance(appearance, cx));
        let stand_in = stand_in();
        let any = open(&mut cx, &stand_in)?;
        for _ in 0..180 {
            tick(&mut cx, any)?;
        }
        let image = cx.capture_screenshot(any)?;
        let name = format!("bots-live-{}.png", if appearance.is_dark() { "dark" } else { "light" });
        image.save(out.join(&name))?;
        println!("{name}: {}x{}", image.width(), image.height());
        cx.update_window(any, |_, window, _| window.remove_window())?;
        cx.run_until_parked();
    }
    let Some(frames) = frames else { return Ok(()) };
    for appearance in [Appearance::Light, Appearance::Dark] {
        cx.update(|cx| Theme::set_appearance(appearance, cx));
        let folder = frames.join(if appearance.is_dark() { "dark" } else { "light" });
        std::fs::create_dir_all(&folder)?;
        let stand_in = stand_in();
        let any = open(&mut cx, &stand_in)?;
        for _ in 0..60 {
            tick(&mut cx, any)?;
        }
        // (seconds in, who, what the box says then)
        let script: Vec<(f32, &str, Value)> = vec![
            (1.5, "bot_release", now("working", json!({ "working": 1 }))),
            (5.0, "bot_release", now("thinking", json!({ "working": 1, "thinking": 1 }))),
            (8.0, "bot_release", now("working", json!({ "working": 1 }))),
            (9.0, "bot_echo", now("blocked", json!({ "blocked": 1, "blockedBy": "error" }))),
            (11.0, "bot_release", now("done", json!({ "finished": NOW }))),
            (14.5, "bot_release", now("idle", json!({}))),
        ];
        let mut next = 0;
        for frame in 0..(17 * 60) {
            let at = frame as f32 / 60.;
            while next < script.len() && script[next].0 <= at {
                let (_, bot, said) = &script[next];
                says(&mut cx, any, &stand_in, bot, said.clone())?;
                next += 1;
            }
            tick(&mut cx, any)?;
            let image = cx.capture_screenshot(any)?;
            // The sidebar and the conversation's header.
            let scale = image.width() as f32 / 1100.;
            let crop = image::imageops::crop_imm(&image, 0, 0, (620. * scale) as u32, (560. * scale) as u32).to_image();
            crop.save(folder.join(format!("{frame:05}.png")))?;
        }
        println!("{}: {} frames", folder.display(), 17 * 60);
        cx.update_window(any, |_, window, _| window.remove_window())?;
        cx.run_until_parked();
    }
    Ok(())
}
