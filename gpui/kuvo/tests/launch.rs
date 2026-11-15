//! The app opening on a computer, as someone meets it: a new Mac set up, one whose Kuvo is here
//! already, an earlier Kuvo Code's sessions the box won't move yet, a box that doesn't start, and a
//! build that never installs. The installer is a stand-in (a shell script in a bundle of the
//! test's own, saying what each test tells it to) and so is the box: nothing real is installed,
//! started or reached. Clicks and keys go through the window; waiting is on the test's own clock.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::launch::{Attached, Launch, Opens, Stage};
use kuvo::stand_in::first_run::Scenario;
use kuvo::stand_in::{Replies, StandIn};
use kuvo_box::install::stand_in::Bundle;
use kuvo_box::install::{Installer, OnlyAttaches, Progress, Step, Verb};
use kuvo_ui::{Appearance, Theme};

const NOW: f64 = 1_794_650_000_000.0;
const TRIAL: &str = "run.kuvo.code.trial713.p5";
const ROOT: &str = ".kuvo-trial713-p5";

/// Whether this computer's box answers yet, as the test says: none, or the stand-in.
#[derive(Clone, Default)]
struct Here {
    stand_in: Arc<Mutex<Option<StandIn>>>,
    asked: Arc<Mutex<usize>>,
}

impl Here {
    fn answers(&self, stand_in: &StandIn) {
        *self.stand_in.lock().unwrap() = Some(stand_in.clone());
    }

    fn asked(&self) -> usize {
        *self.asked.lock().unwrap()
    }

    fn attach(&self) -> Arc<dyn Fn() -> Attached + Send + Sync> {
        let here = self.clone();
        Arc::new(move || {
            *here.asked.lock().unwrap() += 1;
            match here.stand_in.lock().unwrap().clone() {
                Some(stand_in) => {
                    let boxes = kuvo::home::stand_in_boxes(stand_in);
                    let first = boxes.nearest().expect("the stand-in is the nearest box");
                    Ok((boxes, first))
                }
                None => Err("Kuvo Code isn't set up on this Mac.".into()),
            }
        })
    }
}

fn open<'a>(cx: &'a mut TestAppContext, installer: Result<Installer, OnlyAttaches>, here: &Here) -> (Entity<Launch>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let opens = Opens { installer, attach: here.attach() };
    let window = cx.add_window(move |window, cx| Launch::new(opens, false, window, cx));
    let launch = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    (launch, cx)
}

