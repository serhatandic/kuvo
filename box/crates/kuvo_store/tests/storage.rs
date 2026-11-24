//! Real files, subprocess-owned locks, and SQLite connections in scratch homes.

use kuvo_store::{
    declarations::{Backup, DATA_DECLARATIONS, Owner, data},
    files,
    format::{self, Error as VersionError, ObjectFormat},
    home::Home,
    layout::{self, Layout},
    locks::{LaunchLock, OnBusy, OwnedLock, StaleLock, try_file_lock},
    sqlite,
};
use serde_json::json;
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{self, BufRead, BufReader, Read},
    process::{Child, Command, Stdio},
    time::Duration,
};

const WAIT: Duration = Duration::from_secs(2);
const STALE: Duration = Duration::from_secs(300);
const EXAMPLE: ObjectFormat = ObjectFormat::additive("example");

#[test]
fn format_steps_start_at_the_decoded_version_and_refuse_explicit_invalid_and_future_values() {
    let steps: [fn(i32) -> Result<i32, VersionError>; 2] = [|n| Ok(n + 2), |n| Ok(n * 3)];
    for (version, input) in [(None, 1), (Some(json!(1)), 3), (Some(json!(2.0)), 9)] {
        assert_eq!(
            format::upgrade("example", version.as_ref(), input, &steps).unwrap(),
            9
        );
    }
    for value in [
        json!(3),
        json!(-1),
        json!(1.5),
        json!("1"),
        json!(null),
        json!(9007199254740992_u64),
    ] {
        let result = format::upgrade("example", Some(&value), 0, &steps);
        assert!(matches!(
            result,
            Err(VersionError::Unsupported { current: 2, .. })
        ));
    }
}

#[test]
fn reading_a_versioned_record_keeps_extensions_and_preserves_unreadable_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("record");
    assert_eq!(EXAMPLE.read(&path).unwrap(), None);
    let old = json!({"name": "original", "extension": {"data":[1, 2]}});
    files::write_atomic(&path, old.to_string().as_bytes()).unwrap();
    let mut record = EXAMPLE.read(&path).unwrap().unwrap();
    assert_eq!(record.version, 0);
    assert_eq!(record.fields, *old.as_object().unwrap());
    record.fields.insert("enabled".into(), json!(true));
    assert_eq!(fs::read_to_string(&path).unwrap(), old.to_string());
    let encoded = EXAMPLE.encode(record.fields.clone());
    assert_eq!(encoded["formatVersion"], 1);
    assert_eq!(
        EXAMPLE.decode(encoded).unwrap(),
        format::Object {
            version: 1,
            fields: record.fields
        }
    );

    let reader = ObjectFormat {
        family: "example",
        current: 2,
    };
    let seen = reader
        .decode(json!({"formatVersion": 1, "content": "kept"}))
        .unwrap();
    assert_eq!(seen.version, 1);
    assert_eq!(seen.fields["content"], "kept");
    for (bytes, future) in [("{", false), ("[]", false), ("{\"formatVersion\":3}", true)] {
        fs::write(&path, bytes).unwrap();
        let result = reader.read(&path);
        assert!(matches!(result, Err(VersionError::Unsupported { .. })) == future);
        assert!(future || matches!(result, Err(VersionError::Unreadable { .. })));
        assert_eq!(fs::read_to_string(&path).unwrap(), bytes);
    }
}

