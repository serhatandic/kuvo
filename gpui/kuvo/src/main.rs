//! Opens Kuvo. `cargo run -p kuvo` reaches this Mac's own box;
//! `cargo run -p kuvo --features stand-in -- --stand-in` runs against a made-up one, touching nothing real.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    App, AppContext as _, Bounds, Context, Focusable, IntoElement, Menu, MenuItem, ParentElement, Render, Styled, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, point, px, size,
};
use kuvo::{Home, Quit, bind_keys};
use kuvo_box::BoxClient;
use kuvo_box::boxes::{Boxes, Options};
use kuvo_box::link::WebSocketDial;
use kuvo_box::transport::Transport;
use kuvo_ui::{ActiveTheme, Appearance, Assets, StyledType, Theme, TypeSize};

fn main() {
    // Asked for the made-up box in a build without it: said plainly, never this Mac's real box instead.
    if std::env::args().any(|argument| argument == "--stand-in") && !cfg!(feature = "stand-in") {
        eprintln!("This build has no stand-in box. Run it with: cargo run -p kuvo --features stand-in -- --stand-in");
        std::process::exit(2);
    }
    let boxes = if stand_in() { Ok(stand_in_boxes()) } else { your_boxes() };
    gpui_platform::application().with_assets(Assets).run(move |cx: &mut App| {
        bind_keys(cx);
        // Its menu, and quitting: with ⌘Q, or as its window closes (it has only the one).
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.set_menus([Menu { name: "Kuvo".into(), items: vec![MenuItem::action("Quit Kuvo", Quit)], disabled: false }]);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1240.), px(820.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("Kuvo".into()), appears_transparent: true, traffic_light_position: Some(point(px(16.), px(16.))) }),
            window_min_size: Some(size(px(720.), px(480.))),
            ..Default::default()
        };
        let opened = match boxes.clone() {
            Ok((boxes, first)) => cx
                .open_window(options, |window, cx| {
                    Theme::init(Appearance::of(window), cx);
                    cx.new(|cx| Home::new(boxes, first, true, window, cx))
                })
                .map(|window| {
                    window
                        .update(cx, |home, window, cx| {
                            let landing = home.workspace().read(cx).landing().focus_handle(cx);
                            window.focus(&landing, cx);
                        })
                        .ok();
                }),
            Err(why) => cx
                .open_window(options, |window, cx| {
                    Theme::init(Appearance::of(window), cx);
                    cx.new(|_| Missing(why))
                })
                .map(|_| ()),
        };
        opened.expect("open Kuvo's window");
        cx.activate(true);
    });
}

/// Whether it's run against the made-up box (`--stand-in`, with the `stand-in` feature).
fn stand_in() -> bool {
    cfg!(feature = "stand-in") && std::env::args().any(|argument| argument == "--stand-in")
}

/// Your boxes: the one on this computer (the nearest), and every other of your computers, each
/// reached directly as a paired device. With none here, the computers kept from the last run; with
/// neither, why the window has nothing to show.
fn your_boxes() -> Result<(Boxes, BoxClient), String> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let found = kuvo_box::place::this_mac(&home).ok_or_else(|| "Kuvo Code isn't set up on this Mac.".to_owned()).and_then(|found| kuvo_box::place::server(&found, "this Mac").map_err(|error| error.to_string()));
    let folder = kuvo::keeper::folder(&home);
    #[cfg(target_os = "macos")]
    let keeper: Arc<dyn kuvo_box::link::Keeper> = Arc::new(kuvo::keeper::Keychain::new("run.kuvo.code.desktop"));
    #[cfg(not(target_os = "macos"))]
    let keeper: Arc<dyn kuvo_box::link::Keeper> = Arc::new(kuvo::keeper::PrivateFile::new(folder.join("identity")));
    let (nearest, missing) = match found {
        Ok(server) => (Some(Arc::new(server) as Arc<dyn Transport>), None),
        Err(why) => (None, Some(why)),
    };
    let boxes = Boxes::new(Options {
        nearest,
        keeper,
        dial: Arc::new(WebSocketDial),
        name: kuvo_box::boxes::device_name(),
        app: env!("CARGO_PKG_VERSION").to_owned(),
        kept: Some(folder.join("boxes.json")),
        timing: Default::default(),
    });
    // The nearest box; with none here, the first of your computers the last run kept.
    let first = boxes.nearest().or_else(|| boxes.rows().iter().find(|row| row.usable()).and_then(|row| boxes.client(row.key())));
    first.map(|first| (boxes, first)).ok_or_else(|| missing.unwrap_or_else(|| "Kuvo Code isn't set up on this Mac.".to_owned()))
}

/// The made-up box; with `--first-run <nothing|crowded|running>`, one whose first run hasn't been
/// had, standing so. Its other computers are made up too, reached as it says (no relay, no key).
#[cfg(feature = "stand-in")]
fn stand_in_boxes() -> (Boxes, BoxClient) {
    use kuvo::stand_in::{Replies, StandIn, first_run::Scenario};
    let arguments: Vec<String> = std::env::args().collect();
    let first_run = arguments.iter().position(|argument| argument == "--first-run").map(|at| {
        let name = arguments.get(at + 1).map(String::as_str).unwrap_or("crowded");
        Scenario::named(name).unwrap_or_else(|| {
            eprintln!("There's no first run “{name}”. Use nothing, crowded or running.");
            std::process::exit(2);
        })
    });
    let stand_in = match first_run {
        Some(scenario) => StandIn::first_run(Replies::Live, kuvo::time::now_ms(), scenario),
        None => StandIn::new(Replies::Live),
    };
    let boxes = kuvo::home::stand_in_boxes(stand_in);
    let first = boxes.nearest().expect("the stand-in is the nearest box");
    (boxes, first)
}

#[cfg(not(feature = "stand-in"))]
fn stand_in_boxes() -> (Boxes, BoxClient) {
    unreachable!("built without the stand-in box")
}

/// What the window says when there's no box to reach.
struct Missing(String);

impl Render for Missing {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = &cx.theme().colors;
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .bg(c.background)
            .child(div().text_ui(TypeSize::Body).text_color(c.text).child(self.0.clone()))
            .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child("Open Kuvo Code on this Mac, then open Kuvo again."))
    }
}
