//! The box's installer as the app runs it, against a stand-in installer (a shell script the test
//! writes in a bundle of its own, saying what each test tells it to): nothing real is installed,
//! fetched, started or read. What's checked is what the app does with the box's word: which verb
//! it runs next, what it hands on to draw, and which builds never start the installer at all.

use std::path::Path;

use kuvo_box::install::stand_in::Bundle;
use kuvo_box::install::{End, Installer, Outcome, Progress, Verb};

const TRIAL: &str = "run.kuvo.code.trial713.p5";
const ROOT: &str = ".kuvo-trial713-p5";

fn bundle(folder: &Path) -> (Bundle, Installer) {
    let bundle = Bundle::make(folder, TRIAL);
    let installer = Installer::for_app(&bundle.executable(), Some(ROOT)).expect("a copy with a root of its own installs");
    (bundle, installer)
}

fn set_up(installer: &Installer) -> (Outcome, Vec<Progress>) {
    let mut drawn = Vec::new();
    let outcome = installer.set_up(&mut |progress| drawn.push(progress));
    (outcome, drawn)
}

/// How the stand-in was asked, without the bundle's own path.
fn asked(bundle: &Bundle) -> Vec<String> {
    let app = format!(" --app {}", bundle.path.display());
    bundle.asked().into_iter().map(|line| line.replace(&app, "")).collect()
}

