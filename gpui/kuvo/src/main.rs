//! Opens Kuvo. `cargo run -p kuvo` reaches this Mac's own box;
//! `cargo run -p kuvo --features stand-in -- --stand-in` runs against a made-up one, touching nothing real.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, AppContext as _, Bounds, Focusable, TitlebarOptions, WindowBounds, WindowOptions, point, px, size};
use kuvo::launch::{Launch, Opens};
use kuvo::updates::Updates;
use kuvo::{Home, bind_keys};
use kuvo_box::BoxClient;
use kuvo_box::boxes::Boxes;
use kuvo_box::install::Installer;
use kuvo_box::link::WebSocketDial;
use kuvo_box::transport::Transport;
use kuvo_ui::{Appearance, Assets, Theme};

fn main() {
    // Asked for the made-up box in a build without it: said plainly, never this Mac's real box instead.
    if std::env::args().any(|argument| argument == "--stand-in") && !cfg!(feature = "stand-in") {
        eprintln!("This build has no stand-in box. Run it with: cargo run -p kuvo --features stand-in -- --stand-in");
        std::process::exit(2);
    }
    // What this computer holds, as the installer the bundle carries says it, and nothing more: no
    // window, nothing set up, moved or started (`kuvo --install-status`, for checking a built
    // bundle). As when the app opens, `status` first stages the release and the engine in the
    // box's home (`updates/install`): for Kuvo Code itself that's the real `~/.kuvo-code`.
    if std::env::args().any(|argument| argument == "--install-status") {
        std::process::exit(install_status());
    }
    // Whether this build's Sparkle loads as the app would load it, and the feed it names; nothing
    // started, nothing reached (`kuvo --updater-status`: the release and a signed copy check it).
    if std::env::args().any(|argument| argument == "--updater-status") {
        std::process::exit(updater_status());
    }
    gpui_platform::application().with_assets(Assets).run(move |cx: &mut App| {
        bind_keys(cx);
        // Touch ID, or this Mac's password, before what lets something use what's yours from now on.
        kuvo::confirm_you::init(cx);
        // Your settings, kept in the app's own folder (none for the made-up box: kept while it runs).
        kuvo::yours::init(if stand_in() { None } else { your_settings() }, cx);
        // Pictures pasted into a composer, kept beside your settings to attach.
        if let Some(folder) = your_settings().and_then(|settings| settings.parent().map(|folder| folder.join("Pasted"))) {
            cx.set_global(kuvo::pasted::PastedFolder(folder));
        }
        // Who its notifications are from (macOS takes it from the bundle; Linux from this).
        cx.set_app_identity("run.kuvo.code", "Kuvo");
        // Its updates: Sparkle, where the bundle carries it and names a feed (`updates`). The
        // stand-in's app has a made-up updater, so Settings › Updates shows as it would.
        if stand_in() {
            stand_in_updates(cx);
        } else {
            kuvo::updates::start(cx);
        }
        // Its menu bar (Quit with ⌘Q among it, and Check for Updates… where it updates itself), and
        // quitting as its window closes (it has only the one). The first Quit, on a Mac whose Kuvo
        // stays at work in the menu bar, says so first (`kuvo::quit_note`, answered in
        // `app_menu`), remembered in your settings.
        kuvo::app_menu::init(cx);
        cx.on_window_closed(|cx, _| {
            // (The small window beside System Settings isn't the app's own.)
            if !cx.windows().iter().any(|window| window.downcast::<Home>().is_some() || window.downcast::<Launch>().is_some()) {
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
        let opened = if stand_in() {
            let (boxes, first) = stand_in_boxes();
            cx.open_window(options, |window, cx| {
                Theme::init(kuvo::yours::appearance(Appearance::of(window), cx), cx);
                kuvo::yours::follow_system(window);
                cx.new(|cx| Home::new(boxes, first, true, window, cx))
            })
            .map(|window| {
                window
                    .update(cx, |home, window, cx| {
                        let landing = home.workspace().read(cx).landing().focus_handle(cx);
                        window.focus(&landing, cx);
                    })
                    .ok();
            })
        } else {
            // This computer's Kuvo, set up where it isn't yet, then the app.
            let opens = opens();
            cx.open_window(options, |window, cx| {
                Theme::init(kuvo::yours::appearance(Appearance::of(window), cx), cx);
                kuvo::yours::follow_system(window);
                cx.new(|cx| Launch::new(opens, true, window, cx))
            })
            .map(|_| ())
        };
        opened.expect("open Kuvo's window");
        cx.activate(true);
    });
}


/// Asks the installer this build carries what this computer holds (`status`: it sets up, moves and
/// starts nothing, and writes only its own staging in the box's home, `updates/install`), and
/// prints its steps and its answer as they are. 0 where it answered;
/// 1 where this build carries none or may not run it, or it gave no answer.
fn install_status() -> i32 {
    use kuvo_box::install::{Progress, Ran, Verb};
    let installer = match opens().installer {
        Ok(installer) => installer,
        Err(why) => {
            eprintln!("{}", why.0);
            return 1;
        }
    };
    println!("app: {} ({})", installer.carried().bundle.display(), installer.carried().id);
    println!("home: ~/{}", installer.root());
    let ran = installer.run(Verb::Status, &mut |progress| {
        if let Progress::Step(step) = progress {
            println!("step: {}", step.title.unwrap_or(step.step));
        }
    });
    match ran {
        Ran::Answered { exit, answer } => {
            let word = |said: Option<String>| said.unwrap_or_else(|| "none".into());
            println!("exit: {exit}");
            if let Some(result) = answer.result {
                println!("result: {result}");
            }
            println!("verdict: {}", word(answer.verdict));
            println!("verb: {}", word(answer.verb));
            if let Some(message) = answer.message {
                println!("message: {message}");
            }
            0
        }
        Ran::Broken(why) => {
            eprintln!("{why}");
            1
        }
    }
}

#[cfg(target_os = "macos")]
fn updater_status() -> i32 {
    kuvo::updates::sparkle::status()
}

#[cfg(not(target_os = "macos"))]
fn updater_status() -> i32 {
    eprintln!("This build has no updater of its own.");
    1
}

/// Whether it's run against the made-up box (`--stand-in`, with the `stand-in` feature).
fn stand_in() -> bool {
    cfg!(feature = "stand-in") && std::env::args().any(|argument| argument == "--stand-in")
}

/// How this build sets up and reaches this computer's Kuvo: the installer the app's bundle
/// carries (none for a build run outside one, which only attaches), and its own folders. Kuvo Code
/// itself keeps to `~/.kuvo-code`; a copy of the app (a demo's) to the root it's given
/// (`KUVO_DEMO_MACHINE_ROOT`, a folder's name in your home folder) and its own bundle ID.
fn opens() -> Opens {
    let demo_root = std::env::var("KUVO_DEMO_MACHINE_ROOT").ok();
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let installer = std::env::current_exe().map_err(|_| kuvo_box::install::OnlyAttaches("Kuvo couldn’t find where it’s installed.".into())).and_then(|executable| Installer::for_app(&executable, demo_root.as_deref(), &home));
    // Whose folders and keys this build keeps to: a copy of the app never the real Kuvo's.
    let names = std::env::current_exe().ok().and_then(|executable| kuvo_box::install::names_for(&executable, demo_root.as_deref(), &home));
    Opens {
        waits_within: kuvo::launch::WAITS_WITHIN,
        installer,
        attach: Arc::new(move || match &names {
            Some(names) => your_boxes(&names.root, &names.app_id),
            None => Err("This copy of Kuvo has no folder of its own to use.".to_owned()),
        }),
    }
}

/// Where your settings are kept: `settings.json` in this build's own folder (`~/.kuvo-code/desktop`;
/// a copy of the app's in its own root), none where it has no folder of its own.
fn your_settings() -> Option<PathBuf> {
    let demo_root = std::env::var("KUVO_DEMO_MACHINE_ROOT").ok();
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let names = kuvo_box::install::names_for(&std::env::current_exe().ok()?, demo_root.as_deref(), &home)?;
    Some(kuvo::keeper::folder_of(&home, &names.root).join(kuvo::yours::FILE))
}

/// Your boxes: the one on this computer (the nearest), and every other of your computers, each
/// reached directly as a paired device. With none here, the computers kept from the last run; with
/// neither, why the window has nothing to show.
fn your_boxes(root: &str, app_id: &str) -> Result<(Boxes, BoxClient), String> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let found = kuvo_box::place::this_mac_of(&home, root, app_id).ok_or_else(|| "Kuvo isn't set up on this Mac.".to_owned()).and_then(|found| kuvo_box::place::server(&found, "this Mac").map_err(|error| error.to_string()));
    #[cfg(target_os = "macos")]
    let keeper: Arc<dyn kuvo_box::link::Keeper> = Arc::new(kuvo::keeper::Keychain::new(format!("{app_id}.desktop")));
    #[cfg(not(target_os = "macos"))]
    let keeper: Arc<dyn kuvo_box::link::Keeper> = Arc::new(kuvo::keeper::PrivateFile::new(kuvo::keeper::folder_of(&home, root).join("identity")));
    let (nearest, missing) = match found {
        Ok(server) => (Some(Arc::new(server) as Arc<dyn Transport>), None),
        Err(why) => (None, Some(why)),
    };
    let boxes = kuvo::keeper::boxes(&home, root, nearest, keeper, Arc::new(WebSocketDial));
    // The nearest box; with none here, the first of your computers the last run kept.
    let first = boxes.nearest().or_else(|| boxes.rows().iter().find(|row| row.usable()).and_then(|row| boxes.client(row.key())));
    first.map(|first| (boxes, first)).ok_or_else(|| missing.unwrap_or_else(|| "Kuvo isn't set up on this Mac.".to_owned()))
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

/// A made-up updater for the stand-in's app: it updates itself, and was last checked an hour ago.
#[cfg(feature = "stand-in")]
fn stand_in_updates(cx: &mut App) -> gpui::Entity<Updates> {
    let updater = kuvo::updates::stand_in::StandIn::new(kuvo::updates::version(), Some(kuvo::time::now_ms() - 3_600_000.0));
    Updates::install(std::rc::Rc::new(updater), None, cx)
}

#[cfg(not(feature = "stand-in"))]
fn stand_in_updates(cx: &mut App) -> gpui::Entity<Updates> {
    kuvo::updates::start(cx)
}

#[cfg(not(feature = "stand-in"))]
fn stand_in_boxes() -> (Boxes, BoxClient) {
    unreachable!("built without the stand-in box")
}