#[test]
fn list_headers_and_receipt_fallback_keep_their_legacy_rules() {
    let records = vec![json!({"id": "kept"})];
    assert_eq!(
        format::read_additive_list("accounts", json!(records)).unwrap(),
        records
    );
    assert_eq!(
        format::read_additive_list(
            "accounts",
            json!(format::write_additive_list(records.clone()))
        )
        .unwrap(),
        records
    );
    assert!(
        format::read_additive_list("accounts", json!([{"formatVersion": 2}, {"id":"kept"}]))
            .is_err()
    );
    assert_eq!(
        format::token_payload("add/1.payload", "add/", 1).unwrap(),
        Some("payload")
    );
    assert_eq!(
        format::token_payload("pair/1.payload", "add/", 1).unwrap(),
        None
    );
    assert!(format::token_payload("add/2.payload", "add/", 1).is_err());

    let temp = tempfile::tempdir().unwrap();
    let uploads = temp.path().join("uploads");
    let current = temp.path().join("bin/.installed.json");
    files::write_atomic(&uploads.join("installed.json"), br#"{"name":"legacy"}"#).unwrap();
    assert_eq!(
        format::read_installed(&uploads).unwrap().unwrap().fields["name"],
        "legacy"
    );
    files::write_atomic(&current, br#"{"formatVersion":1,"name":"current"}"#).unwrap();
    assert_eq!(
        format::read_installed(&uploads).unwrap().unwrap().fields["name"],
        "current"
    );
    files::write_atomic(&current, br#"{"formatVersion":2}"#).unwrap();
    assert!(matches!(
        format::read_installed(&uploads),
        Err(VersionError::Unsupported { .. })
    ));
}

#[test]
fn layout_23_opens_in_place_and_only_an_empty_home_can_initialize() {
    let temp = tempfile::tempdir().unwrap();
    let home = Home::new(temp.path().join("new")).unwrap();
    layout::open(&home).unwrap();
    let marker = home.root().join(layout::FILE);
    assert_eq!(fs::read(&marker).unwrap(), b"{\"version\":23}\n");
    let stamp = fs::metadata(&marker).unwrap().modified().unwrap();
    layout::open(&home).unwrap();
    assert_eq!(fs::metadata(marker).unwrap().modified().unwrap(), stamp);
    assert_eq!(layout::backup_version(&home).unwrap(), 23);
    assert_eq!(fs::read_dir(home.root()).unwrap().count(), 1);

    let legacy = Home::new(temp.path().join("old")).unwrap();
    files::write_atomic(&legacy.run().join("queues.json"), b"{}").unwrap();
    assert!(matches!(
        layout::open(&legacy),
        Err(layout::Error::MoveRequired { version: 0 })
    ));
    assert_eq!(layout::backup_version(&legacy).unwrap(), 0);
    assert!(!legacy.root().join(layout::FILE).exists());
}

#[test]
fn an_existing_layout_routes_to_the_move_or_refuses_without_writing() {
    let temp = tempfile::tempdir().unwrap();
    let home = Home::new(temp.path().to_owned()).unwrap();
    let marker = home.root().join(layout::FILE);
    for (input, expected) in [
        (r#"{"version":9}"#, Some(Layout::Older { version: 9 })),
        (
            r#"{"version":22,"upgradingTo":23}"#,
            Some(Layout::Interrupted {
                version: 22,
                upgrading_to: 23,
            }),
        ),
        (r#"{"version":23,"upgradingTo":24}"#, None),
        (r#"{"version":21,"upgradingTo":23}"#, None),
        (r#"{"version":24}"#, None),
        (r#"{"version":-1}"#, None),
        (r#"{"version":"23"}"#, None),
        (r#"{"version":23,"upgradingTo":null}"#, None),
        ("{", None),
    ] {
        fs::write(&marker, input).unwrap();
        assert_eq!(layout::inspect(&home).ok(), expected, "{input}");
        assert!(layout::open(&home).is_err());
        assert_eq!(fs::read_to_string(&marker).unwrap(), input);
        assert_eq!(fs::read_dir(home.root()).unwrap().count(), 1);
    }
    fs::write(&marker, r#"{"version":22,"upgradingTo":23}"#).unwrap();
    assert!(matches!(
        layout::backup_version(&home),
        Err(layout::Error::ResumeMove {
            version: 22,
            upgrading_to: 23
        })
    ));
    layout::publish(&home).unwrap();
    assert_eq!(layout::inspect(&home).unwrap(), Layout::Current);
}

#[test]
fn write_new_cannot_clobber_an_already_published_key() {
    let temp = tempfile::tempdir().unwrap();
    let run = temp.path().join("home/run");
    let key = run.join("socket.key");
    files::write_new(&key, b"first\n").unwrap();
    assert_eq!(
        files::write_new(&key, b"second\n").unwrap_err().kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(fs::read(&key).unwrap(), b"first\n");
    assert_eq!(fs::read_dir(run).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn atomic_write_replaces_a_link_and_keeps_temporary_and_secret_files_private() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    let file = temp.path().join("run").join("record");
    fs::write(&target, b"target").unwrap();
    files::write_atomic(&file, b"old").unwrap();
    let run = file.parent().unwrap();
    assert_eq!(
        fs::metadata(run).unwrap().permissions().mode() & 0o777,
        0o700
    );
    fs::set_permissions(run, fs::Permissions::from_mode(0o755)).unwrap();
    fs::remove_file(&file).unwrap();
    symlink(&target, &file).unwrap();
    files::write_secret(&file, b"new").unwrap();
    assert!(!fs::symlink_metadata(&file).unwrap().is_symlink());
    assert_eq!(
        fs::metadata(run).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(fs::read(&file).unwrap(), b"new");
    assert_eq!(fs::read(&target).unwrap(), b"target");
    assert_eq!(fs::read_dir(run).unwrap().count(), 1);
}

#[test]
fn held_reader_keeps_old_bytes_while_fresh_opens_see_the_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("record");
    files::write_atomic(&file, b"old").unwrap();
    let mut reader = Probe::start("share", &file);
    assert_eq!(reader.line(), "ready");
    files::write_atomic(&file, b"new").unwrap();
    assert_eq!(fs::read(&file).unwrap(), b"new");
    reader.child.stdin.take();
    assert_eq!(reader.line(), "old");
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[cfg(windows)]
#[test]
fn replacement_refuses_a_process_without_delete_sharing_and_leaves_old_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("record");
    files::write_atomic(&file, b"old").unwrap();
    let mut reader = Probe::start("deny", &file);
    assert_eq!(reader.line(), "ready");
    let result = files::write_atomic(&file, b"new");
    assert!(result.is_err(), "a non-share-delete reader was bypassed");
    assert_eq!(fs::read(&file).unwrap(), b"old");
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    reader.child.stdin.take();
    assert_eq!(reader.line(), "old");
    reader.finish();
    files::write_atomic(&file, b"new").unwrap();
    assert_eq!(fs::read(&file).unwrap(), b"new");
}

#[test]
fn writer_locks_refuse_live_owners_and_reclaim_a_process_that_exits() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(".link.lock");
    let mut owner = Probe::start("owner", &path);
    assert_eq!(owner.line(), "ready");
    assert_eq!(
        OwnedLock::acquire(&path, Duration::ZERO)
            .unwrap_err()
            .kind(),
        io::ErrorKind::WouldBlock
    );
    owner.finish();
    let guard = OwnedLock::acquire(&path, WAIT).unwrap();
    assert_eq!(fs::read_dir(&path).unwrap().count(), 1);
    drop(guard);
    assert!(!path.exists());

    let path = temp.path().join("writer.lock");
    let guard = try_file_lock(&path).unwrap();
    assert!(try_file_lock(&path).is_err());
    drop(guard);
    assert!(try_file_lock(&path).is_ok());
}

#[test]
fn required_legacy_lock_fails_closed_and_best_effort_reports_no_guard() {
    let temp = tempfile::tempdir().unwrap();
    let lock = temp.path().join("stale");
    let guard = StaleLock::acquire(&lock, STALE, Duration::ZERO, OnBusy::Refuse)
        .unwrap()
        .unwrap();
    assert!(StaleLock::acquire(&lock, STALE, Duration::ZERO, OnBusy::Refuse).is_err());
    assert!(
        StaleLock::acquire(&lock, STALE, Duration::ZERO, OnBusy::Proceed)
            .unwrap()
            .is_none()
    );
    drop(guard);
    assert!(!lock.exists());
}

fn mark(run: &std::path::Path, pid: Option<u32>) {
    let lock = run.join(".launching");
    fs::create_dir(&lock).unwrap();
    #[cfg(unix)]
    File::open(lock)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(std::time::SystemTime::now() - Duration::from_secs(30)),
        )
        .unwrap();
    if let Some(pid) = pid {
        fs::write(run.join(".launching.pid"), format!("{pid}\n")).unwrap();
    }
}

#[test]
fn launch_lock_waits_for_a_live_or_older_owner() {
    for owner in [Some(std::process::id()), None] {
        let temp = tempfile::tempdir().unwrap();
        mark(temp.path(), owner);
        assert_eq!(
            LaunchLock::acquire(temp.path(), STALE, Duration::ZERO)
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(temp.path().join(".launching").is_dir());
        if let Some(pid) = owner {
            assert_eq!(
                fs::read_to_string(temp.path().join(".launching.pid")).unwrap(),
                format!("{pid}\n")
            );
        }
    }
}

#[test]
fn launch_lock_reclaims_a_dead_owner_and_only_one_breaker_takes_over() {
    let temp = tempfile::tempdir().unwrap();
    let run = temp.path();
    let mut child = Probe::start("exit", run);
    let pid = child.child.id();
    child.finish();
    mark(run, Some(pid));
    let breaking = run.join(".launching.breaking");
    fs::create_dir(&breaking).unwrap();
    fs::write(breaking.join("pid"), format!("{}\n", std::process::id())).unwrap();
    assert!(LaunchLock::acquire(run, STALE, Duration::ZERO).is_err());
    fs::write(breaking.join("pid"), format!("{pid}\n")).unwrap();
    let gone = run.join(format!(".launching.breaking.gone-{pid}"));
    fs::create_dir(&gone).unwrap();
    assert!(LaunchLock::acquire(run, STALE, Duration::ZERO).is_err());
    assert_eq!(
        fs::read_to_string(breaking.join("pid")).unwrap(),
        format!("{pid}\n")
    );
    fs::remove_dir(&gone).unwrap();

    let lock = LaunchLock::acquire(run, STALE, WAIT).unwrap();
    assert_eq!(
        fs::read_to_string(run.join(".launching.pid")).unwrap(),
        format!("{}\n", std::process::id())
    );
    drop(lock);
    assert!(!run.join(".launching").exists());
    assert!(!run.join(".launching.pid").exists());
    assert!(gone.is_dir());

    mark(run, Some(pid));
    let guard = LaunchLock::acquire(run, STALE, WAIT).unwrap();
    drop(guard);
    assert!(!run.join(".launching").exists());
}

#[test]
fn sqlite_adopts_its_schema_once_in_wal_and_leaves_a_future_database_untouched() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("insights.sqlite");
    let step = |db: &rusqlite::Connection| {
        db.execute_batch("CREATE TABLE usage(record TEXT); INSERT INTO usage VALUES ('kept');")
    };
    let db = sqlite::open_wal(&path, sqlite::INSIGHTS_BUSY, "Insights database", &[step]).unwrap();
    let pragma = |name| {
        db.pragma_query_value(None, name, |row| row.get::<_, i64>(0))
            .unwrap()
    };
    assert_eq!(pragma("busy_timeout"), 3000);
    assert_eq!(pragma("user_version"), 1);
    assert_eq!(
        db.pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))
            .unwrap(),
        "wal"
    );
    drop(db);
    let db = sqlite::open_wal(&path, sqlite::INSIGHTS_BUSY, "Insights database", &[step]).unwrap();
    assert_eq!(
        db.query_row("SELECT record FROM usage", [], |row| row
            .get::<_, String>(0))
            .unwrap(),
        "kept"
    );
    drop(db);

    let future = temp.path().join("future.sqlite");
    let db = rusqlite::Connection::open(&future).unwrap();
    db.pragma_update(None, "user_version", 2).unwrap();
    drop(db);
    let before = fs::read(&future).unwrap();
    assert!(matches!(
        sqlite::open_wal(&future, sqlite::INSIGHTS_BUSY, "Insights database", &[step]),
        Err(sqlite::Error::Format(VersionError::Unsupported { .. }))
    ));
    assert_eq!(fs::read(&future).unwrap(), before);
    let db = sqlite::open_index(&future, WAIT).unwrap();
    assert!(db.execute_batch("CREATE TABLE unexpected(v);").is_err());
}

#[test]
fn declarations_cover_each_required_kind_and_family_with_queue_specific_authority() {
    let required: BTreeSet<_> = include_str!("../../../test/data-declarations.test.ts")
        .split("const REQUIRED_KINDS = [")
        .nth(1)
        .unwrap()
        .split("];")
        .next()
        .unwrap()
        .lines()
        .filter_map(|line| line.trim().strip_prefix('"')?.strip_suffix("\","))
        .collect();
    let kinds: BTreeSet<_> = DATA_DECLARATIONS.iter().map(|row| row.kind).collect();
    assert_eq!(kinds.len(), DATA_DECLARATIONS.len());
    assert_eq!(kinds, required);
    let inventory: BTreeSet<_> = include_str!("../../../../docs/architecture/stored-formats.md")
        .split("## Stored formats")
        .nth(1)
        .unwrap()
        .split("## Message formats")
        .next()
        .unwrap()
        .lines()
        .filter_map(|line| {
            line.strip_prefix("| ")?
                .split_once(" |")
                .map(|(family, _)| family)
        })
        .filter(|name| *name != "Family" && !name.starts_with('-'))
        .collect();
    let families: BTreeSet<_> = DATA_DECLARATIONS
        .iter()
        .flat_map(|row| row.families.iter().copied())
        .collect();
    assert_eq!(families, inventory);
    assert_eq!(families.len(), 55);

    for kind in [
        "queueItems",
        "queueNotes",
        "queueFiles",
        "queueHistoryAndDelivery",
    ] {
        let owner = data(kind).unwrap().owner;
        for (queue, expected) in [
            ("ses_Example", Some(Owner::SessionComputer)),
            ("grp_example", Some(Owner::GroupHost)),
            ("usr_you", Some(Owner::LocalComputer)),
            ("future_example", None),
        ] {
            assert_eq!(owner.owner(Some(queue)), expected);
        }
    }
    let external: Vec<_> = DATA_DECLARATIONS
        .iter()
        .filter(|row| row.external.is_some())
        .map(|row| row.kind)
        .collect();
    assert_eq!(
        external,
        [
            "directoryWipeReceipts",
            "diskBackupArchives",
            "sshBackupArchives",
            "relayBackupArchives",
            "relayBackupUploads"
        ]
    );
    for kind in [
        "diskBackupArchives",
        "sshBackupArchives",
        "relayBackupArchives",
    ] {
        let row = data(kind).unwrap();
        assert_eq!(row.backup, Backup::Disabled);
        assert_eq!(row.owner.owner(None), Some(Owner::BackupSource));
        assert_eq!(row.external.unwrap().archive_suffix, Some(".kuvo"));
        assert_eq!(row.external.unwrap().version, 1);
    }
    let upload = data("relayBackupUploads").unwrap();
    assert_eq!(upload.owner.owner(None), Some(Owner::BackupReceiver));
    let upload = upload.external.unwrap();
    assert_eq!(
        upload.root,
        data("relayBackupArchives").unwrap().external.unwrap().root
    );
    assert_eq!(upload.journal, Some(".upload.json"));
    assert_eq!(upload.partial, Some(".upload.partial"));
    assert_eq!(upload.version, 1);
}

struct Probe {
    child: Child,
    stdout: BufReader<std::process::ChildStdout>,
}

impl Probe {
    fn start(mode: &str, file: &std::path::Path) -> Self {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "storage_probe", "--nocapture"])
            .env_clear()
            .env("STORE_PROBE_MODE", mode)
            .env("STORE_PROBE_PATH", file)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self { child, stdout }
    }

    fn line(&mut self) -> String {
        for line in self.stdout.by_ref().lines() {
            if let Some(value) = line.unwrap().strip_prefix("probe: ") {
                return value.to_owned();
            }
        }
        panic!("probe exited without replying");
    }

    fn finish(&mut self) {
        self.child.stdin.take();
        assert!(self.child.wait().unwrap().success());
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
#[ignore = "driven by the parent storage scenario"]
fn storage_probe() {
    let mode = std::env::var("STORE_PROBE_MODE").unwrap();
    if mode == "exit" {
        return;
    }
    let path = std::path::PathBuf::from(std::env::var_os("STORE_PROBE_PATH").unwrap());
    if mode == "owner" {
        let _lock = OwnedLock::acquire(&path, WAIT).unwrap();
        println!("probe: ready");
        let _ = io::stdin().read_to_end(&mut Vec::new());
        // Exit without unwinding to exercise dead-owner recovery.
        std::process::exit(0);
    }
    let mut options = File::options();
    options.read(true);
    #[cfg(windows)]
    if mode == "deny" {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
        options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    }
    let mut reader = options.open(path).unwrap();
    println!("probe: ready");
    let _ = io::stdin().read_to_end(&mut Vec::new());
    let mut bytes = String::new();
    reader.read_to_string(&mut bytes).unwrap();
    println!("probe: {bytes}");
}

#[test]
fn a_resume_stamp_distinguishes_file_replacement_at_the_same_path() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("record");
    assert_eq!(files::file_stamp(&path).unwrap(), None);
    files::write_atomic(&path, b"old").unwrap();
    let old = files::file_stamp(&path).unwrap().unwrap();
    files::write_atomic(&path, b"new").unwrap();
    let new = files::file_stamp(&path).unwrap().unwrap();
    assert_eq!(old.size, new.size);
    assert_ne!(old.file, new.file);
}

#[test]
fn a_failed_sqlite_schema_change_rolls_back_schema_and_version_together() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("insights.sqlite");
    let first = |db: &rusqlite::Connection| db.execute_batch("CREATE TABLE adopted(id);");
    let failed = |db: &rusqlite::Connection| db.execute_batch("INSERT INTO missing VALUES (1);");
    assert!(
        sqlite::open_wal(
            &path,
            sqlite::INSIGHTS_BUSY,
            "Insights database",
            &[first, failed]
        )
        .is_err()
    );
    let db = sqlite::open_index(&path, WAIT).unwrap();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name = 'adopted'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn best_effort_async_writes_still_wait_their_turn_in_one_process() {
    use kuvo_store::locks::SerialLock;
    use std::{
        future::{Future, poll_fn},
        task::Poll,
    };

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("answer");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        let writer = SerialLock::new(temp.path().join(".notes.lock"));
        let first = writer
            .acquire(STALE, Duration::ZERO, OnBusy::Proceed)
            .await
            .unwrap();
        let second_writer = writer.clone();
        let mut second =
            std::pin::pin!(second_writer.acquire(STALE, Duration::ZERO, OnBusy::Proceed));
        poll_fn(|cx| {
            assert!(second.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;

        files::write_atomic(&path, b"first\n").unwrap();
        drop(first);
        let _second = second.await.unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first\n");
        files::write_atomic(&path, b"first\nsecond\n").unwrap();
    });
    assert_eq!(fs::read(path).unwrap(), b"first\nsecond\n");
}

#[test]
fn owners_taking_turns_wait_for_each_other_while_a_lock_is_removed() {
    // Windows refuses to open a lock folder its last owner is removing: the
    // next owner waits for it rather than failing.
    let temp = tempfile::tempdir().unwrap();
    let path = std::sync::Arc::new(temp.path().join(".conversation.lock"));
    let workers = (0..4)
        .map(|_| {
            let path = path.clone();
            std::thread::spawn(move || {
                for _ in 0..200 {
                    let guard = OwnedLock::acquire(&path, Duration::from_secs(10))
                        .expect("a busy lock is waited for");
                    drop(guard);
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap();
    }
}
