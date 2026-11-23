#![cfg(target_os = "macos")]

use kuvo_move::mac::{AppStep, CutoverStep, GenericPassword, KeychainItems, Run, stage};
use kuvo_store::{files, home::Home, layout};
use rusqlite::Connection;
use serde::Deserialize;
use std::{
    cell::Cell,
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
    std::os::unix::fs::symlink(
        app.source.join("Attachments/c1/file with spaces.png"),
        app.source.join("Chats/c1/absolute-attachment"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        app.home.join("unrelated"),
        app.source.join("Chats/c1/external"),
    )
    .unwrap();
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
        fs::read_link(staged.path.join("chats/c1/absolute-attachment")).unwrap(),
        app.destination.join("attachments/c1/file with spaces.png")
    );
    assert_eq!(
        fs::read_link(staged.path.join("chats/c1/external")).unwrap(),
        app.home.join("unrelated")
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
    preferences.insert("phoneLink".into(), false.into());
    preferences.insert(
        "phoneLink.relay".into(),
        "wss://relay.example.invalid/".into(),
    );
    put(
        &app.source.join("personalities.json"),
        br#"[{"id":"custom.keep","name":"Keep","description":"Be precise.","updated":"3"},{"id":"custom.gone","name":"Gone","description":"Already removed."}]"#,
    );
    put(
        &app.source.join("personalities-removed.json"),
        br#"[{"id":"custom.gone","removed":4}]"#,
    );
    preferences.insert(
        "favoriteModels".into(),
        "invalid,provider/model,provider/model,provider/with/slash,/model".into(),
    );
    put(
        &app.source.join("opencode/config/opencode/opencode.json"),
        br#"{"mcp":{"one":{"environment":{"TOKEN":"{env:KUVO_KEY_ONE}"},"args":["--token={file:/tmp/KUVO_KEY_TWO}","{env:UNRELATED}"]}}}"#,
    );
    put(
        &app.source.join("bots/b1/workspace/.opencode/opencode.json"),
        br#"{"mcp":{"one":{"environment":{"TOKEN":"{file:/tmp/KUVO_KEY_ONE}"}}}}"#,
    );
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
        serde_json::json!({
            "branch_prefix": "carried/",
            "mcp": {"one": {
                "environment": {"TOKEN": "{file:~/.kuvo-code/box/keys/KUVO_KEY_ONE}"},
                "args": ["--token={file:~/.kuvo-code/box/keys/KUVO_KEY_TWO}", "{env:UNRELATED}"]
            }}
        })
    );
    assert_eq!(
        read("bots/b1/workspace/.opencode/opencode.json")["mcp"]["one"]["environment"]["TOKEN"],
        "{file:~/.kuvo-code/box/keys/KUVO_KEY_ONE}"
    );
    assert_eq!(
        read("box/models.json"),
        serde_json::json!({"starred":[
            {"providerID":"provider", "id":"model"},
            {"providerID":"provider", "id":"with/slash"}
        ]})
    );
    assert_eq!(
        read("box/git.json"),
        serde_json::json!({"moved":true,"formatVersion":1})
    );
    assert_eq!(
        read("box/approval.json"),
        serde_json::json!({"last":"askFirst"})
    );
    let link = read("run/mac-link.json");
    assert_eq!(link["phones"], false);
    assert_eq!(link["relay"], "wss://relay.example.invalid");
    assert_eq!(
        read("bots/personalities.json"),
        serde_json::json!({
            "kuvo":1,
            "personalities":[{"id":"custom.keep","name":"Keep","description":"Be precise.","updated":3,
                "symbol":"person.crop.circle"}],
            "removed":[{"id":"custom.gone","removed":4}]
        })
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
    assert!(matches!(error, kuvo_move::Error::SourceChanged(_)));
    assert!(!app.destination.join(layout::FILE).exists());
    let replayed = app.stage(|_| {});
    assert_eq!(
        fs::read(replayed.path.join("chats/c1/notes.md")).unwrap(),
        b"new note"
    );
}

#[test]
fn staged_artifact_paths_and_ids_move_with_their_files() {
    use sha2::{Digest, Sha256};

    let root = tempfile::tempdir().unwrap();
    let app = example(root.path());
    let from = app.source.join("Chats/c1/notes.md");
    let to = app.destination.join("chats/c1/notes.md");
    let id = |path: &Path| {
        let sum = Sha256::digest(path.to_str().unwrap());
        format!("art_{}", hex::encode(&sum[..12]))
    };
    let log = "artifacts/log/2026-11.jsonl";
    let record = |path: &Path, version| serde_json::json!({"a":{"id":id(path),"path":path},"formatVersion":version});
    let original = format!("{}\n", record(&from, 1));
    let source = app.source.join("box").join(log);
    put(&source, original.as_bytes());
    let staged = app.stage(|_| {});
    let actual = fs::read_to_string(staged.path.join(log)).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&actual).unwrap(),
        record(&to, 1)
    );
    assert_eq!(fs::read_to_string(&source).unwrap(), original);

    let future = format!("{}{}\n", original, record(&from, 2));
    put(&source, future.as_bytes());
    let home = Home::new(app.destination.clone()).unwrap();
    let error = stage(
        &home,
        &app.home,
        &app.bundle,
        &app.release,
        &app.engine,
        |_| {},
    )
    .unwrap_err();
    assert!(error.to_string().contains("Artifact index"));
    assert_eq!(fs::read_to_string(source).unwrap(), future);
    assert_eq!(fs::read_to_string(staged.path.join(log)).unwrap(), future);
    assert!(!app.destination.join(layout::FILE).exists());
}