#[test]
fn a_new_mac_is_set_up_and_its_steps_are_handed_on() {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = bundle(made.path());
    bundle.says("status", None, 0, &[r#"{"verdict":"fresh","verb":"new","status":{"state":"none"}}"#]);
    bundle.says("new", None, 0, &[r#"{"step":"checking"}"#, r#"{"step":"engine","title":"Downloading the engine"}"#, r#"{"result":"done","verdict":"ours","started":true,"status":{}}"#]);
    let (outcome, drawn) = set_up(&installer);
    assert_eq!(outcome, Outcome { end: End::Started, verdict: Some("ours".into()), ran: Some(Verb::New), message: None });
    assert_eq!(
        drawn,
        vec![
            Progress::Asking,
            Progress::Running { verb: Verb::New, verdict: Some("fresh".into()) },
            Progress::Step { step: "checking".into(), title: None },
            Progress::Step { step: "engine".into(), title: Some("Downloading the engine".into()) },
        ]
    );
    // Asked as the protocol says: the verb, the app's bundle, the copy's own root.
    assert_eq!(asked(&bundle), vec![format!("status --root {ROOT}"), format!("new --root {ROOT}")]);
    assert!(bundle.asked().iter().all(|line| line.contains(&format!("--app {}", bundle.path.display()))), "{:?}", bundle.asked());
}

#[test]
fn a_mac_with_its_kuvo_already_only_attaches() {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = bundle(made.path());
    for verdict in ["ours", "machine", "two", "back"] {
        bundle.says("status", None, 0, &[&format!(r#"{{"verdict":"{verdict}","verb":null,"status":{{}}}}"#)]);
        let (outcome, _) = set_up(&installer);
        assert_eq!(outcome, Outcome { end: End::Attach, verdict: Some(verdict.into()), ran: None, message: None }, "{verdict}");
    }
    // Nothing but `status` was ever run.
    assert!(asked(&bundle).iter().all(|line| line.starts_with("status ")), "{:?}", asked(&bundle));
}

#[test]
fn its_helper_not_in_place_is_started_by_new() {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = bundle(made.path());
    bundle.says("status", None, 0, &[r#"{"verdict":"ours","verb":"new","status":{}}"#]);
    bundle.says("new", None, 0, &[r#"{"result":"nothing","verdict":"ours","started":true,"status":{}}"#]);
    assert_eq!(set_up(&installer).0, Outcome { end: End::Started, verdict: Some("ours".into()), ran: Some(Verb::New), message: None });
}

#[test]
fn an_earlier_kuvo_code_s_data_is_handed_to_move_and_its_refusal_said() {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = bundle(made.path());
    let refusal = "Moving an earlier Kuvo Code's sessions into their new place isn't in this version yet.";
    for verdict in ["app-07", "app-box", "moving", "stuck"] {
        bundle.says("status", None, 0, &[&format!(r#"{{"verdict":"{verdict}","verb":"move","status":{{}}}}"#)]);
        bundle.says("move", None, 1, &[&format!(r#"{{"result":"refused","message":"{refusal}","verdict":"{verdict}","verb":"move","status":{{}}}}"#)]);
        let (outcome, drawn) = set_up(&installer);
        assert_eq!(outcome, Outcome { end: End::Stopped, verdict: Some(verdict.into()), ran: Some(Verb::Move), message: Some(refusal.into()) }, "{verdict}");
        assert_eq!(drawn.last(), Some(&Progress::Running { verb: Verb::Move, verdict: Some(verdict.into()) }));
    }
    // A move that's done (once the box has it) starts the box, as a new one does.
    bundle.says("move", None, 0, &[r#"{"result":"done","verdict":"ours","started":true,"status":{}}"#]);
    assert_eq!(set_up(&installer).0.end, End::Started);
}

#[test]
fn what_the_box_refuses_or_fails_is_said_in_its_words() {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = bundle(made.path());
    // The shell's own part failing at `status`, with no verdict: nothing else is run.
    bundle.says("status", None, 1, &[r#"{"result":"failed","message":"This copy of Kuvo Code is damaged. Download it again, then try again."}"#]);
    assert_eq!(set_up(&installer).0, Outcome { end: End::Stopped, verdict: None, ran: None, message: Some("This copy of Kuvo Code is damaged. Download it again, then try again.".into()) });
    assert_eq!(asked(&bundle).len(), 1);
    // `new` failing part way, after a step.
    bundle.says("status", None, 0, &[r#"{"verdict":"fresh","verb":"new","status":{}}"#]);
    bundle.says("new", None, 1, &[r#"{"step":"engine"}"#, r#"{"result":"failed","message":"The engine couldn't be downloaded. Check that this Mac is online, then try again."}"#]);
    let (outcome, _) = set_up(&installer);
    assert_eq!((outcome.end, outcome.verdict.as_deref(), outcome.message.as_deref()), (End::Stopped, Some("fresh"), Some("The engine couldn't be downloaded. Check that this Mac is online, then try again.")));
    // A box made whose helper couldn't be started: failed, in its words.
    bundle.says("new", None, 1, &[r#"{"result":"failed","verdict":"ours","started":false,"message":"Kuvo Helper couldn't be started: it said nothing.","status":{}}"#]);
    let (outcome, _) = set_up(&installer);
    assert_eq!((outcome.end, outcome.message.as_deref()), (End::Stopped, Some("Kuvo Helper couldn't be started: it said nothing.")));
    // Made in a home that isn't the real one: done, but it doesn't run, and says so.
    bundle.says("new", None, 0, &[r#"{"result":"done","verdict":"ours","started":false,"message":"It's made, and starts by itself only in ~/.kuvo-code.","status":{}}"#]);
    let (outcome, _) = set_up(&installer);
    assert_eq!((outcome.end, outcome.message.as_deref()), (End::NotStarted, Some("It's made, and starts by itself only in ~/.kuvo-code.")));
}

#[test]
fn another_move_holding_the_lock_waits() {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = bundle(made.path());
    bundle.says("status", None, 0, &[r#"{"verdict":"waits","verb":null,"status":{}}"#]);
    assert_eq!(set_up(&installer).0.end, End::Waits);
    // Taken between `status` and `new` (exit 4).
    bundle.says("status", None, 0, &[r#"{"verdict":"fresh","verb":"new","status":{}}"#]);
    bundle.says("new", None, 4, &[r#"{"result":"waits","verdict":"waits","message":"Another move is under way.","status":{}}"#]);
    let (outcome, _) = set_up(&installer);
    assert_eq!((outcome.end, outcome.message.as_deref()), (End::Waits, Some("Another move is under way.")));
}

#[test]
fn an_installer_that_says_nothing_readable_is_the_app_s_fault_and_said_so() {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = bundle(made.path());
    // Not asked as its usage says (exit 2, no JSON): its own last line is handed on.
    bundle.says("status", None, 2, &[]).complains("status", "Usage: sh kuvo-install status|new|move [--app <bundle>] [--root <name>]");
    let (outcome, _) = set_up(&installer);
    assert_eq!(outcome.end, End::Broken);
    assert!(outcome.message.as_deref().is_some_and(|message| message.contains("Usage: sh kuvo-install")), "{:?}", outcome.message);
    // Words that aren't JSON, and lines that aren't an answer.
    bundle.says("status", None, 0, &["sh: line 3: syntax error", "[1, 2]", r#"{"neither":"an answer"}"#]);
    assert_eq!(set_up(&installer).0.end, End::Broken);
    // A verb this app doesn't know is never run.
    bundle.says("status", None, 0, &[r#"{"verdict":"fresh","verb":"erase","status":{}}"#]);
    let before = asked(&bundle).len();
    assert_eq!(set_up(&installer).0.end, End::Broken);
    assert_eq!(asked(&bundle).len(), before + 1);
}

#[test]
fn nothing_the_app_was_started_with_reaches_the_installer() {
    let made = tempfile::tempdir().unwrap();
    let (bundle, installer) = bundle(made.path());
    bundle.says("status", None, 0, &[r#"{"verdict":"ours","verb":null,"status":{}}"#]);
    // (A child's own environment: nothing of this process is changed.)
    let run = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "started_with_kuvo_variables", "--ignored", "--nocapture"])
        .env("KUVO_HOME", "/somewhere/else")
        .env("KUVO_DEMO_MACHINE_ROOT", ROOT)
        .env("KUVO_TEST_P5_BUNDLE", bundle.executable())
        .output()
        .unwrap();
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stdout));
    assert_eq!(asked(&bundle).len(), 1);
    assert_eq!(bundle.kuvo_variables(), Vec::<String>::new());
    drop(installer);
}

/// Run only by the test above, in a process started with `KUVO_*` variables.
#[test]
#[ignore]
fn started_with_kuvo_variables() {
    let Some(executable) = std::env::var_os("KUVO_TEST_P5_BUNDLE") else { return };
    assert!(std::env::var_os("KUVO_HOME").is_some());
    let installer = Installer::for_app(Path::new(&executable), Some(ROOT)).unwrap();
    assert_eq!(set_up(&installer).0.end, End::Attach);
}

#[test]
fn a_build_that_isnt_the_app_in_its_bundle_never_starts_the_installer() {
    let made = tempfile::tempdir().unwrap();
    // A dev build (`cargo run`): this test's own program, in no bundle.
    let refused = Installer::for_app(&std::env::current_exe().unwrap(), None).unwrap_err();
    assert!(refused.0.contains("doesn’t set up a computer"), "{}", refused.0);
    // (Nor with a root named: it carries no release to install.)
    assert!(Installer::for_app(&std::env::current_exe().unwrap(), Some(ROOT)).is_err());
    // A copy of the app with no root of its own, or the real one's: never the real ~/.kuvo-code.
    let copy = Bundle::make(&made.path().join("copy"), TRIAL);
    for root in [None, Some(".kuvo-code"), Some(""), Some(".."), Some("a/b"), Some("../.kuvo-code")] {
        assert!(Installer::for_app(&copy.executable(), root).is_err(), "{root:?}");
    }
    // Kuvo Code itself, pointed at a root that isn't its own.
    let real = Bundle::make(&made.path().join("real"), "run.kuvo.code");
    assert!(Installer::for_app(&real.executable(), Some(ROOT)).is_err());
    // A bundle with no release in it, or no ID.
    let empty = Bundle::make(&made.path().join("empty"), TRIAL);
    std::fs::remove_file(empty.path.join("Contents/Resources/box-release/kuvo-install")).unwrap();
    assert!(Installer::for_app(&empty.executable(), Some(ROOT)).is_err());
    let nameless = Bundle::make(&made.path().join("nameless"), TRIAL);
    std::fs::write(nameless.path.join("Contents/Info.plist"), "bplist00").unwrap();
    assert!(Installer::for_app(&nameless.executable(), Some(ROOT)).is_err());
    // None of them was ever started.
    for bundle in [&copy, &real, &empty, &nameless] {
        assert_eq!(bundle.asked(), Vec::<String>::new());
    }
}

#[test]
fn kuvo_code_itself_installs_into_its_own_home_and_carries_the_keychain_tool() {
    let made = tempfile::tempdir().unwrap();
    let real = Bundle::make(made.path(), "run.kuvo.code");
    // (Only how it would be asked is checked: this test never runs it against the real home.)
    let installer = Installer::for_app(&real.executable(), None).unwrap();
    assert_eq!(installer.root(), ".kuvo-code");
    let carried = installer.carried();
    assert_eq!(carried.id, "run.kuvo.code");
    assert_eq!(carried.bundle, real.path);
    // The one-time Keychain hand-over tool sits where the box's move looks for it, in the bundle
    // the installer is told; the app itself never runs it.
    assert_eq!(carried.keys.as_deref(), Some(real.path.join("Contents/MacOS/kuvo-keys").as_path()));
    assert_eq!(real.asked(), Vec::<String>::new());
    std::fs::remove_file(real.path.join("Contents/MacOS/kuvo-keys")).unwrap();
    assert_eq!(kuvo_box::install::carried(&real.executable()).unwrap().keys, None);
}
