//! The app opening on a computer, as someone meets it: a new Mac set up, one whose Kuvo is here
//! already, an earlier Kuvo Code's sessions the box won't move yet, a box that doesn't start, and a
//! build that never installs. The installer is a stand-in (a shell script in a bundle of the
//! test's own, saying what each test tells it to) and so is the box: nothing real is installed,
//! started or reached. Clicks and keys go through the window; waiting is on the test's own clock.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::launch::{Attached, Launch, Opens, Said, Stage};
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
                None => Err(NOT_RUNNING.into()),
            }
        })
    }
}

fn open<'a>(cx: &'a mut TestAppContext, installer: Result<Installer, OnlyAttaches>, here: &Here) -> (Entity<Launch>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let opens = Opens { installer, attach: here.attach(), waits_within: WAITS };
    let window = cx.add_window(move |window, cx| Launch::new(opens, false, window, cx));
    let launch = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    (launch, cx)
}

fn carried(folder: &Path) -> (Bundle, Result<Installer, OnlyAttaches>) {
    let bundle = Bundle::make(folder, TRIAL);
    let installer = Installer::for_app(&bundle.executable(), Some(ROOT), folder);
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

/// What reaching this computer's Kuvo says when it isn't answering (`kuvo_box::place`'s words).
const NOT_RUNNING: &str = "Kuvo on this Mac isn't running.";
/// How long these tests' window waits on something else at work before it says so.
const WAITS: Duration = Duration::from_secs(20);
const FRESH: &str = r#"{"verdict":"fresh","verb":"new","status":{}}"#;
const DONE: &str = r#"{"result":"done","verdict":"ours","started":true,"status":{}}"#;
/// A move's steps as the box lists them ahead of where it is (`kuvo/box/rehome`'s `steps`).
const STEPS: &str = r#""steps":[{"step":"plan","title":"Checking this Mac"},{"step":"clone","title":"Copying your sessions and settings"},{"step":"database","title":"Bringing your sessions up to date"},{"step":"check","title":"Checking nothing is missing"},{"step":"clear","title":"Finishing up"}]"#;

/// What `status` says of an earlier Kuvo Code's sessions to move.
fn to_move() -> String {
    format!(r#"{{"verdict":"app-07","verb":"move","status":{{{STEPS}}}}}"#)
}

/// How each step in the move's list stands, in order, as it's drawn.
fn steps_drawn(cx: &mut VisualTestContext) -> Vec<&'static str> {
    (1..=5).map(|place| ["done", "now", "stopped", "ahead"].into_iter().find(|state| shown(cx, &format!("launch-step-{place}-{state}"))).unwrap_or("missing")).collect()
}

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
fn a_wiped_mac_waits_for_reinstall_without_attaching(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    bundle.says("status", None, 0, &[r#"{"verdict":"fresh","verb":null,"offer":"reinstall-local","status":{}}"#]);
    let here = Here::default();
    let (launch, cx) = open(cx, installer, &here);
    assert!(matches!(launch.read_with(cx, |launch, _| launch.stage().clone()), Stage::ReinstallOffered));
    assert_eq!(verbs(&bundle), ["status"]);
    assert_eq!(line(&launch, cx), "Reinstall Kuvo Code");
    assert!(!shown(cx, "launch-starting"));
    assert!(!opened(&launch, cx));
    assert_eq!(here.asked(), 0);
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
        launch.progress(Progress::Running { verb: Verb::New, verdict: Some("fresh".into()), moved: Default::default() }, cx);
        launch.progress(Progress::Step(Step { step: "checking".into(), ..Step::default() }), cx);
    });
    assert_eq!(line(&launch, cx), "Setting up…");
    launch.update(cx, |launch, cx| launch.progress(Progress::Step(Step { step: "engine".into(), title: Some("Downloading the engine".into()), ..Step::default() }), cx));
    assert_eq!(line(&launch, cx), "Setting up · Downloading the engine");
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
    bundle.says("status", None, 0, &[&to_move()]);
    bundle.says("move", None, 1, &[&format!(r#"{{"result":"refused","message":"{not_yet}","verdict":"app-07","verb":"move","status":{{{STEPS}}}}}"#)]);
    let here = Here::default();
    let (launch, cx) = open(cx, installer, &here);
    assert_eq!(verbs(&bundle), ["status", "move"]);
    assert_eq!(line(&launch, cx), "Couldn’t move your sessions");
    assert_eq!(message(&launch, cx).as_deref(), Some(not_yet));
    // Try Again, and no second line under it.
    assert!(shown(cx, "launch-again") && !shown(cx, "launch-note") && !opened(&launch, cx));
    // Refused before any step: none is marked as where it stopped.
    assert_eq!(steps_drawn(cx), ["ahead", "ahead", "ahead", "ahead", "ahead"]);
}

#[gpui::test]
fn an_earlier_kuvo_code_s_sessions_are_moved_and_the_app_opens(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    bundle.says("status", None, 0, &[&to_move()]);
    let dropped = "Websites you were signed in to in Kuvo Code's browser need signing in again. Your history and saved passwords come along.";
    let done = format!(r#"{{"result":"done","verdict":"ours","started":true,"status":{{"dropped":["{dropped}"],{STEPS}}}}}"#);
    bundle.says("move", None, 0, &[r#"{"step":"plan","title":"Checking this Mac","index":1,"of":5}"#, r#"{"step":"done","title":"Starting Kuvo on this Mac","index":5,"of":5}"#, &done]);
    let here = Here::default();
    // (A Mac's own Kuvo that's had its first run, as one moved from an earlier Kuvo Code says
    // it has; all that macOS could ask about allowed already, so nothing else comes up.)
    let moved = StandIn::at(Replies::Never, NOW);
    moved.set_permissions(|permissions| *permissions = kuvo::stand_in::permissions::Permissions { screen_recording: true, accessibility: true, full_disk_access: true, ..kuvo::stand_in::permissions::Permissions::nothing_allowed() });
    here.answers(&moved);
    let (launch, cx) = open(cx, installer, &here);
    assert_eq!(verbs(&bundle), ["status", "move"]);
    // Done: said in a card of its own, with what wasn't brought along in the box's words, and
    // what macOS will name from now on. No Welcome, and no steps' dots under it.
    assert!(!opened(&launch, cx));
    assert!(matches!(launch.read_with(cx, |launch, _| launch.stage().clone()), Stage::Said { what: Said::Moved, .. }));
    assert!(shown(cx, "first-run-card") && shown(cx, "launch-dropped") && shown(cx, "launch-helper") && !shown(cx, "first-run-dots"));
    assert_eq!(launch.read_with(cx, |launch, _| launch.moving().map(|moving| moving.dropped.clone())), Some(vec![dropped.into()]));
    // Return opens the app, in their sessions: the first run isn't had again.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(opened(&launch, cx));
    assert!(shown(cx, "rail-settings") && !shown(cx, "first-run"));
}

#[gpui::test]
fn a_move_is_a_card_of_its_own_listing_the_box_s_steps_and_where_it_is(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    // The app quit while the box moved; it's at its third step of five.
    let at = |step: &str, title: &str, index: u32| format!(r#"{{"verdict":"waits","verb":null,"status":{{"progress":{{"step":"{step}","title":"{title}","index":{index},"of":5}},{STEPS}}}}}"#);
    bundle.says("status", None, 0, &[&at("database", "Bringing your sessions up to date", 3)]);
    let here = Here::default();
    let (launch, cx) = open(cx, installer, &here);
    assert!(shown(cx, "first-run-card") && shown(cx, "launch-steps") && !shown(cx, "first-run-dots") && !shown(cx, "launch-again"));
    assert_eq!(steps_drawn(cx), ["done", "done", "now", "ahead", "ahead"]);
    assert_eq!(launch.read_with(cx, |launch, _| launch.moving().unwrap().steps[2].1.to_string()), "Bringing your sessions up to date");
    // Past its last step, Kuvo is being started: every step done.
    bundle.says("status", None, 0, &[&at("done", "Starting Kuvo on this Mac", 5)]);
    wait(cx, Duration::from_secs(2));
    assert_eq!(steps_drawn(cx), ["done", "done", "done", "done", "done"]);
}

#[gpui::test]
fn a_move_that_stops_says_where_and_why_in_the_box_s_words_and_tries_again_from_the_card(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    let why = "Kuvo couldn't bring your sessions up to date, so nothing was moved. Try again.";
    bundle.says("status", None, 0, &[&to_move()]);
    let failed = format!(r#"{{"result":"failed","message":"{why}","verdict":"app-07","status":{{"failed":{{"at":1,"step":"database","message":"{why}"}},{STEPS}}}}}"#);
    bundle.says("move", None, 1, &[r#"{"step":"plan","title":"Checking this Mac","index":1,"of":5}"#, r#"{"step":"clone","title":"Copying your sessions and settings","index":2,"of":5}"#, &failed]);
    let here = Here::default();
    let (launch, cx) = open(cx, installer, &here);
    assert_eq!(line(&launch, cx), "Couldn’t move your sessions");
    assert_eq!(message(&launch, cx).as_deref(), Some(why));
    // Where the box says it stopped, not the last step this window happened to hear of.
    assert_eq!(steps_drawn(cx), ["done", "done", "stopped", "ahead", "ahead"]);
    assert!(shown(cx, "launch-message") && !shown(cx, "launch-note") && shown(cx, "launch-again") && !opened(&launch, cx));
    // Try Again, from the same card: this time it moves.
    bundle.says("move", None, 0, &[r#"{"step":"plan","title":"Checking this Mac","index":1,"of":5}"#, DONE]);
    here.answers(&StandIn::at(Replies::Never, NOW));
    click(cx, "launch-again");
    assert_eq!(verbs(&bundle), ["status", "move", "status", "move"]);
    assert!(matches!(launch.read_with(cx, |launch, _| launch.stage().clone()), Stage::Said { what: Said::Moved, .. }));
    click(cx, "launch-next");
    assert!(opened(&launch, cx));
}

#[gpui::test]
fn a_move_that_stopped_part_way_says_it_couldnt_finish(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    let why = "Kuvo was moving your sessions to their new place on this Mac. Try again to finish.";
    bundle.says("status", None, 0, &[&format!(r#"{{"verdict":"stuck","verb":"move","status":{{{STEPS}}}}}"#)]);
    bundle.says("move", None, 1, &[&format!(r#"{{"result":"failed","message":"{why}","verdict":"stuck","status":{{"failed":{{"at":1,"step":"clear","message":"{why}"}},{STEPS}}}}}"#)]);
    let here = Here::default();
    let (launch, cx) = open(cx, installer, &here);
    assert_eq!(line(&launch, cx), "Couldn’t finish moving");
    assert_eq!(steps_drawn(cx), ["done", "done", "done", "done", "stopped"]);
    assert_eq!(message(&launch, cx).as_deref(), Some(why));
}

#[gpui::test]
fn a_mac_with_two_setups_is_told_in_the_box_s_words_before_the_app_opens(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    let two = "This Mac is set up as one of your machines, and has an earlier Kuvo Code's sessions too: putting them together isn't in this version yet.";
    bundle.says("status", None, 0, &[&format!(r#"{{"verdict":"two","verb":null,"message":"{two}","status":{{}}}}"#)]);
    let here = Here::default();
    here.answers(&StandIn::at(Replies::Never, NOW));
    let (launch, cx) = open(cx, installer, &here);
    assert_eq!(verbs(&bundle), ["status"]);
    assert!(matches!(launch.read_with(cx, |launch, _| launch.stage().clone()), Stage::Said { what: Said::Left(words), .. } if words.as_ref() == two));
    assert!(shown(cx, "launch-message") && !shown(cx, "launch-again") && !opened(&launch, cx));
    click(cx, "launch-next");
    assert!(opened(&launch, cx));
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
    // (A box that doesn't list its steps: the one it's at is drawn.)
    assert!(shown(cx, "first-run-card") && shown(cx, "launch-step-3-now") && !shown(cx, "launch-again"));
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
    // Said as done, since the move was watched from here; then the app.
    assert!(matches!(launch.read_with(cx, |launch, _| launch.stage().clone()), Stage::Said { what: Said::Moved, .. }));
    click(cx, "launch-next");
    assert!(opened(&launch, cx));
    assert_eq!(verbs(&bundle), ["status", "status", "status"]);
}

#[gpui::test]
fn a_move_that_goes_on_and_on_is_said_with_try_again_rather_than_waited_for_forever(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    bundle.says("status", None, 0, &[r#"{"verdict":"waits","verb":null,"status":{"progress":{"step":"clone","title":"Copying your sessions and settings","index":3,"of":12}}}"#]);
    let here = Here::default();
    let (launch, cx) = open(cx, installer, &here);
    wait(cx, Duration::from_secs(16));
    assert!(!shown(cx, "launch-again") && shown(cx, "launch-step-3-now"));
    wait(cx, Duration::from_secs(6));
    assert_eq!(line(&launch, cx), "Still moving your sessions");
    assert!(shown(cx, "launch-again") && shown(cx, "launch-message"));
    // Nothing but `status` was ever run; Try Again asks from the start.
    assert!(verbs(&bundle).iter().all(|verb| verb == "status"));
    let asked = verbs(&bundle).len();
    click(cx, "launch-again");
    assert_eq!(verbs(&bundle).len(), asked + 1);
    assert!(shown(cx, "launch-step-3-now") && !shown(cx, "launch-again"));
}

#[gpui::test]
fn a_second_open_while_the_first_sets_this_computer_up_waits_for_it_and_says_nothing_of_a_move(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    // The first open holds the lock while it sets up; no move, so no step to draw.
    bundle.says("status", None, 0, &[r#"{"verdict":"waits","verb":null,"status":{}}"#]);
    let here = Here::default();
    let (launch, cx) = open(cx, installer, &here);
    assert!(!shown(cx, "first-run-card") && !shown(cx, "launch-again") && !shown(cx, "launch-message"));
    assert_eq!(launch.read_with(cx, |launch, _| launch.waiting_words()), "Setting up Kuvo on this Mac…");
    // Done: it opens.
    bundle.says("status", None, 0, &[r#"{"verdict":"ours","verb":null,"status":{}}"#]);
    here.answers(&StandIn::first_run(Replies::Never, NOW, Scenario::Nothing));
    wait(cx, Duration::from_secs(2));
    assert!(opened(&launch, cx));
    assert!(verbs(&bundle).iter().all(|verb| verb == "status"));
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
    // It's waited for a little (at login the two start together), quietly.
    assert!(!shown(cx, "launch-again") && !shown(cx, "first-run-card"));
    wait(cx, Duration::from_secs(21));
    // No welcome for someone who's had it: the window's quiet middle, why, and Try Again.
    assert!(!shown(cx, "first-run-card") && shown(cx, "launch-message") && shown(cx, "launch-again"));
    assert_eq!(message(&launch, cx).as_deref(), Some(NOT_RUNNING));
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
fn a_computer_whose_engine_is_fetched_before_it_s_judged_says_it_s_being_set_up(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    // As the real installer goes on a computer with nothing: `status` itself unpacks the release
    // and fetches the engine, then fails offline.
    let offline = "The engine couldn't be downloaded. Check that this Mac is online, then try again.";
    bundle.says("status", None, 1, &[r#"{"step":"checking"}"#, r#"{"step":"engine"}"#, &format!(r#"{{"result":"failed","message":"{offline}"}}"#)]);
    let here = Here::default();
    let (launch, cx) = open(cx, installer, &here);
    assert_eq!(message(&launch, cx).as_deref(), Some(offline));
    assert!(shown(cx, "launch-again"));
    assert_eq!(verbs(&bundle), ["status"]);
    // While it worked, the window's quiet line said so: the steps came as it was still asked.
    launch.update(cx, |launch, cx| {
        launch.show(Stage::Asking, false, cx);
        assert_eq!(launch.waiting_words(), "Setting up Kuvo on this Mac…");
    });
    // A Mac whose Kuvo is here never says so: nothing is set up for whoever sees it.
    let other = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(other.path());
    bundle.says("status", None, 0, &[r#"{"verdict":"ours","verb":null,"status":{}}"#]);
    let (launch, cx) = open(cx, installer, &here);
    assert_eq!(launch.read_with(cx, |launch, _| launch.waiting_words()), "Starting…");
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
    let dev = Installer::for_app(&std::env::current_exe().unwrap(), None, std::env::temp_dir().as_path());
    assert!(dev.is_err());
    // Nothing here: it says so, and that this build installs nothing. Nothing was installed.
    let here = Here::default();
    let (launch, cx) = open(cx, dev.clone(), &here);
    assert_eq!(message(&launch, cx).as_deref(), Some(NOT_RUNNING));
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

#[gpui::test]
fn an_installer_that_breaks_under_a_move_is_said_in_the_move_s_card(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    bundle.says("status", None, 0, &[&to_move()]);
    bundle.says("move", None, 3, &["not an answer"]);
    let here = Here::default();
    let (launch, cx) = open(cx, installer, &here);
    // The move's own card still: its steps and Try Again.
    assert_eq!(line(&launch, cx), "Couldn’t move your sessions");
    assert!(shown(cx, "launch-steps") && !shown(cx, "launch-note") && shown(cx, "launch-again") && !opened(&launch, cx));
}

#[gpui::test]
fn after_a_move_what_macos_must_allow_kuvo_helper_is_asked_before_the_app_opens(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    bundle.says("status", None, 0, &[&to_move()]);
    let done = format!(r#"{{"result":"done","verdict":"ours","started":true,"status":{{{STEPS}}}}}"#);
    bundle.says("move", None, 0, &[r#"{"step":"plan","title":"Checking this Mac","index":1,"of":5}"#, &done]);
    let here = Here::default();
    // This Mac's own Kuvo, moved, whose helper macOS allows neither yet.
    let moved = StandIn::at(Replies::Never, NOW);
    moved.set_permissions(|permissions| *permissions = kuvo::stand_in::permissions::Permissions::nothing_allowed());
    here.answers(&moved);
    let (launch, cx) = open(cx, installer, &here);
    assert!(matches!(launch.read_with(cx, |launch, _| launch.stage().clone()), Stage::Said { what: Said::Moved, .. }));
    // (The step says it next: not this card.)
    assert!(!shown(cx, "launch-helper"));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(matches!(launch.read_with(cx, |launch, _| launch.stage().clone()), Stage::Allow { .. }));
    assert!(shown(cx, "permissions-step") && shown(cx, "permission-screenRecording-open") && !opened(&launch, cx));
    // Return waits while something's asked about; Not Now puts it off and opens the app.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(!opened(&launch, cx));
    click(cx, "launch-not-now");
    assert_eq!(moved.seen_at("POST", "kuvo/box/permissions").len(), 3);
    assert!(opened(&launch, cx));
    assert!(shown(cx, "rail-settings") && !shown(cx, "permissions-step"));
}

const ATTACHES: &str = r#"{"verdict":"ours","verb":null,"status":{}}"#;
const WAITING: &str = r#"{"step":"waiting","title":"Kuvo on this Mac updates once your sessions finish.","confirm":{"title":"Update Kuvo on this Mac now?","message":"2 sessions are at work on this Mac. They stop as Kuvo restarts.","label":"Update Now"}}"#;
const BUSY: &str = r#"{"result":"busy","message":"Something is running there now."}"#;

/// The app as it opened on a Mac whose Kuvo is from before it took the release the app carries
/// itself (`made` as the test says, before it opens): its installer brings it up (`update`),
/// first asked to wait for the work there to end, and saying `first`.
fn on_a_mac_from_before<'a>(cx: &'a mut TestAppContext, made: &Path, first: &[&str], set: impl FnOnce(&StandIn)) -> (Bundle, StandIn, Entity<Launch>, &'a mut VisualTestContext) {
    let (bundle, installer) = carried(made);
    let exit = if first.last().is_some_and(|last| last.contains("\"updated\"") || last.contains("\"current\"")) { 0 } else { 1 };
    bundle.says("status", None, 0, &[ATTACHES]).says("update", Some(1), exit, first);
    let box_here = StandIn::at(Replies::Never, NOW);
    box_here.from_before_update_here();
    set(&box_here);
    let here = Here::default();
    here.answers(&box_here);
    let (launch, cx) = open(cx, installer, &here);
    assert!(opened(&launch, cx));
    (bundle, box_here, launch, cx)
}

fn workspace_of(launch: &Entity<Launch>, cx: &mut VisualTestContext) -> Entity<kuvo::workspace::Workspace> {
    let home = launch.read_with(cx, |launch, _| launch.home().cloned()).expect("the app is open");
    home.read_with(cx, |home, _| home.workspace().clone())
}

fn chip(launch: &Entity<Launch>, cx: &mut VisualTestContext) -> Option<kuvo::workspace::HeldUpdate> {
    workspace_of(launch, cx).read_with(cx, |workspace, _| workspace.update_chip())
}

fn label(launch: &Entity<Launch>, cx: &mut VisualTestContext) -> Option<&'static str> {
    chip(launch, cx).as_ref().map(kuvo::title_bar::chip_label)
}

#[gpui::test]
fn a_mac_whose_kuvo_is_from_before_is_updated_once_its_work_ends_or_now_on_your_word(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, _, launch, cx) = on_a_mac_from_before(cx, made.path(), &[WAITING, BUSY], |_| {});
    // The installer was asked to bring it up, waiting for the work there to end: the window is
    // open as ever, with only the chip saying so.
    assert_eq!(verbs(&bundle), ["status", "update"]);
    assert!(!bundle.asked()[1].contains("--now"));
    assert!(shown(cx, "update-chip") && !shown(cx, "update-card") && shown(cx, "rail-settings"));
    assert_eq!(label(&launch, cx), Some("Update Ready"));
    click(cx, "update-chip");
    assert!(shown(cx, "update-card-title") && shown(cx, "update-now"));
    assert_eq!(chip(&launch, cx).map(|chip| chip.title).as_deref(), Some("Kuvo on this Mac updates once your sessions finish."));
    // Update Now asks first, in the box's words, and only then is it updated now.
    bundle.says("update", Some(2), 0, &[r#"{"step":"updating","title":"Updating Kuvo on this Mac…"}"#, r#"{"result":"updated"}"#]);
    click(cx, "update-now");
    assert!(shown(cx, "update-confirm"));
    assert_eq!(verbs(&bundle), ["status", "update"]);
    click(cx, "update-confirm");
    assert_eq!(verbs(&bundle), ["status", "update", "update"]);
    assert!(bundle.asked()[2].starts_with("update --now"), "{:?}", bundle.asked());
    // Updated: nothing more to say.
    assert!(!shown(cx, "update-chip"));
}

#[gpui::test]
fn update_now_with_no_word_from_the_box_still_asks_first(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    // (Busy, with no waiting step: the box said no question of its own.)
    let (bundle, _, launch, cx) = on_a_mac_from_before(cx, made.path(), &[BUSY], |_| {});
    click(cx, "update-chip");
    click(cx, "update-now");
    assert!(shown(cx, "update-confirm"), "what's at work here stops: it asks first");
    assert_eq!(verbs(&bundle), ["status", "update"]);
    assert_eq!(label(&launch, cx), Some("Update Ready"));
}

#[gpui::test]
fn update_now_that_fails_says_why_and_try_again_waits_for_the_work_to_end(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, _, launch, cx) = on_a_mac_from_before(cx, made.path(), &[WAITING, BUSY], |_| {});
    let offline = "The engine couldn't be downloaded. Check that this Mac is online, then try again.";
    bundle.says("update", Some(2), 1, &[&format!(r#"{{"result":"failed","message":"{offline}"}}"#)]);
    click(cx, "update-chip");
    click(cx, "update-now");
    click(cx, "update-confirm");
    click(cx, "update-chip");
    let said = chip(&launch, cx).expect("the chip");
    assert_eq!((said.title.as_str(), said.detail.as_str(), said.attention), ("Kuvo on this Mac couldn’t be updated", offline, true));
    assert!(shown(cx, "update-card-words") && shown(cx, "update-now"));
    // Try Again never updates over work at once: it waits for it to end (at once where none is).
    bundle.says("update", Some(3), 0, &[r#"{"result":"updated"}"#]);
    click(cx, "update-now");
    assert!(!shown(cx, "update-confirm"));
    assert_eq!(verbs(&bundle), ["status", "update", "update", "update"]);
    assert!(!bundle.asked()[3].contains("--now"), "{:?}", bundle.asked());
    assert!(!shown(cx, "update-chip"));
}

#[gpui::test]
fn update_now_that_finds_it_updating_already_says_so(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, _, launch, cx) = on_a_mac_from_before(cx, made.path(), &[WAITING, BUSY], |_| {});
    bundle.says("update", Some(2), 1, &[r#"{"result":"busy","message":"It's updating already."}"#]);
    click(cx, "update-chip");
    click(cx, "update-now");
    click(cx, "update-confirm");
    let said = chip(&launch, cx).expect("the chip");
    assert!(said.updating && said.action.is_none() && !said.attention, "{said:?}");
    assert_eq!(said.title, "Updating Kuvo on this Mac…");
    assert_eq!(label(&launch, cx), Some("Updating…"));
}

#[gpui::test]
fn a_mac_whose_kuvo_cant_be_updated_now_says_why_with_nothing_to_press(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let blocked = "A Mac that connects to this machine has an older Kuvo Code, which would put back older versions: update Kuvo Code there first.";
    let (_, _, launch, cx) = on_a_mac_from_before(cx, made.path(), &[&format!(r#"{{"result":"blocked","message":"{blocked}"}}"#)], |_| {});
    click(cx, "update-chip");
    let said = chip(&launch, cx).expect("the chip");
    assert_eq!((said.detail.as_str(), said.updating), (blocked, false));
    assert!(shown(cx, "update-card-words") && !shown(cx, "update-now"));
}

#[gpui::test]
fn an_installer_that_cant_say_is_said_never_a_spinner(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    // (It answers `status`, and doesn't know `update`.)
    bundle.says("status", None, 0, &[ATTACHES]).complains("update", "Usage: sh kuvo-install status|new|move");
    let box_here = StandIn::at(Replies::Never, NOW);
    box_here.from_before_update_here();
    let here = Here::default();
    here.answers(&box_here);
    let (launch, cx) = open(cx, installer, &here);
    assert!(opened(&launch, cx));
    let said = chip(&launch, cx).expect("the chip");
    assert!(!said.updating && said.action.is_none() && said.detail.contains("Usage: sh kuvo-install"), "{said:?}");
}

#[gpui::test]
fn a_mac_whose_kuvo_takes_the_release_itself_is_only_offered_it(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = carried(made.path());
    bundle.says("status", None, 0, &[ATTACHES]);
    let box_here = StandIn::at(Replies::Never, NOW);
    let here = Here::default();
    here.answers(&box_here);
    let (launch, cx) = open(cx, installer, &here);
    assert!(opened(&launch, cx));
    // Offered, and the box takes it itself: the installer isn't asked to.
    assert_eq!(box_here.seen_at("POST", "kuvo/box/update").len(), 1);
    assert_eq!(verbs(&bundle), ["status"]);
    assert!(!shown(cx, "update-chip"));
}

#[gpui::test]
fn an_older_machine_waits_for_this_macs_kuvo_to_update_first_and_says_so(cx: &mut TestAppContext) {
    let made = tempfile::tempdir().unwrap();
    // (Its list offers to set devbox up again, too: this Mac's own update comes first.)
    let (bundle, _, launch, cx) = on_a_mac_from_before(cx, made.path(), &[WAITING, BUSY], |box_here| {
        box_here.peer_box("key_devbox").expect("devbox").set_older(true);
        box_here.sets_up_again("key_devbox");
    });
    click(cx, "computer-switcher");
    click(cx, "menu-item-devbox");
    click(cx, "update-chip");
    let said = chip(&launch, cx).expect("the chip");
    assert_eq!(said.title, "Kuvo on this Mac updates once your sessions finish.");
    assert_eq!(said.detail, "Then devbox can be brought up to date from here.");
    // Its Update Now brings this Mac's Kuvo up now, asked first.
    bundle.says("update", Some(2), 0, &[r#"{"result":"updated"}"#]);
    click(cx, "update-now");
    click(cx, "update-confirm");
    assert!(bundle.asked()[2].starts_with("update --now"), "{:?}", bundle.asked());
}