#[test]
fn a_future_app_record_is_refused_before_cloning() {
    let root = tempfile::tempdir().unwrap();
    let app = example(root.path());
    let bots = app.source.join("bots.json");
    let original = br#"[{"formatVersion":2},{"id":"future-bot"}]"#;
    put(&bots, original);
    let home = Home::new(app.destination.clone()).unwrap();
    let result = stage(
        &home,
        &app.home,
        &app.bundle,
        &app.release,
        &app.engine,
        |_| panic!("A future record reached staging"),
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Legacy Mac import list")
    );
    assert_eq!(fs::read(bots).unwrap(), original);
    assert!(!app.destination.join(layout::FILE).exists());
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
    cut_child(root, world, "staging_child", &format!("{boundary:?}"));
}

fn cut_child(root: &Path, world: Option<&Path>, test: &str, boundary: &str) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", test, "--nocapture", "--ignored"])
        .env_clear()
        .env("KUVO_MOVE_TEST_MAC_ROOT", root)
        .env("KUVO_MOVE_TEST_MAC_STEP", boundary)
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

fn publish(app: &App, complete: impl FnMut(CutoverStep)) -> kuvo_move::mac::Outcome {
    let path = app.home.join("generic-keychain");
    let keychain = path.is_dir().then(|| FileKeychain::new(path));
    publish_using(app, keychain.as_ref(), complete)
}

fn publish_using(
    app: &App,
    keychain: Option<&FileKeychain>,
    complete: impl FnMut(CutoverStep),
) -> kuvo_move::mac::Outcome {
    let home = Home::new(app.destination.clone()).unwrap();
    let system_account = app.home.join("unused-system-account");
    let run = Run {
        home: &home,
        account: &app.home,
        system_account: &system_account,
        bundle: &app.bundle,
        release: &app.release,
        engine: &app.engine,
    };
    match keychain {
        Some(keychain) => run.execute_with_keychain(keychain, complete),
        None => run.execute(complete),
    }
}

struct FileKeychain {
    root: PathBuf,
    fail_after_write: Option<String>,
    armed: Cell<bool>,
    fail_read: Cell<bool>,
}

impl FileKeychain {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            fail_after_write: None,
            armed: Cell::new(false),
            fail_read: Cell::new(false),
        }
    }
}

impl KeychainItems for FileKeychain {
    fn read(&self, service: &str) -> io::Result<Option<GenericPassword>> {
        if self.fail_after_write.as_deref() == Some(service) && self.fail_read.replace(false) {
            return Err(io::Error::other("read-back failed in the test"));
        }
        files::read(&self.root.join(service))?
            .map(|bytes| {
                let (account, value) = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                Ok(GenericPassword { account, value })
            })
            .transpose()
    }

    fn write(&self, service: &str, item: &GenericPassword) -> io::Result<()> {
        let body = serde_json::to_vec(&(&item.account, &item.value)).map_err(io::Error::other)?;
        files::write_atomic(&self.root.join(service), &body)?;
        if self.fail_after_write.as_deref() == Some(service) && self.armed.replace(false) {
            self.fail_read.set(true);
        }
        Ok(())
    }

    fn delete(&self, service: &str, account: &str) -> io::Result<()> {
        if self
            .read(service)?
            .is_some_and(|item| item.account != account)
        {
            return Err(io::Error::other("wrong test account"));
        }
        files::remove_file_synced(&self.root.join(service))
    }
}

