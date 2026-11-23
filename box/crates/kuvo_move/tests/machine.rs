//! Exercise the real store handoff and machine move, including a killed mover.

use kuvo_move::{Error, Step};
use kuvo_os::platform::Platform;
use kuvo_store::{home::Home, layout};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

const MAC: &str = "legacy-mac";
const FIXTURE: &[(&str, &str)] = &[
    (
        "config/opencode/opencode.json",
        r#"{"provider":{"keep":true,"over":0}}"#,
    ),
    (
        "keys/settings.json",
        r#"{"provider":{"over":1},"instructions":["RULES.md"]}"#,
    ),
    ("keys/KUVO_KEY_EXAMPLE", "fixture-key"),
    ("keys/KUVO_KEY_EXAMPLE__bot", "bot-key"),
    ("box/git.json", r#"{"branchPrefix":"work/","moved":true}"#),
    (
        "run/launch.env",
        "KUVO_SETTINGS=before\nKUVO_PRIVATE_SETTINGS=secret\nKUVO_TZ=UTC\n",
    ),
    (
        "run/link.json",
        r#"{"key":"fake-key","name":"Test Machine","secret":"fake-secret","peersFrom":{"legacy-mac":{"at":9,"peers":[{"name":"An older Mac"}],"moved":[]},"signed-mac":{"at":4}},"macSigns":{"signed-mac":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"},"vouched":{},"macAts":{},"retained":true}"#,
    ),
    ("run/outbox/ltr_123.json", r#"{"text":"keep this letter"}"#),
    ("run/outbox/ltr_old.json.mac-taken", "already claimed"),
    ("run/mac-agents.json", "{}"),
    ("run/connected/client", "present"),
    ("run/present/client", "present"),
    (
        "run/pages.json",
        r#"["https://example.invalid",{"url":"file:///tmp/scratch","session":"ses_example"},"ftp://example.invalid"]"#,
    ),
    ("agents/claude/config/sign-in.from-mac", "marker"),
    ("agents/claude/config/sign-in", "carried"),
    ("agents/claude/config/another", "local"),
    ("agents/runtime/codex/bin/codex", "retire"),
    ("agents/runtime/grok/bin/grok", "retire"),
    (
        "agents/runtime/claude/node_modules/@anthropic-ai/claude-agent-sdk-linux-test/program",
        "retire",
    ),
    (
        "agents/runtime/claude/node_modules/@anthropic-ai/claude-agent-sdk/package.json",
        "keep",
    ),
    ("agents/cache/install/tmp", "retire"),
    ("agents/sessions/ses_test/session.json", "{}"),
    (
        "bots/personalities.json",
        r#"{"kuvo":1,"personalities":[],"removed":[{"id":"custom.retired","removed":3}]}"#,
    ),
];

fn seed(root: &Path) -> Home {
    for (file, bytes) in FIXTURE {
        let file = root.join(file);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, bytes).unwrap();
    }
    fs::write(
        root.join("bots/kit.json"),
        json!({
            "kuvo": 1, "timeZone": "Europe/Istanbul",
            "projects": [{ "directory": root.join("workspace"), "name": "My Workspace" }],
            "personalities": [
                { "id": "custom.kept", "name": "A  Kit  Personality", "description": " Instructions ", "custom": true },
                { "id": "custom.retired", "name": "Retired", "description": "Removed earlier", "custom": true }
            ],
        })
        .to_string(),
    )
    .unwrap();
    Home::new(root.to_owned()).unwrap()
}

fn read(root: &Path, name: &str) -> Value {
    serde_json::from_slice(&fs::read(root.join(name)).unwrap()).unwrap()
}

fn open(home: &Home, step: impl FnMut(Step)) -> Result<(), Error> {
    kuvo_move::open(home, Platform::current(), None, step)
}

#[test]
fn an_older_machine_adopts_its_settings_and_keeps_its_work_once() {
    let root = tempfile::tempdir().unwrap();
    let home = seed(root.path());
    assert!(matches!(
        layout::open(&home),
        Err(layout::Error::MoveRequired { version: 0 })
    ));
    open(&home, |_| {}).unwrap();
    let root = root.path();
    assert_eq!(read(root, layout::FILE), json!({ "version": 23 }));
    let link = read(root, "run/link.json");
    assert_eq!(link["key"], "fake-key");
    assert_eq!(link["secret"], "fake-secret");
    assert_eq!(link["retained"], true);
    assert_eq!(link["formatVersion"], 1);
    assert_eq!(
        link["legacy"],
        json!({ (MAC): { "at": 9, "peers": [{ "name": "An older Mac" }], "moved": [] } })
    );
    for removed in ["peersFrom", "vouched", "macAts"] {
        assert!(!link.as_object().unwrap().contains_key(removed));
    }
    assert_eq!(
        read(root, "box/opencode.json"),
        json!({
            "provider": { "keep": true, "over": 1 },
            "instructions": ["RULES.md"],
            "branch_prefix": "work/",
            "permissions": [{ "action": "edit", "resource": "*/bots/bot_*/MEMORY.md", "effect": "allow" }],
        })
    );
    assert_eq!(
        read(root, "box/git.json"),
        json!({ "moved": true, "formatVersion": 1 })
    );
    assert_eq!(
        fs::read(root.join("box/keys/KUVO_KEY_EXAMPLE")).unwrap(),
        b"fixture-key"
    );
    assert!(!root.join("box/keys/KUVO_KEY_EXAMPLE__bot").exists());
    assert_eq!(
        fs::read(root.join("box/adopted-from/opencode.json")).unwrap(),
        fs::read(root.join("config/opencode/opencode.json")).unwrap()
    );
    assert_eq!(
        fs::read(root.join("run/launch.env")).unwrap(),
        b"KUVO_TZ=UTC\n"
    );
    assert_eq!(
        read(root, "box/time-zone.json"),
        json!({ "kuvo": 1, "zone": "Europe/Istanbul", "n": 0, "carried": true, "kitSeen": "Europe/Istanbul" })
    );
    assert_eq!(
        read(root, "box/projects.json"),
        json!({ "version": 1, "projects": [{ "directory": root.join("workspace"), "name": "My Workspace", "folders": [] }] })
    );
    assert_eq!(
        read(root, "bots/personalities.json"),
        json!({
            "kuvo": 1,
            "personalities": [{ "id": "custom.kept", "name": "A Kit Personality", "description": "Instructions", "symbol": "person.crop.circle", "updated": 0 }],
            "removed": [{ "id": "custom.retired", "removed": 3 }],
        })
    );
    assert_eq!(
        read(root, "run/outbox/sending/ltr_123.json"),
        json!({"text": "keep this letter"})
    );
    assert_eq!(
        read(root, "run/pages.json"),
        json!([
            { "url": "https://example.invalid", "session": null },
            { "url": "file:///tmp/scratch", "session": "ses_example" },
        ])
    );
    for missing in [
        "run/outbox/ltr_123.json",
        "run/mac-agents.json",
        "run/connected",
        "run/present",
        "agents/claude/config/sign-in",
        "agents/claude/config/sign-in.from-mac",
        "agents/runtime/codex",
        "agents/runtime/grok",
        "agents/cache/install",
        "agents/runtime/claude/node_modules/@anthropic-ai/claude-agent-sdk-linux-test",
    ] {
        assert!(!root.join(missing).exists(), "{missing}");
    }
    for kept in [
        "run/outbox/ltr_old.json.mac-taken",
        "agents/claude/config/another",
        "agents/sessions/ses_test/session.json",
        "agents/runtime/claude/node_modules/@anthropic-ai/claude-agent-sdk/package.json",
    ] {
        let (_, bytes) = FIXTURE.iter().find(|(path, _)| *path == kept).unwrap();
        assert_eq!(
            fs::read(root.join(kept)).unwrap(),
            bytes.as_bytes(),
            "{kept}"
        );
    }

    let settings = root.join("box/opencode.json");
    fs::write(&settings, "{}").unwrap();
    open(&home, |step| panic!("unexpected move step {step:?}")).unwrap();
    assert_eq!(fs::read(&settings).unwrap(), b"{}");
}

#[test]
fn a_refused_home_keeps_its_layout_and_data() {
    let root = tempfile::tempdir().unwrap();
    let home = seed(root.path());
    let marker = root.path().join(layout::FILE);
    for layout in [r#"{"version":24}"#, r#"{"version":11,"upgradingTo":12}"#] {
        fs::write(&marker, layout).unwrap();
        let before = snapshot(root.path());
        assert!(open(&home, |step| panic!("unexpected write at {step:?}")).is_err());
        assert_eq!(snapshot(root.path()), before);
    }
    fs::write(&marker, r#"{"version":0,"upgradingTo":1}"#).unwrap();
    fs::create_dir_all(root.path().join("updates/rehome")).unwrap();
    fs::write(
        root.path().join("updates/rehome/journal.json"),
        r#"{"step":"place"}"#,
    )
    .unwrap();
    let before = snapshot(root.path());
    assert!(matches!(
        open(&home, |step| panic!("unexpected write at {step:?}")),
        Err(Error::PreviousMove(_))
    ));
    assert_eq!(snapshot(root.path()), before);
}

#[test]
fn an_empty_home_opens_and_a_mac_app_folder_requires_staging() {
    let root = tempfile::tempdir().unwrap();
    let fresh = Home::new(root.path().join("test-fresh")).unwrap();
    open(&fresh, |step| panic!("unexpected move step {step:?}")).unwrap();
    assert_eq!(read(fresh.root(), layout::FILE), json!({"version": 23}));

    let app = root
        .path()
        .join("Library/Application Support/run.kuvo.code/chats");
    fs::create_dir_all(&app).unwrap();
    fs::write(app.join("fixture.json"), "{}").unwrap();
    let destination = Home::new(root.path().join(".kuvo-code")).unwrap();
    assert!(matches!(
        kuvo_move::open(&destination, Platform::Mac, Some(root.path()), |_| {}),
        Err(Error::AppFolder(_))
    ));
    assert!(!destination.root().exists());
}

#[test]
fn an_interrupted_additive_layout_keeps_a_machine_that_never_had_a_bridge() {
    let root = tempfile::tempdir().unwrap();
    let home = Home::new(root.path().to_owned()).unwrap();
    let config = root.path().join("config/opencode/opencode.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, br#"{"instructions":["RULES.md"]}"#).unwrap();
    fs::write(
        root.path().join(layout::FILE),
        br#"{"version":0,"upgradingTo":1}"#,
    )
    .unwrap();
    assert!(matches!(
        layout::open(&home),
        Err(layout::Error::ResumeMove {
            version: 0,
            upgrading_to: 1
        })
    ));
    open(&home, |_| {}).unwrap();
    assert_eq!(read(root.path(), layout::FILE), json!({"version":23}));
    assert_eq!(
        read(root.path(), "box/opencode.json")["instructions"],
        json!(["RULES.md"])
    );
    assert!(!root.path().join("run/link.json").exists());
}

#[test]
fn a_live_predecessor_bridge_keeps_the_move_from_retiring_its_files() {
    let root = tempfile::tempdir().unwrap();
    let home = seed(root.path());
    let record = root.path().join("run/bridge.json");
    fs::write(&record, json!({ "pid": std::process::id() }).to_string()).unwrap();
    let before = snapshot(root.path());
    assert!(
        matches!(open(&home, |_| panic!("started a move")), Err(Error::Busy(path))
        if path.ends_with("run/bridge.json"))
    );
    assert_eq!(snapshot(root.path()), before);
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_owned();
            if entry.file_type().unwrap().is_dir() {
                result.insert(relative, None);
                visit(root, &path, result);
            } else {
                result.insert(relative, Some(fs::read(&path).unwrap()));
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

#[test]
fn a_machine_move_can_be_killed_at_every_durable_boundary() {
    let root = tempfile::tempdir().unwrap();
    let mut boundaries = Vec::new();
    let home = seed(root.path());
    open(&home, |step| boundaries.push(step)).unwrap();
    assert_eq!(
        boundaries,
        [
            Step::Settings,
            Step::Kit,
            Step::Link,
            Step::Outbox,
            Step::Pages,
            Step::Agents,
            Step::Presence,
            Step::Layout
        ]
    );
    for step in boundaries {
        let path = root.path().join("machine");
        let home = seed(&path);
        let expected = open_then_snapshot(&home);
        fs::remove_dir_all(&path).unwrap();
        let home = seed(&path);
        let mut child = ChildGuard(
            Command::new(env::current_exe().unwrap())
                .args(["--exact", "move_child", "--nocapture"])
                .env_clear()
                .envs(
                    env::vars_os().filter(|(name, _)| !name.to_string_lossy().starts_with("KUVO_")),
                )
                .env("KUVO_MOVE_TEST_ROOT", home.root())
                .env("KUVO_MOVE_TEST_STEP", format!("{step:?}"))
                .stdout(Stdio::piped())
                .stdin(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let stdout = child.0.stdout.take().unwrap();
        let (send, receive) = mpsc::channel();
        let watch = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if line.as_deref().is_ok_and(|line| line == "checkpoint") {
                    send.send(()).ok();
                    break;
                }
            }
        });
        receive
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|err| panic!("{step:?}: {err}"));
        child.0.kill().unwrap();
        assert!(!child.0.wait().unwrap().success());
        watch.join().unwrap();
        open(&home, |_| {}).unwrap();
        assert_eq!(snapshot(&path), expected, "after {step:?}");
        fs::remove_dir_all(&path).unwrap();
    }
}

fn open_then_snapshot(home: &Home) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    open(home, |_| {}).unwrap();
    snapshot(home.root())
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[test]
fn move_child() {
    let Some(root) = env::var_os("KUVO_MOVE_TEST_ROOT") else {
        return;
    };
    let pause = env::var("KUVO_MOVE_TEST_STEP").unwrap();
    let home = Home::new(root.into()).unwrap();
    open(&home, |step| {
        if format!("{step:?}") == pause {
            // A one-thread libtest prints the test name without a newline.
            println!("\ncheckpoint");
            io::stdout().flush().unwrap();
            io::stdin().read_exact(&mut [0]).unwrap();
        }
    })
    .unwrap();
}