fn carried(folder: &Path) -> (Bundle, Result<Installer, OnlyAttaches>) {
    let bundle = Bundle::make(folder, TRIAL);
    let installer = Installer::for_app(&bundle.executable(), Some(ROOT));
    assert!(installer.is_ok());
    (bundle, installer)
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

fn wait(cx: &mut VisualTestContext, how_long: Duration) {
    cx.executor().advance_clock(how_long);
    cx.run_until_parked();
}

fn line(launch: &Entity<Launch>, cx: &mut VisualTestContext) -> String {
    launch.read_with(cx, |launch, _| launch.line().0.to_string())
}

fn message(launch: &Entity<Launch>, cx: &mut VisualTestContext) -> Option<String> {
    launch.read_with(cx, |launch, _| launch.message().map(|message| message.to_string()))
}

/// The app as it opened, read at once (nothing here follows a box on a thread).
fn opened(launch: &Entity<Launch>, cx: &mut VisualTestContext) -> bool {
    let Some(home) = launch.read_with(cx, |launch, _| launch.home().cloned()) else { return false };
    home.update_in(cx, |home, _, cx| {
        home.set_now(NOW, cx);
        home.read_at_once(cx);
    });
    cx.run_until_parked();
    true
}

fn verbs(bundle: &Bundle) -> Vec<String> {
    bundle.asked().iter().map(|line| line.split(' ').next().unwrap_or_default().to_owned()).collect()
}

const FRESH: &str = r#"{"verdict":"fresh","verb":"new","status":{}}"#;
const DONE: &str = r#"{"result":"done","verdict":"ours","started":true,"status":{}}"#;

#[gpui::test]
fn a_new_mac_is_set_up_and_lands_in_the_welcome(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    bundle.says("status", None, 0, &[FRESH]).says("new", None, 0, &[r#"{"step":"checking"}"#, r#"{"step":"engine"}"#, DONE]);
    let box_here = StandIn::first_run(Replies::Never, NOW, Scenario::Nothing);
    let here = Here::default();
    here.answers(&box_here);
    let (launch, cx) = open(cx, installer, &here);
    // The installer was asked what's here, then to make what it named, and nothing else.
    assert_eq!(verbs(&bundle), ["status", "new"]);
    // Its box answers: the app is open on the first run's welcome, in the same card.
    assert!(opened(&launch, cx));
    assert!(shown(cx, "first-run-card") && shown(cx, "first-run-computer") && !shown(cx, "launch-computer"));
    // The release the app carries was offered to the box, once.
    let offered = box_here.seen_at("POST", "kuvo/box/update");
    assert_eq!(offered.len(), 1);
    assert_eq!(offered[0].body.as_ref().and_then(|body| body["from"].as_str()), bundle.path.join("Contents/Resources/box-release").to_str());
}

#[gpui::test]
fn setting_up_is_said_on_this_mac_s_row_in_the_welcome_card(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    bundle.says("status", None, 0, &[FRESH]).says("new", None, 0, &[DONE]);
    let here = Here::default();
    let (launch, cx) = open(cx, installer, &here);
    // Made and started, its server not up yet: the card waits, on the same row.
    assert!(matches!(launch.read_with(cx, |launch, _| launch.stage().clone()), Stage::Starting));
    assert!(shown(cx, "first-run-card") && shown(cx, "launch-computer") && !shown(cx, "launch-again"));
    assert_eq!(line(&launch, cx), "Starting…");
    // While the installer works, the row says so; with the box's words for a step once it sends any.
    launch.update(cx, |launch, cx| {
        launch.progress(Progress::Running { verb: Verb::New, verdict: Some("fresh".into()) }, cx);
        launch.progress(Progress::Step(Step { step: "checking".into(), ..Step::default() }), cx);
    });
    assert_eq!(line(&launch, cx), "Setting up Kuvo…");
    launch.update(cx, |launch, cx| launch.progress(Progress::Step(Step { step: "engine".into(), title: Some("Downloading the engine".into()), ..Step::default() }), cx));
    assert_eq!(line(&launch, cx), "Setting up Kuvo · Downloading the engine");
    cx.run_until_parked();
    assert!(shown(cx, "launch-computer") && shown(cx, "launch-next"));
}

#[gpui::test]
fn a_box_that_takes_a_moment_to_start_is_waited_for(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    bundle.says("status", None, 0, &[FRESH]).says("new", None, 0, &[DONE]);
    let here = Here::default();
    let (launch, cx) = open(cx, installer, &here);
    assert!(!opened(&launch, cx));
    wait(cx, Duration::from_secs(3));
    assert!(!opened(&launch, cx) && shown(cx, "launch-computer"));
    // Its server is up: the next ask finds it.
    here.answers(&StandIn::first_run(Replies::Never, NOW, Scenario::Nothing));
    wait(cx, Duration::from_millis(500));
    assert!(opened(&launch, cx));
    assert!(shown(cx, "first-run-computer"));
    assert_eq!(verbs(&bundle), ["status", "new"]);
}

#[gpui::test]
fn a_box_that_never_starts_is_said_and_tried_again(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    bundle.says("status", None, 0, &[FRESH]).says("new", None, 0, &[DONE]);
    let here = Here::default();
    let (launch, cx) = open(cx, installer, &here);
    wait(cx, Duration::from_secs(61));
    assert_eq!(line(&launch, cx), "Kuvo didn’t start");
    assert!(shown(cx, "launch-message") && shown(cx, "launch-again") && shown(cx, "first-run-card"));
    // Try Again asks the installer from the start: by now it says the box is there, and only
    // attaches.
    bundle.says("status", None, 0, &[r#"{"verdict":"ours","verb":null,"status":{}}"#]);
    here.answers(&StandIn::first_run(Replies::Never, NOW, Scenario::Nothing));
    click(cx, "launch-again");
    assert_eq!(verbs(&bundle), ["status", "new", "status"]);
    assert!(opened(&launch, cx));
}

#[gpui::test]
fn what_the_box_refuses_is_said_in_its_words_with_try_again(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    let offline = "The engine couldn't be downloaded. Check that this Mac is online, then try again.";
    bundle.says("status", None, 0, &[FRESH]).says("new", None, 1, &[r#"{"step":"engine"}"#, &format!(r#"{{"result":"failed","message":"{offline}"}}"#)]);
    let here = Here::default();
    let (launch, cx) = open(cx, installer, &here);
    assert_eq!(line(&launch, cx), "Couldn’t set up Kuvo");
    assert_eq!(message(&launch, cx).as_deref(), Some(offline));
    assert!(shown(cx, "first-run-card") && shown(cx, "launch-message") && shown(cx, "launch-again"));
    // Return is Try Again. Online by then: it's made, and the app opens.
    bundle.says("new", None, 0, &[DONE]);
    here.answers(&StandIn::first_run(Replies::Never, NOW, Scenario::Nothing));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(verbs(&bundle), ["status", "new", "status", "new"]);
    assert!(opened(&launch, cx));
}

#[gpui::test]
fn a_move_the_box_refuses_is_said_in_its_words(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    let not_yet = "This Mac has 2.1 GB free, and moving needs 6.4 GB. Free up some space, then try again.";
    bundle.says("status", None, 0, &[r#"{"verdict":"app-07","verb":"move","status":{}}"#]);
    bundle.says("move", None, 1, &[&format!(r#"{{"result":"refused","message":"{not_yet}","verdict":"app-07","verb":"move","status":{{}}}}"#)]);
    let here = Here::default();
    let (launch, cx) = open(cx, installer, &here);
    assert_eq!(verbs(&bundle), ["status", "move"]);
    assert_eq!(line(&launch, cx), "Couldn’t move your sessions");
    assert_eq!(message(&launch, cx).as_deref(), Some(not_yet));
    // Said plainly, as nothing of the earlier Kuvo Code runs meanwhile.
    assert!(shown(cx, "launch-again") && shown(cx, "launch-note") && !opened(&launch, cx));
}

#[gpui::test]
fn an_earlier_kuvo_code_s_sessions_are_moved_and_the_app_opens(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    bundle.says("status", None, 0, &[r#"{"verdict":"app-07","verb":"move","status":{}}"#]);
    bundle.says("move", None, 0, &[r#"{"step":"plan","title":"Getting ready","index":1,"of":12}"#, r#"{"step":"done","title":"Starting Kuvo on this Mac","index":12,"of":12}"#, DONE]);
    let here = Here::default();
    here.answers(&StandIn::at(Replies::Never, NOW));
    let (launch, cx) = open(cx, installer, &here);
    assert_eq!(verbs(&bundle), ["status", "move"]);
    assert!(opened(&launch, cx));
    assert!(shown(cx, "rail-settings"));
}

#[gpui::test]
fn a_move_still_at_work_from_an_earlier_open_is_drawn_where_it_is_until_its_done(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    // The app quit while the box moved: the move went on by itself, and says where it is.
    let waits = |step: &str, title: &str, index: u32| format!(r#"{{"verdict":"waits","verb":null,"status":{{"progress":{{"step":"{step}","title":"{title}","index":{index},"of":12}}}}}}"#);
    bundle.says("status", None, 0, &[&waits("clone", "Copying your sessions and settings", 3)]);
    let here = Here::default();
    let (launch, cx) = open(cx, installer, &here);
    assert!(shown(cx, "first-run-card") && shown(cx, "launch-steps") && !shown(cx, "launch-again"));
    assert_eq!(line(&launch, cx), "Moving your sessions · Copying your sessions and settings");
    assert_eq!(launch.read_with(cx, |launch, _| launch.at()), Some((3, 12)));
    // Asked again in a moment: further on, the same card.
    bundle.says("status", None, 0, &[&waits("check", "Checking the copy", 8)]);
    wait(cx, Duration::from_secs(2));
    assert_eq!(line(&launch, cx), "Moving your sessions · Checking the copy");
    assert_eq!(launch.read_with(cx, |launch, _| launch.at()), Some((8, 12)));
    // Done: the box says this Mac's Kuvo is here, and the app opens. `move` was never run by
    // this open of the app: the box's own run finished it.
    bundle.says("status", None, 0, &[r#"{"verdict":"ours","verb":null,"status":{}}"#]);
    here.answers(&StandIn::at(Replies::Never, NOW));
    wait(cx, Duration::from_secs(2));
    assert!(opened(&launch, cx));
    assert_eq!(verbs(&bundle), ["status", "status", "status"]);
}

#[gpui::test]
fn a_box_an_earlier_build_still_runs_is_used_while_it_cant_be_moved(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    bundle.says("status", None, 0, &[r#"{"verdict":"app-box","verb":"move","status":{}}"#]);
    bundle.says("move", None, 1, &[r#"{"result":"refused","message":"Not in this version yet.","verdict":"app-box","verb":"move","status":{}}"#]);
    let here = Here::default();
    here.answers(&StandIn::at(Replies::Never, NOW));
    let (launch, cx) = open(cx, installer, &here);
    assert_eq!(verbs(&bundle), ["status", "move"]);
    assert!(opened(&launch, cx));
    assert!(shown(cx, "rail-settings") && !shown(cx, "launch"));
}

#[gpui::test]
fn a_mac_whose_kuvo_is_here_opens_with_no_card(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    bundle.says("status", None, 0, &[r#"{"verdict":"ours","verb":null,"status":{}}"#]);
    let here = Here::default();
    here.answers(&StandIn::at(Replies::Never, NOW));
    let (launch, cx) = open(cx, installer, &here);
    assert_eq!(verbs(&bundle), ["status"]);
    assert_eq!(here.asked(), 1);
    assert!(opened(&launch, cx));
    assert!(shown(cx, "rail-settings") && !shown(cx, "first-run-card"));
}

#[gpui::test]
fn a_mac_whose_kuvo_is_here_but_isnt_running_says_so_quietly(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    bundle.says("status", None, 0, &[r#"{"verdict":"ours","verb":null,"status":{}}"#]);
    let here = Here::default();
    let (launch, cx) = open(cx, installer, &here);
    // No welcome for someone who's had it: the window's quiet middle, why, and Try Again.
    assert!(!shown(cx, "first-run-card") && shown(cx, "launch-message") && shown(cx, "launch-again"));
    assert_eq!(message(&launch, cx).as_deref(), Some("Kuvo Code isn't set up on this Mac."));
    // Its helper wasn't in place: `new` starts it (the box's word), still with no card.
    bundle.says("status", None, 0, &[r#"{"verdict":"ours","verb":"new","status":{}}"#]).says("new", None, 0, &[r#"{"result":"nothing","verdict":"ours","started":true,"status":{}}"#]);
    click(cx, "launch-again");
    assert_eq!(verbs(&bundle), ["status", "status", "new"]);
    assert!(!shown(cx, "first-run-card"));
    here.answers(&StandIn::at(Replies::Never, NOW));
    wait(cx, Duration::from_millis(500));
    assert!(opened(&launch, cx));
}

#[gpui::test]
fn an_installer_that_cant_be_read_is_said_plainly(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    bundle.says("status", None, 2, &[]).complains("status", "Usage: sh kuvo-install status|new|move");
    let here = Here::default();
    let (launch, cx) = open(cx, installer, &here);
    let said = message(&launch, cx).unwrap_or_default();
    assert!(said.starts_with("Kuvo couldn’t be set up on this") && said.contains("Usage: sh kuvo-install"), "{said}");
    assert!(shown(cx, "launch-again"));
    assert_eq!(verbs(&bundle), ["status"]);
}

#[gpui::test]
fn a_build_that_isnt_the_app_in_its_bundle_only_attaches(cx: &mut TestAppContext) {
    // A dev build: this test's own program, in no bundle.
    let dev = Installer::for_app(&std::env::current_exe().unwrap(), None);
    assert!(dev.is_err());
    // Nothing here: it says so, and that this build installs nothing. Nothing was installed.
    let here = Here::default();
    let (launch, cx) = open(cx, dev.clone(), &here);
    assert_eq!(message(&launch, cx).as_deref(), Some("Kuvo Code isn't set up on this Mac."));
    assert!(shown(cx, "launch-note") && shown(cx, "launch-again") && !shown(cx, "first-run-card"));
    // A box that's here (set up by Kuvo Code itself): it attaches, as it always did.
    let box_here = StandIn::at(Replies::Never, NOW);
    here.answers(&box_here);
    click(cx, "launch-again");
    assert!(opened(&launch, cx));
    assert!(shown(cx, "rail-settings"));
    // (And offers it nothing: it carries no release.)
    assert!(box_here.seen_at("POST", "kuvo/box/update").is_empty());
}