#[test]
#[ignore = "subprocess used by the published-home crash scenario"]
fn publication_child() {
    let world = PathBuf::from(std::env::var_os("KUVO_MOVE_TEST_MAC_WORLD").unwrap());
    let app: App = serde_json::from_slice(&fs::read(world).unwrap()).unwrap();
    let stop = std::env::var("KUVO_MOVE_TEST_MAC_STEP").unwrap();
    let result = publish(&app, |step| {
        if format!("{step:?}") == stop {
            println!("\ncheckpoint");
            io::stdout().flush().unwrap();
            io::stdin().read_exact(&mut [0]).unwrap();
        }
    });
    assert_eq!(
        result.exit_code(),
        0,
        "{}",
        serde_json::to_value(result).unwrap()
    );
}

#[test]
#[ignore = "supply KUVO_MOVE_GOLDEN_ROOT with the generated tagged scratch fixtures"]
fn tagged_published_apps_resume_every_cutover_step() {
    let root = PathBuf::from(std::env::var_os("KUVO_MOVE_GOLDEN_ROOT").unwrap());
    assert!(root.canonicalize().unwrap().starts_with("/private/tmp"));
    for tag in ["v0.7.2", "v0.8.0-beta.1"] {
        let world = root.join(tag).join("app-world.json");
        let app: App = serde_json::from_slice(&fs::read(&world).unwrap()).unwrap();
        assert!(app.home.starts_with(&root));
        files::create_private_dirs(&app.home.join("unused-system-account")).unwrap();
        let backup = tempfile::tempdir_in(root.join(tag)).unwrap();
        let original_home = backup.path().join("old");
        files::clone_mac_entry(&app.home, &original_home, false).unwrap();
        let original_source = snapshot(&app.source);
        let reset = || {
            files::remove_tree_synced(&app.home).unwrap();
            files::clone_mac_entry(&original_home, &app.home, false).unwrap();
        };
        let complete = || {
            let result = publish(&app, |_| {});
            assert_eq!(
                result.exit_code(),
                0,
                "{tag}: {}",
                serde_json::to_value(result).unwrap()
            );
            assert_eq!(snapshot(&app.source), original_source);
            assert_eq!(
                layout::inspect(&Home::new(app.destination.clone()).unwrap()).unwrap(),
                layout::Layout::Current
            );
            let outside = app.home.join("proj/.git/worktrees/feature-one/gitdir");
            let backlink = fs::read_to_string(&outside).unwrap();
            assert!(
                Path::new(backlink.trim()).starts_with(app.destination.canonicalize().unwrap())
            );
            assert!(app.home.join("keychain/helper/phone-link.key").is_file());
            assert!(!app.home.join("keychain/app/phone-link.key").exists());
            let mut result = published_snapshot(&app.destination);
            result.insert(outside, Entry::File(backlink.into_bytes()));
            result
        };
        let expected = complete();
        let repeated =
            serde_json::to_value(publish(&app, |_| panic!("Done move replayed"))).unwrap();
        assert_eq!(repeated["result"], "done");
        assert!(repeated.get("started").is_none());
        assert!(repeated.get("message").is_none());
        for cutover in [
            CutoverStep::Staging(AppStep::Clone),
            CutoverStep::Staging(AppStep::Import),
            CutoverStep::Staging(AppStep::Database),
            CutoverStep::Staging(AppStep::Rewrite),
            CutoverStep::Staging(AppStep::Check),
            CutoverStep::Prepared,
            CutoverStep::KeyCopies,
            CutoverStep::Keys,
            CutoverStep::FirstPlacement,
            CutoverStep::Placed,
            CutoverStep::FirstGitRepair,
            CutoverStep::Git,
            CutoverStep::Layout,
            CutoverStep::Published,
            CutoverStep::OriginalKeys,
            CutoverStep::Settled,
        ] {
            reset();
            cut_child(
                &app.home,
                Some(&world),
                "publication_child",
                &format!("{cutover:?}"),
            );
            if cutover == CutoverStep::FirstPlacement {
                let home = Home::new(app.destination.clone()).unwrap();
                let outcome = kuvo_move::mac::resume(
                    &home,
                    &app.home,
                    &app.home.join("unused-system-account"),
                )
                .unwrap();
                assert_eq!(
                    outcome.exit_code(),
                    0,
                    "{}",
                    serde_json::to_value(outcome).unwrap()
                );
            }
            assert_eq!(complete(), expected, "{tag} after {cutover:?}");
        }
        reset();
        let locked = app.home.join("keychain-locked");
        put(&locked, b"locked");
        let result = publish(&app, |_| {});
        assert_eq!(result.exit_code(), 1);
        assert!(
            serde_json::to_string(&result)
                .unwrap()
                .contains("Keychain is locked")
        );
        assert!(app.home.join("keychain/app/phone-link.key").is_file());
        assert!(!app.destination.join(layout::FILE).exists());
        fs::remove_file(locked).unwrap();
        assert_eq!(complete(), expected);

        reset();
        let unreadable = app.home.join("helper-cannot-read");
        put(&unreadable, b"denied");
        let result = publish(&app, |_| {});
        assert_eq!(result.exit_code(), 1);
        assert!(app.home.join("keychain/app/phone-link.key").is_file());
        assert!(!app.home.join("keychain/helper/phone-link.key").exists());
        assert!(!app.destination.join(layout::FILE).exists());
        fs::remove_file(unreadable).unwrap();
        assert_eq!(complete(), expected);

        reset();
        let note = app.source.join("Chats/c1/notes.md");
        let mut wrote = false;
        let changed = publish(&app, |step| {
            if step == CutoverStep::FirstPlacement && !wrote {
                put(&note, b"written while placing");
                wrote = true;
            }
        });
        assert!(wrote);
        assert_eq!(changed.exit_code(), 0);
        assert_eq!(
            fs::read(app.destination.join("chats/c1/notes.md")).unwrap(),
            b"written while placing"
        );
        reset();
        let slug = |folder: PathBuf| {
            folder
                .to_str()
                .unwrap()
                .chars()
                .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
                .collect::<String>()
        };
        let projects = app.home.join(".claude/projects");
        let previous = projects.join(slug(app.source.join("Chats/c1")));
        let next = projects.join(slug(app.destination.join("chats/c1")));
        put(&previous.join("thread.jsonl"), b"one thread");
        put(
            &app.source.join("groups.json"),
            br#"[{"id":"grp_team","name":"Team","members":[]}]"#,
        );
        put(
            &app.source.join("groups/grp_team.json"),
            br#"[{"id":"first","text":"Ready","time":"2026-10-01T10:00:00Z"}]"#,
        );
        cut_child(
            &app.home,
            Some(&world),
            "publication_child",
            &format!("{:?}", CutoverStep::Staging(AppStep::Import)),
        );
        let messages = "groups/grp_team/messages.json";
        let private = app.destination.join("updates/rehome/rust-stage");
        assert!(private.join("groups/grp_team/group.json").is_file());
        let staged_messages = fs::read(private.join(messages)).unwrap();
        assert!(!app.destination.join(layout::FILE).exists());
        cut_child(
            &app.home,
            Some(&world),
            "publication_child",
            &format!("{:?}", CutoverStep::FirstHistory),
        );
        assert!(!previous.exists());
        assert!(!app.destination.join(layout::FILE).exists());
        let home = Home::new(app.destination.clone()).unwrap();
        let outcome =
            kuvo_move::mac::resume(&home, &app.home, &app.home.join("unused-system-account"))
                .unwrap();
        assert_eq!(
            outcome.exit_code(),
            0,
            "{}",
            serde_json::to_value(outcome).unwrap()
        );
        assert_eq!(fs::read(next.join("thread.jsonl")).unwrap(), b"one thread");
        assert_eq!(
            fs::read(app.destination.join(messages)).unwrap(),
            staged_messages
        );
        assert!(!previous.exists());
        reset();
        put(&app.destination.join("box/earlier.md"), b"old note");
        cut_child(
            &app.home,
            Some(&world),
            "publication_child",
            &format!("{:?}", CutoverStep::FirstDisplacement),
        );
        assert!(!app.destination.join(layout::FILE).exists());
        assert!(!app.destination.join("box/earlier.md").exists());
        let home = Home::new(app.destination.clone()).unwrap();
        let outcome =
            kuvo_move::mac::resume(&home, &app.home, &app.home.join("unused-system-account"))
                .unwrap();
        assert_eq!(
            outcome.exit_code(),
            0,
            "{}",
            serde_json::to_value(outcome).unwrap()
        );
        let kept = fs::read_dir(app.destination.join("trash"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(
            fs::read(kept.path().join("box/earlier.md")).unwrap(),
            b"old note"
        );
        assert_eq!(snapshot(&app.source), original_source);
        reset();
    }
}

fn published_snapshot(root: &Path) -> BTreeMap<PathBuf, Entry> {
    let mut tree = snapshot(root);
    for (file, clocks) in [
        (
            "updates/rehome/journal.json",
            &["began", "clonedAt", "kept"][..],
        ),
        ("updates/rehome/state.json", &["movedAt"][..]),
    ] {
        let Entry::File(bytes) = tree.get_mut(Path::new(file)).unwrap() else {
            panic!("Need {file}")
        };
        let mut data: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        for clock in clocks {
            data[clock] = serde_json::Value::Null;
        }
        *bytes = serde_json::to_vec(&data).unwrap();
    }
    tree
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
            println!("\ncheckpoint");
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

/// A complete staged release for command-level admission and copying tests.
fn staged_release(home: &Path) -> App {
    use sha2::{Digest, Sha256};

    let app = example(home);
    let mut digest = BTreeMap::new();
    for name in [
        "bin/kuvo-box",
        "bin/kuvo-start",
        "config/opencode/plugins/kuvo-code.js",
    ] {
        put(&app.release.join(name), b"fixture program");
        digest.insert(name, hex::encode(Sha256::digest(b"fixture program")));
    }
    put(
        &app.release.join("box.json"),
        &serde_json::to_vec(&serde_json::json!({
            "version": "move-test", "engine": "2.2.6", "files": digest
        }))
        .unwrap(),
    );
    app
}

fn claude_service(folder: &Path) -> String {
    use sha2::{Digest, Sha256};
    use unicode_normalization::UnicodeNormalization;
    let name: String = folder.to_str().unwrap().nfc().collect();
    format!(
        "Claude Code-credentials-{}",
        hex::encode(&Sha256::digest(name)[..4])
    )
}

#[test]
fn claude_keychain_copy_survives_each_effect_and_requires_its_adapter_on_resume() {
    for boundary in [
        CutoverStep::CredentialAside,
        CutoverStep::CredentialCopy,
        CutoverStep::CredentialRetired,
    ] {
        let root = tempfile::tempdir().unwrap();
        let app = staged_release(root.path());
        let folder = Path::new("agents/claude/accounts/cafe\u{301}");
        put(&app.source.join(folder).join("profile.json"), b"settings");
        let target = app.destination.join(folder);
        let items = FileKeychain::new(app.home.join("generic-keychain"));
        let source_name = claude_service(&app.source.join(folder));
        let target_name = claude_service(&target);
        let aside_name = format!("{target_name}.before-kuvo-move");
        let expected = GenericPassword {
            account: "the account".into(),
            value: b"fixture source token".to_vec(),
        };
        let previous = GenericPassword {
            account: "previous account".into(),
            value: b"fixture prior contents".to_vec(),
        };
        items.write(&source_name, &expected).unwrap();
        items.write(&target_name, &previous).unwrap();
        let world = app.home.join("credential-world.json");
        put(
            &world,
            &serde_json::to_vec(
                &serde_json::json!({"home":app.home,"S":app.source,"K":app.destination,
            "bundle":app.bundle,"release":app.release,"engine":app.engine}),
            )
            .unwrap(),
        );
        cut_child(
            &app.home,
            Some(&world),
            "publication_child",
            &format!("{boundary:?}"),
        );
        assert!(target.join("profile.json").is_file());
        assert_eq!(
            items.read(&source_name).unwrap().is_none(),
            boundary == CutoverStep::CredentialRetired
        );
        assert!(items.read(&aside_name).unwrap().as_ref() == Some(&previous));

        let journal =
            fs::read_to_string(app.destination.join("updates/rehome/publication.json")).unwrap();
        for item in [&expected, &previous] {
            assert!(!journal.contains(std::str::from_utf8(&item.value).unwrap()));
            assert!(!journal.contains(&serde_json::to_string(&item.value).unwrap()));
        }
        let home = Home::new(app.destination.clone()).unwrap();
        let system = app.home.join("unused-system-account");
        assert_eq!(
            kuvo_move::mac::resume(&home, &app.home, &system)
                .unwrap()
                .exit_code(),
            3
        );
        assert!(!app.destination.join(layout::FILE).exists());
        let recovered =
            kuvo_move::mac::resume_with_keychain(&home, &app.home, &system, &items).unwrap();
        assert_eq!(
            recovered.exit_code(),
            0,
            "{:?}",
            serde_json::to_value(recovered)
        );
        assert!(items.read(&target_name).unwrap().as_ref() == Some(&expected));
        assert!(items.read(&source_name).unwrap().is_none());
        assert!(items.read(&aside_name).unwrap().is_none());
        assert_eq!(layout::inspect(&home).unwrap(), layout::Layout::Current);
    }
}

#[test]
fn failed_claude_keychain_read_back_restores_the_existing_item_and_files() {
    let root = tempfile::tempdir().unwrap();
    let app = staged_release(root.path());
    let folder = Path::new("agents/claude/accounts/custom");
    let source = app.source.join(folder);
    let target = app.destination.join(folder);
    put(&source.join("profile.json"), b"settings");
    let source_name = claude_service(&source);
    let target_name = claude_service(&target);
    let expected = GenericPassword {
        account: "account".into(),
        value: b"fixture source".to_vec(),
    };
    let existing = GenericPassword {
        account: "account".into(),
        value: b"fixture previous".to_vec(),
    };
    let mut items = FileKeychain::new(app.home.join("generic-keychain"));
    items.write(&source_name, &expected).unwrap();
    items.write(&target_name, &existing).unwrap();
    items.fail_after_write = Some(target_name.clone());
    items.armed.set(true);
    let failed = publish_using(&app, Some(&items), |_| {});
    assert_eq!(failed.exit_code(), 1, "{:?}", serde_json::to_value(failed));
    assert!(items.read(&source_name).unwrap().as_ref() == Some(&expected));
    assert!(items.read(&target_name).unwrap().as_ref() == Some(&existing));
    assert!(
        items
            .read(&format!("{target_name}.before-kuvo-move"))
            .unwrap()
            .is_none()
    );
    assert!(source.join("profile.json").is_file());
    assert!(!target.exists());
    assert!(!app.destination.join(layout::FILE).exists());
    assert_eq!(publish_using(&app, Some(&items), |_| {}).exit_code(), 0);
    assert!(items.read(&source_name).unwrap().is_none());
    assert!(items.read(&target_name).unwrap().as_ref() == Some(&expected));
}

#[test]
fn a_write_during_copy_is_carried_by_the_same_move() {
    let root = tempfile::tempdir().unwrap();
    let app = staged_release(root.path());
    let home = Home::new(app.destination.clone()).unwrap();
    let mut clones = 0;
    let outcome = publish(&app, |step| {
        if step == CutoverStep::Staging(AppStep::Clone) {
            assert_eq!(
                kuvo_move::rehome_status(&home)["progress"],
                serde_json::json!({
                    "step": "upgrade", "title": "Bringing your settings along", "index": 5, "of": 11
                })
            );
            clones += 1;
            if clones == 1 {
                put(&app.source.join("Chats/c1/new.md"), b"written during copy");
            }
        }
    });
    assert_eq!(
        outcome.exit_code(),
        0,
        "{}",
        serde_json::to_value(outcome).unwrap()
    );
    assert_eq!(clones, 2);
    assert_eq!(
        kuvo_move::rehome_status(&home)["progress"],
        serde_json::Value::Null
    );
    assert_eq!(
        fs::read(app.destination.join("chats/c1/new.md")).unwrap(),
        b"written during copy"
    );
}

#[test]
fn linked_machine_settings_are_copied_without_replacing_the_external_file() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("machine");
    let source = root.path().join("settings.json");
    let saved = br#"{"instructions":["keep this"]}"#;
    put(&source, saved);
    put(&target.join("bin/kuvo-start"), b"old launcher");
    let linked = target.join("config/opencode/opencode.json");
    fs::create_dir_all(linked.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&source, &linked).unwrap();

    let home = Home::new(target.clone()).unwrap();
    kuvo_move::open(&home, kuvo_os::platform::Platform::Mac, None, |_| {}).unwrap();
    assert_eq!(fs::read_link(&linked).unwrap(), source);
    assert_eq!(fs::read(&source).unwrap(), saved);
    assert_eq!(
        fs::read(target.join("box/adopted-from/opencode.json")).unwrap(),
        saved
    );
    let settings: serde_json::Value =
        serde_json::from_slice(&fs::read(target.join("box/opencode.json")).unwrap()).unwrap();
    assert_eq!(settings["instructions"], serde_json::json!(["keep this"]));
    assert_eq!(layout::inspect(&home).unwrap(), layout::Layout::Current);
}

#[test]
fn a_cut_short_new_home_fences_startup_until_it_is_remade() {
    let root = tempfile::tempdir().unwrap();
    let app = staged_release(root.path());
    fs::remove_dir_all(&app.source).unwrap();
    put(&app.source.join("engine/2.2.6/opencode"), b"fresh engine");
    let marker = app.destination.join("updates/rehome/new");
    put(&marker, b"1\n");
    put(&app.destination.join("bin/partial-program"), b"partial");
    let home = Home::new(app.destination.clone()).unwrap();
    let open = || {
        kuvo_move::open(
            &home,
            kuvo_os::platform::Platform::Mac,
            Some(root.path()),
            |_| {},
        )
    };
    assert!(matches!(open(), Err(kuvo_move::Error::Busy(path)) if path == marker));
    assert!(!app.destination.join(layout::FILE).exists());
    let answer = kuvo_move::mac::new_home(
        &home,
        Some(&app.source.join("box")),
        None,
        Path::new("/not-the-test-account"),
        Some(&app.release),
        false,
    );
    assert_eq!(answer.exit_code(), 0, "{:?}", serde_json::to_value(answer));
    assert!(!marker.exists());
    assert!(open().is_ok());
    assert_eq!(layout::inspect(&home).unwrap(), layout::Layout::Current);
}

#[test]
fn sqlite_reader_sidecars_do_not_restart_the_move() {
    let root = tempfile::tempdir().unwrap();
    let app = staged_release(root.path());
    let database = app.source.join("opencode/data/opencode");
    fs::create_dir_all(&database).unwrap();
    let mut checks = 0;
    let outcome = publish(&app, |step| {
        if step == CutoverStep::Staging(AppStep::Check) {
            checks += 1;
            put(
                &database.join("opencode.db-shm"),
                b"rebuildable SQLite index",
            );
            put(&database.join("opencode.db-wal"), b"");
        }
    });
    assert_eq!(
        outcome.exit_code(),
        0,
        "{:?}",
        serde_json::to_value(outcome)
    );
    assert_eq!(checks, 1);
    assert_eq!(
        fs::read(database.join("opencode.db-shm")).unwrap(),
        b"rebuildable SQLite index"
    );
    assert_eq!(
        fs::metadata(database.join("opencode.db-wal"))
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn repeated_source_writes_stop_after_three_rounds() {
    let root = tempfile::tempdir().unwrap();
    let app = staged_release(root.path());
    let mut clones = 0;
    let file = app.source.join("Chats/c1/new.md");
    let outcome = publish(&app, |step| {
        if step == CutoverStep::FirstPlacement {
            clones += 1;
            put(&file, vec![b'x'; clones].as_slice());
        }
    });
    assert_eq!(outcome.exit_code(), 1);
    let reply = serde_json::to_value(outcome).unwrap();
    assert_eq!(clones, 3);
    assert_eq!(reply["result"], "failed");
    assert_eq!(reply["status"]["tries"], 1);
    assert_eq!(reply["status"]["nextTry"], serde_json::Value::Null);
    assert_eq!(
        reply["message"],
        "Something kept changing your sessions while Kuvo copied them, so nothing was moved. Quit the earlier Kuvo Code wherever it's open, then try again."
    );
    assert!(!app.destination.join(layout::FILE).exists());
    assert!(!app.destination.join("box").exists());
    assert_eq!(fs::read(&file).unwrap(), b"xxx");
    assert_eq!(publish(&app, |_| {}).exit_code(), 0);
    assert_eq!(
        fs::read(app.destination.join("chats/c1/new.md")).unwrap(),
        b"xxx"
    );
}

#[test]
fn an_unreadable_old_journal_fences_both_sides() {
    let root = tempfile::tempdir().unwrap();
    let app = staged_release(root.path());
    let journal = app.destination.join("updates/rehome/journal.json");
    let bytes = br#"{"v": 1, "step": "cle"#;
    put(&journal, bytes);
    let before = snapshot(&app.source);
    let outcome = publish(&app, |step| panic!("Reached {step:?}"));
    assert_eq!(outcome.exit_code(), 3);
    let reply = serde_json::to_value(outcome).unwrap();
    assert_eq!(reply["result"], "stuck");
    assert_eq!(reply["status"]["state"], "moving");
    assert_eq!(reply["status"]["failed"]["step"], serde_json::Value::Null);
    assert_eq!(fs::read(journal).unwrap(), bytes);
    assert!(!app.destination.join(layout::FILE).exists());
    assert_eq!(snapshot(&app.source), before);
}

#[test]
fn a_missing_pinned_engine_reports_offline_before_copying() {
    let root = tempfile::tempdir().unwrap();
    let app = staged_release(root.path());
    fs::remove_dir_all(&app.engine).unwrap();
    let before = snapshot(&app.source);
    let outcome = publish(&app, |step| panic!("Reached {step:?}"));
    assert_eq!(outcome.exit_code(), 1);
    let reply = serde_json::to_value(outcome).unwrap();
    assert_eq!(reply["result"], "refused");
    assert_eq!(reply["status"]["state"], "failed");
    assert_eq!(reply["status"]["failed"]["step"], "plan");
    assert_eq!(
        reply["message"],
        "Kuvo's engine couldn't be downloaded. Check that this Mac is online, then try again."
    );
    assert!(!app.destination.join(layout::FILE).exists());
    assert_eq!(snapshot(&app.source), before);
}

#[test]
fn a_future_engine_schema_is_refused_before_copying() {
    let root = tempfile::tempdir().unwrap();
    let app = staged_release(root.path());
    let file = app.source.join("opencode/data/opencode/opencode.db");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    {
        let db = Connection::open(&file).unwrap();
        db.execute_batch(
            "CREATE TABLE migration (id TEXT);
             INSERT INTO migration VALUES ('20270101000000_later');",
        )
        .unwrap();
    }
    let before = snapshot(&app.source);
    let outcome = publish(&app, |step| panic!("Reached {step:?}"));
    assert_eq!(outcome.exit_code(), 1);
    let reply = serde_json::to_value(outcome).unwrap();
    assert_eq!(reply["result"], "refused");
    assert_eq!(reply["status"]["failed"]["step"], "plan");
    assert_eq!(
        reply["message"],
        "Your sessions were last used by a version of Kuvo Code that this one doesn't know how to bring along (1 20270101000000_later). Nothing was changed. Update Kuvo if a newer version is out, then try again."
    );
    assert!(!app.destination.join("updates/rehome/rust-stage").exists());
    assert!(!app.destination.join(layout::FILE).exists());
    assert_eq!(snapshot(&app.source), before);
}

#[test]
fn browser_only_app_gets_a_home_without_copying_the_browser_profile() {
    let root = tempfile::tempdir().unwrap();
    let app = staged_release(root.path());
    fs::remove_dir_all(&app.source).unwrap();
    put(&app.source.join("Browser/history"), b"old profile");
    let before = snapshot(&app.source);
    let outcome = publish(&app, |_| {});
    assert_eq!(
        outcome.exit_code(),
        0,
        "{}",
        serde_json::to_value(outcome).unwrap()
    );
    assert_eq!(
        fs::read(app.destination.join("box/kind")).unwrap(),
        b"mac\n"
    );
    assert!(app.destination.join(layout::FILE).is_file());
    assert!(!app.destination.join("data").exists());
    assert!(!app.destination.join("Browser").exists());
    assert_eq!(snapshot(&app.source), before);
}

#[test]
fn an_external_writer_is_named_and_left_running_until_it_closes_the_file() {
    let root = tempfile::tempdir().unwrap();
    let app = staged_release(root.path());
    let file = app.source.join("Chats/c1/open.log");
    let mut child = ChildGuard(
        Command::new("/bin/zsh")
            .args(["-c", "exec 3>>\"$1\"; echo ready; read line", "zsh"])
            .arg(&file)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut ready = String::new();
    BufReader::new(child.0.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready.trim(), "ready");
    let before = snapshot(&app.source);
    let outcome = publish(&app, |step| panic!("Reached {step:?}"));
    assert_eq!(outcome.exit_code(), 1);
    let reply = serde_json::to_value(outcome).unwrap();
    assert_eq!(reply["result"], "refused");
    assert_eq!(
        reply["message"],
        "“zsh” is still using your earlier Kuvo Code's files. Quit it, then try again."
    );
    assert_eq!(snapshot(&app.source), before);
    assert!(!app.destination.join(layout::FILE).exists());
    assert!(child.0.try_wait().unwrap().is_none());
    drop(child.0.stdin.take());
    child.0.wait().unwrap();

    let outcome = publish(&app, |_| {});
    assert_eq!(
        outcome.exit_code(),
        0,
        "{}",
        serde_json::to_value(outcome).unwrap()
    );
    assert!(app.destination.join("chats/c1/open.log").is_file());
}

#[test]
fn a_collision_after_placing_rolls_back_without_replacing_the_new_file() {
    let root = tempfile::tempdir().unwrap();
    let app = staged_release(root.path());
    let file = app.destination.join("engine");
    let before = snapshot(&app.source);
    let outcome = publish(&app, |step| {
        if step == CutoverStep::FirstPlacement {
            put(&file, b"left by another installer");
        }
    });
    assert_eq!(outcome.exit_code(), 1);
    let reply = serde_json::to_value(outcome).unwrap();
    assert_eq!(reply["status"]["failed"]["step"], "place");
    assert_eq!(reply["result"], "failed");
    assert_eq!(fs::read(file).unwrap(), b"left by another installer");
    assert!(!app.destination.join("box").exists());
    assert!(!app.destination.join(layout::FILE).exists());
    assert_eq!(snapshot(&app.source), before);
}

#[test]
fn a_claude_accounts_chat_history_is_carried_into_staging() {
    let root = tempfile::tempdir().unwrap();
    let app = example(root.path());
    let slug = |path: &Path| {
        path.to_str()
            .unwrap()
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
            .collect::<String>()
    };
    let projects = "agents/claude/accounts/test/projects";
    let old = app
        .source
        .join(projects)
        .join(slug(&app.source.join("Chats/c1")))
        .join("thread.jsonl");
    put(&old, b"history\n");
    let copied = app.stage(|_| {});
    let next = copied
        .path
        .join(projects)
        .join(slug(&app.destination.join("chats/c1")))
        .join("thread.jsonl");
    assert_eq!(fs::read(next).unwrap(), b"history\n");
    assert_eq!(fs::read(old).unwrap(), b"history\n");
}
