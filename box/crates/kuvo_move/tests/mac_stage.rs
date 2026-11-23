#![cfg(target_os = "macos")]

use kuvo_move::mac::{AppStep, stage};
use kuvo_store::{home::Home, layout};
use rusqlite::Connection;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

#[derive(Deserialize)]
struct App {
    home: PathBuf,
    #[serde(rename = "K")]
    destination: PathBuf,
    #[serde(rename = "S")]
    source: PathBuf,
    bundle: PathBuf,
    release: PathBuf,
    engine: PathBuf,
}

impl App {
    fn stage(&self, completed: impl FnMut(AppStep)) -> kuvo_move::mac::StagedApp {
        stage(
            &Home::new(self.destination.clone()).unwrap(),
            &self.home,
            &self.bundle,
            &self.release,
            &self.engine,
            completed,
        )
        .unwrap()
    }
}

fn put(path: &Path, content: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn paths(home: &Path) -> App {
    App {
        home: home.into(),
        destination: home.join(".kuvo-code"),
        source: home.join("Library/Application Support/run.kuvo.code.stage-test"),
        bundle: home.join("Applications/Kuvo Code.app"),
        release: home.join(".kuvo-code/updates/install/release"),
        engine: home.join(".kuvo-code/updates/install/engine"),
    }
}

fn example(home: &Path) -> App {
    let app = paths(home);
    put(&app.bundle.join("Contents/Info.plist"), br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>run.kuvo.code.stage-test</string></dict></plist>"#);
    put(&app.release.join("box.json"), br#"{"engine":"2.2.6"}"#);
    put(&app.release.join("bin/kuvo-start"), b"launcher");
    put(
        &app.release
            .join("bin/Kuvo Helper.app/Contents/MacOS/Kuvo Helper"),
        b"helper",
    );
    put(
        &app.engine.join("2.2.6/opencode"),
        b"unused when there is no database",
    );
    put(&app.source.join("Chats/c1/notes.md"), b"chat");
    put(
        &app.source.join("Attachments/c1/file with spaces.png"),
        b"attachment",
    );
    put(
        &app.source.join("box/run/engine.json"),
        b"{\"pid\":2147483647}",
    );
    put(&app.source.join("box/run/socket.key"), b"old rendezvous");
    put(
        &app.source.join("box/run/link.json"),
        b"{\"name\":\"A fixture\"}",
    );
    put(&app.source.join("box/bin/mcp-catalog.json"), b"catalog");
    std::os::unix::fs::symlink("notes.md", app.source.join("Chats/c1/latest")).unwrap();
    app
}

#[derive(Debug, Eq, PartialEq)]
enum Entry {
    Directory,
    Link(PathBuf),
    File(Vec<u8>),
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Entry> {
    fn visit(root: &Path, folder: &Path, seen: &mut BTreeMap<PathBuf, Entry>) {
        for entry in fs::read_dir(folder).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            let path = entry.path();
            let key = path.strip_prefix(root).unwrap().to_path_buf();
            let value = if kind.is_dir() {
                visit(root, &path, seen);
                Entry::Directory
            } else if kind.is_symlink() {
                Entry::Link(fs::read_link(path).unwrap())
            } else {
                Entry::File(fs::read(path).unwrap())
            };
            seen.insert(key, value);
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

#[test]
fn staging_places_copies_and_keeps_the_app_home_and_live_destination_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let app = example(root.path());
    let before = snapshot(&app.source);
    let mut boundaries = Vec::new();
    let staged = app.stage(|step| boundaries.push(step));
    assert_eq!(
        boundaries,
        [
            AppStep::Clone,
            AppStep::Import,
            AppStep::Database,
            AppStep::Rewrite,
            AppStep::Check
        ]
    );
    assert_eq!(snapshot(&app.source), before);
    assert!(!app.destination.join(layout::FILE).exists());
    assert!(!app.destination.join("chats").exists());
    assert_eq!(
        fs::read(staged.path.join("chats/c1/notes.md")).unwrap(),
        b"chat"
    );
    assert_eq!(
        fs::read_link(staged.path.join("chats/c1/latest")).unwrap(),
        Path::new("notes.md")
    );
    assert_eq!(
        fs::read(staged.path.join("attachments/c1/file with spaces.png")).unwrap(),
        b"attachment"
    );
    assert!(!staged.path.join("run/engine.json").exists());
    assert!(!staged.path.join("run/socket.key").exists());
    assert_eq!(
        fs::read(staged.path.join("run/link.json")).unwrap(),
        b"{\"name\":\"A fixture\"}"
    );
    assert_eq!(
        fs::read(staged.path.join("bin/mcp-catalog.json")).unwrap(),
        b"catalog"
    );
}

#[test]
fn binary_mac_preferences_become_box_projects_worktrees_and_settings() {
    let root = tempfile::tempdir().unwrap();
    let app = example(root.path());
    let project = app.source.join("Chats/c1");
    let name = project.to_str().unwrap();
    let records = serde_json::json!({
        (name): { "name": " Chat project ", "run": " cargo test " }
    });
    let mut preferences = plist::Dictionary::new();
    preferences.insert(
        "projectFolders".into(),
        plist::Value::Array(vec![name.into()]),
    );
    preferences.insert(
        "projectSettings".into(),
        plist::Value::Data(serde_json::to_vec(&records).unwrap()),
    );
    preferences.insert(
        "worktrees".into(),
        plist::Value::Data(
            serde_json::to_vec(&serde_json::json!({
                (name): { "root": name, "project": app.home.join("project"), "branch": "legacy" }
            }))
            .unwrap(),
        ),
    );
    preferences.insert("git.branchPrefix".into(), "carried/".into());
    preferences.insert("toolApproval".into(), "askFirst".into());
    let file = app
        .home
        .join("Library/Preferences/run.kuvo.code.stage-test.plist");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    plist::to_file_binary(&file, &plist::Value::Dictionary(preferences)).unwrap();
    let before = fs::read(&file).unwrap();
    let staged = app.stage(|_| {});
    let read = |path| -> serde_json::Value {
        serde_json::from_slice(&fs::read(staged.path.join(path)).unwrap()).unwrap()
    };
    let imported = app.destination.join("chats/c1");
    assert_eq!(
        read("box/projects.json"),
        serde_json::json!({ "version": 1, "projects": [
            { "directory": imported, "name": "Chat project", "folders": [], "run": "cargo test" }
        ] })
    );
    assert_eq!(
        read("box/worktrees.json")["worktrees"],
        serde_json::json!([
            { "project": app.home.join("project"), "root": imported, "folder": imported, "branch": "legacy" }
        ])
    );
    assert_eq!(
        read("config/opencode/opencode.json"),
        serde_json::json!({"branch_prefix":"carried/"})
    );
    assert_eq!(
        read("box/git.json"),
        serde_json::json!({"moved":true,"formatVersion":1})
    );
    assert_eq!(
        read("box/approval.json"),
        serde_json::json!({"last":"askFirst"})
    );
    assert_eq!(fs::read(file).unwrap(), before);
}

#[test]
fn a_change_to_the_source_after_cloning_refuses_the_stage() {
    let root = tempfile::tempdir().unwrap();
    let app = example(root.path());
    let home = Home::new(app.destination.clone()).unwrap();
    let notes = app.source.join("Chats/c1/notes.md");
    let error = stage(
        &home,
        &app.home,
        &app.bundle,
        &app.release,
        &app.engine,
        |step| {
            if step == AppStep::Clone {
                fs::write(&notes, b"new note").unwrap();
            }
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("changed during the staged move"));
    assert!(!app.destination.join(layout::FILE).exists());
    let replayed = app.stage(|_| {});
    assert_eq!(
        fs::read(replayed.path.join("chats/c1/notes.md")).unwrap(),
        b"new note"
    );
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

fn cut(root: &Path, world: Option<&Path>, boundary: AppStep) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "staging_child", "--nocapture", "--ignored"])
        .env_clear()
        .env("KUVO_MOVE_TEST_MAC_ROOT", root)
        .env("KUVO_MOVE_TEST_MAC_STEP", format!("{boundary:?}"))
        .stdin(Stdio::piped());
    if let Some(world) = world {
        command.env("KUVO_MOVE_TEST_MAC_WORLD", world);
    }
    let mut child = ChildGuard(command.stdout(Stdio::piped()).spawn().unwrap());
    let stdout = child.0.stdout.take().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let watch = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if line.as_deref().is_ok_and(|line| line == "checkpoint") {
                let _ = send.send(());
                break;
            }
        }
    });
    receive.recv_timeout(Duration::from_secs(35)).unwrap();
    child.0.kill().unwrap();
    assert!(!child.0.wait().unwrap().success());
    watch.join().unwrap();
}

#[test]
fn each_staging_cut_rebuilds_the_same_unpublished_tree() {
    let root = tempfile::tempdir().unwrap();
    let app = example(root.path());
    let original = snapshot(&app.source);
    let expected_stage = app.stage(|_| {});
    let expected = snapshot(&expected_stage.path);
    for boundary in [
        AppStep::Clone,
        AppStep::Import,
        AppStep::Database,
        AppStep::Rewrite,
        AppStep::Check,
    ] {
        cut(root.path(), None, boundary);
        let actual = app.stage(|_| {});
        assert_eq!(snapshot(&actual.path), expected, "after {boundary:?}");
        assert_eq!(snapshot(&app.source), original);
        assert!(!app.destination.join(layout::FILE).exists());
    }
}

#[test]
#[ignore = "subprocess used by the staging crash scenario"]
fn staging_child() {
    let home = PathBuf::from(std::env::var_os("KUVO_MOVE_TEST_MAC_ROOT").unwrap());
    let stop = std::env::var("KUVO_MOVE_TEST_MAC_STEP").unwrap();
    // The parent seeded the app once, before taking its source inventory.
    let app = std::env::var_os("KUVO_MOVE_TEST_MAC_WORLD")
        .map(|file| {
            let app: App = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
            assert_eq!(app.home, home);
            app
        })
        .unwrap_or_else(|| paths(&home));
    app.stage(|step| {
        if format!("{step:?}") == stop {
            println!("checkpoint");
            io::stdout().flush().unwrap();
            io::stdin().read_exact(&mut [0]).unwrap();
        }
    });
}

#[test]
#[ignore = "supply KUVO_MOVE_GOLDEN_ROOT with the generated tagged scratch fixtures"]
fn tagged_mac_app_databases_migrate_on_their_staged_copies() {
    let root = PathBuf::from(std::env::var_os("KUVO_MOVE_GOLDEN_ROOT").unwrap());
    assert!(root.canonicalize().unwrap().starts_with("/private/tmp"));
    for tag in ["v0.7.2", "v0.8.0-beta.1"] {
        let app: App =
            serde_json::from_slice(&fs::read(root.join(tag).join("app-world.json")).unwrap())
                .unwrap();
        assert!(app.home.starts_with(&root));
        let original = snapshot(&app.source);
        let staged = app.stage(|_| {});
        assert_eq!(snapshot(&app.source), original, "{tag}");
        assert!(!app.destination.join(layout::FILE).exists());
        if tag == "v0.8.0-beta.1" {
            let saved: serde_json::Value = serde_json::from_slice(
                &fs::read(staged.path.join("run/outbox/sending/ltr_fixture.json")).unwrap(),
            )
            .unwrap();
            let link: serde_json::Value =
                serde_json::from_slice(&fs::read(app.source.join("phone-link.json")).unwrap())
                    .unwrap();
            assert_eq!(
                saved,
                serde_json::json!({
                    "from": "ses_source", "to": "ses_target", "text": "Keep this letter.", "wait": false,
                    "at": 1760000000000_u64, "peer": link["machines"]["fixture-machine"]["key"]
                })
            );
        }
        let db = Connection::open_with_flags(
            staged.path.join("data/opencode/opencode.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let mut query = db
            .prepare("SELECT title FROM session_v2 ORDER BY title")
            .unwrap();
        let titles = query
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            titles,
            ["chat", "chat (fork #1)", "git edit", "in worktree", "plain"],
            "{tag}"
        );
        let messages: i64 = db
            .query_row("SELECT count(*) FROM session_message", [], |row| row.get(0))
            .unwrap();
        let directory: String = db
            .query_row(
                "SELECT directory FROM session_v2 WHERE title = 'chat'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            Path::new(&directory),
            app.destination.join("chats/c1"),
            "{tag}"
        );
        assert_eq!(messages, 13, "{tag}");
        drop(query);
        drop(db);
        let expected = snapshot(&staged.path);
        let world = root.join(tag).join("app-world.json");
        for boundary in [
            AppStep::Clone,
            AppStep::Import,
            AppStep::Database,
            AppStep::Rewrite,
            AppStep::Check,
        ] {
            cut(&app.home, Some(&world), boundary);
            let replayed = app.stage(|_| {});
            assert_eq!(
                snapshot(&replayed.path),
                expected,
                "{tag} after {boundary:?}"
            );
            assert_eq!(snapshot(&app.source), original, "{tag} after {boundary:?}");
            assert!(!app.destination.join(layout::FILE).exists());
        }
    }
}
