//! Round trips through the Rust backup service using scratch homes. The fake
//! SSH endpoint only supplies the remote shell.
use std::{
    fs,
    path::{Path, PathBuf},
};

use kuvo_setup::backup::{
    Backups, DestinationStore, DiskStore, PointId, Reason, RecoveryKey, Source, Standalone,
};
use kuvo_store::home::Home;

const AT: i64 = 1_795_046_400_000;
const APPROVAL: &[u8] = br#"{"fullAccess":true,"last":"acceptEdits","since":123}"#;
const PORTABLE_APPROVAL: &[u8] = br#"{"last":"acceptEdits","since":123}"#;

fn directory(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::create_dir_all(&path).unwrap();
    path.canonicalize().unwrap()
}

fn write(root: &Path, file: &str, contents: &[u8]) {
    let path = root.join(file);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn seed(home: &Path) {
    write(home, "folder-version.json", br#"{"version":6}"#);
    write(home, "box/approval.json", APPROVAL);
    write(home, "bots/bot_fixture/notes.md", b"Notes kept by the bot.");
    write(
        home,
        "run/waiting/ses_fixture/prompt.json",
        br#"{"body":"Review me"}"#,
    );
    write(
        home,
        "wakes/ses_fixture/wakes/wake.json",
        br#"{"at":1795046400000}"#,
    );
    let bytes: Vec<_> = (0..300_001).map(|index| (index % 251) as u8).collect();
    write(home, "attachments/ses_fixture/kept.bin", &bytes);
    write(
        home,
        "vault/values.json",
        b"no secret values in a computer backup",
    );
}

fn authority() -> Standalone {
    Standalone {
        origin: None,
        signer: None,
        setup: None,
    }
}

fn service(home: &Path) -> Backups {
    Backups::new(Home::new(home.to_owned()).unwrap())
}

fn verify(home: &Path, seed: &Path, id: &PointId) {
    for name in [
        "bots/bot_fixture/notes.md",
        "attachments/ses_fixture/kept.bin",
        "wakes/ses_fixture/wakes/wake.json",
    ] {
        assert_eq!(
            fs::read(home.join(name)).unwrap(),
            fs::read(seed.join(name)).unwrap(),
            "{name}"
        );
    }
    assert_eq!(
        fs::read(home.join("box/approval.json")).unwrap(),
        PORTABLE_APPROVAL
    );
    assert_eq!(
        fs::read(home.join("wakes/ses_fixture/paused")).unwrap(),
        b"restore-review"
    );
    assert!(!home.join("vault/values.json").exists());
    assert!(!home.join("run/waiting/ses_fixture/prompt.json").exists());
    assert_eq!(
        fs::read(home.join(format!(
            "backup/recovered/{id}/home/run/waiting/ses_fixture/prompt.json"
        )))
        .unwrap(),
        fs::read(seed.join("run/waiting/ses_fixture/prompt.json")).unwrap()
    );
}

#[test]
fn a_verified_point_keeps_data_and_holds_pending_work_after_source_loss() {
    let scratch = tempfile::tempdir().unwrap();
    let source_home = directory(scratch.path(), "source");
    let target_home = directory(scratch.path(), "target");
    let disk = directory(scratch.path(), "disk");
    seed(&source_home);
    let source = Source::generate().unwrap();
    let id = PointId::generate(Reason::Daily, AT).unwrap();
    let key = RecoveryKey::generate().unwrap();
    let store = DiskStore::open(&disk, &source_home, &source).unwrap();

    let point = service(&source_home)
        .export(&store, &source, &key, &id, AT, &authority())
        .unwrap();
    assert_eq!(point.version, 6);
    assert_eq!(store.list().unwrap(), [id.clone()]);
    let archived = store.get(&id, &disk).unwrap();
    let bytes = fs::read(&archived).unwrap();
    assert!(bytes.starts_with(b"KUVO backup 1\n"));

    let wrong = RecoveryKey::generate().unwrap();
    assert_eq!(
        service(&target_home)
            .restore(&store, &source, &wrong, &id, &authority())
            .unwrap_err()
            .to_string(),
        "Backup verification failed. Check the recovery key and archive."
    );
    assert!(!target_home.join("box/approval.json").exists());

    fs::rename(&source_home, scratch.path().join("old-disk")).unwrap();
    let result = service(&target_home)
        .restore(&store, &source, &key, &id, &authority())
        .unwrap();
    verify(&target_home, &scratch.path().join("old-disk"), &id);
    assert_eq!(result.held, ["home/run/waiting/ses_fixture/prompt.json"]);
    assert!(result.unavailable.is_empty());
    assert_eq!(
        result,
        service(&target_home)
            .restore(&store, &source, &key, &id, &authority())
            .unwrap()
    );
}

#[cfg(unix)]
#[test]
fn an_ssh_point_survives_source_loss_and_restores_the_same_bytes() {
    use kuvo_setup::backup::SshStore;
    use std::os::unix::fs::PermissionsExt;

    let scratch = tempfile::tempdir().unwrap();
    directory(scratch.path(), "remote");
    let fake = scratch.path().join("fake-ssh");
    fs::write(
        &fake,
        r#"#!/bin/sh
for argument; do command=$argument; done
HOME="$(dirname "$0")/remote" exec sh -c "$command"
"#,
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
    let source_home = directory(scratch.path(), "source");
    let target_home = directory(scratch.path(), "replacement");
    seed(&source_home);
    let source = Source::generate().unwrap();
    let key = RecoveryKey::generate().unwrap();
    let id = PointId::generate(Reason::Wipe, AT).unwrap();
    let store = SshStore::new(&source_home, scratch.path(), "fixture", &source)
        .unwrap()
        .with_program(fake.clone());
    let point = service(&source_home)
        .export(&store, &source, &key, &id, AT, &authority())
        .unwrap();
    assert_eq!(point.id, id);
    assert_eq!(store.list().unwrap(), [id.clone()]);
    fs::rename(&source_home, scratch.path().join("old-disk")).unwrap();
    let store = SshStore::new(&target_home, scratch.path(), "fixture", &source)
        .unwrap()
        .with_program(fake);
    let recovered = service(&target_home)
        .restore(&store, &source, &key, &id, &authority())
        .unwrap();
    assert_eq!(recovered.installed, 5);
    verify(&target_home, &scratch.path().join("old-disk"), &id);
}

#[test]
fn a_wal_snapshot_restores_committed_history_without_engine_signins_or_inbox() {
    use rusqlite::Connection;

    let scratch = tempfile::tempdir().unwrap();
    let home = directory(scratch.path(), "source");
    let replacement = directory(scratch.path(), "replacement");
    let disk = directory(scratch.path(), "disk");
    let db_dir = directory(&home, "data/opencode");
    let db = Connection::open(db_dir.join("opencode.db")).unwrap();
    db.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA wal_autocheckpoint=0;
         CREATE TABLE session(id TEXT PRIMARY KEY, title TEXT);
         CREATE TABLE cached_auth_tokens(value TEXT);
         CREATE TABLE session_inbox(value TEXT);
         INSERT INTO session VALUES ('ses_kept', 'committed history');
         INSERT INTO cached_auth_tokens VALUES ('fixture private token');
         INSERT INTO session_inbox VALUES ('fixture pending turn');",
    )
    .unwrap();
    assert!(db_dir.join("opencode.db-wal").metadata().unwrap().len() > 0);

    let source = Source::generate().unwrap();
    let key = RecoveryKey::generate().unwrap();
    let id = PointId::generate(Reason::Wipe, AT).unwrap();
    let store = DiskStore::open(&disk, &home, &source).unwrap();
    service(&home)
        .export(&store, &source, &key, &id, AT, &authority())
        .unwrap();
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM cached_auth_tokens", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM session_inbox", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    drop(db);
    fs::rename(&home, scratch.path().join("old-disk")).unwrap();
    service(&replacement)
        .restore(&store, &source, &key, &id, &authority())
        .unwrap();
    let db = Connection::open(replacement.join("data/opencode/opencode.db")).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT title FROM session WHERE id = 'ses_kept'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "committed history"
    );
    for table in ["cached_auth_tokens", "session_inbox"] {
        assert_eq!(
            db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn an_unfinished_layout_change_publishes_no_point() {
    let scratch = tempfile::tempdir().unwrap();
    let home = directory(scratch.path(), "source");
    let disk = directory(scratch.path(), "disk");
    let source = Source::generate().unwrap();
    let key = RecoveryKey::generate().unwrap();
    let id = PointId::generate(Reason::Update, AT).unwrap();
    let store = DiskStore::open(&disk, &home, &source).unwrap();
    write(
        &home,
        "folder-version.json",
        br#"{"version":6,"upgradingTo":7}"#,
    );
    assert!(
        service(&home)
            .export(&store, &source, &key, &id, AT, &authority())
            .is_err()
    );
    assert!(store.list().unwrap().is_empty());
    write(&home, "folder-version.json", br#"{"version":7}"#);
    write(&home, "run/update-trial", b"test release");
    let during = service(&home)
        .export(&store, &source, &key, &id, AT, &authority())
        .unwrap_err();
    assert!(during.to_string().contains("Wait for the update to finish"));
    assert!(store.list().unwrap().is_empty());
    fs::remove_file(home.join("run/update-trial")).unwrap();
    let point = service(&home)
        .export(&store, &source, &key, &id, AT, &authority())
        .unwrap();
    assert_eq!(point.version, 7);
    assert_eq!(store.list().unwrap(), [id]);
}

#[test]
fn an_incomplete_queue_payload_blocks_publication() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use serde_json::json;
    use sha2::{Digest, Sha256};

    let scratch = tempfile::tempdir().unwrap();
    let home = directory(scratch.path(), "source");
    let disk = directory(scratch.path(), "disk");
    let replacement = directory(scratch.path(), "replacement");
    let item = "wqi_aaaaaaaaaaaaaaaaaaaa";
    let payload = "managed file";
    let manifest = json!({ "items": [{ "id": item, "removed": true, "files": [{
        "name": ".draft.tmp", "size": payload.len(),
        "sha256": URL_SAFE_NO_PAD.encode(Sha256::digest(payload.as_bytes())),
    }] }] });
    write(
        &home,
        "run/work-queue/queues/usr_you.json",
        manifest.to_string().as_bytes(),
    );
    let source = Source::generate().unwrap();
    let key = RecoveryKey::generate().unwrap();
    let id = PointId::generate(Reason::Wipe, AT).unwrap();
    let store = DiskStore::open(&disk, &home, &source).unwrap();
    let missing = service(&home)
        .export(&store, &source, &key, &id, AT, &authority())
        .unwrap_err();
    assert!(
        missing
            .to_string()
            .contains("Work Queue attachment missing or unreadable")
    );
    assert!(store.list().unwrap().is_empty());

    let relative = format!("run/work-queue/files/{item}/.draft.tmp");
    write(&home, &relative, b"short");
    let short = service(&home)
        .export(&store, &source, &key, &id, AT, &authority())
        .unwrap_err();
    assert!(
        short
            .to_string()
            .contains("Work Queue attachment size differs from its record")
    );
    assert!(store.list().unwrap().is_empty());

    write(&home, &relative, b"changed file");
    let changed = service(&home)
        .export(&store, &source, &key, &id, AT, &authority())
        .unwrap_err();
    assert!(
        changed
            .to_string()
            .contains("Attachment changed during export")
    );
    assert!(store.list().unwrap().is_empty());

    write(&home, &relative, payload.as_bytes());
    service(&home)
        .export(&store, &source, &key, &id, AT, &authority())
        .unwrap();
    service(&replacement)
        .restore(&store, &source, &key, &id, &authority())
        .unwrap();
    assert_eq!(
        fs::read(replacement.join(&relative)).unwrap(),
        payload.as_bytes()
    );
    assert_eq!(
        fs::read(replacement.join("run/work-queue/queues/usr_you.json")).unwrap(),
        manifest.to_string().as_bytes()
    );
}

#[test]
fn daily_retention_keeps_one_point_on_each_of_the_last_thirty_days() {
    use kuvo_setup::backup::expired_points;

    const DAY: i64 = 86_400_000;
    let point = |reason, at| PointId::generate(reason, at).unwrap();
    let mut existing: Vec<_> = (0..=30)
        .map(|days| point(Reason::Daily, AT - DAY * days))
        .collect();
    let aged_out = existing[30].clone();
    let superseded = existing[0].clone();
    let newer = point(Reason::Daily, AT + 1);
    let update = point(Reason::Update, AT - DAY * 30);
    let vault = point(Reason::Vault, AT - DAY * 31);
    let future = point(Reason::Daily, AT + DAY);
    existing.extend([newer.clone(), update.clone(), vault.clone()]);
    let deleted = expired_points(&existing, AT).unwrap();
    assert_eq!(deleted, vec![superseded, aged_out, update]);
    assert!(!deleted.contains(&vault));
    assert!(!deleted.contains(&newer));
    assert!(expired_points(&[future], AT).unwrap().is_empty());
}

#[test]
fn relay_resumes_a_legacy_reservation_and_erase_fences_a_late_commit() {
    use std::time::SystemTime;

    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use kuvo_setup::backup::{
        Error, Result,
        relay::{Method, Receiver, Request, Scope},
    };
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};

    struct Accepted;
    impl Scope for Accepted {
        fn owners(&self, sender: &str, setup: &str, _: &Source, _: bool) -> Result<Vec<String>> {
            if sender == "A".repeat(43) && setup == format!("set_{}", "a".repeat(22)) {
                Ok(vec![sender.into()])
            } else {
                Err(Error::Message("Wait for accepted backup membership."))
            }
        }
    }

    let scratch = tempfile::tempdir().unwrap();
    let account = directory(scratch.path(), "account");
    let source = Source::generate().unwrap();
    let id = PointId::generate(Reason::Daily, AT).unwrap();
    let setup = format!("set_{}", "a".repeat(22));
    let owner = "A".repeat(43);
    let root = account
        .join(".kuvo-backups/relay")
        .join(&setup)
        .join(&owner);
    kuvo_store::files::create_private_dirs(&root).unwrap();
    let journal = root.join(".upload.json");
    let partial = root.join(".upload.partial");
    let payload = b"KUVO backup 1\nopaque fixture bytes";
    let first = 18;
    let sha256 = hex::encode(Sha256::digest(payload));
    let fields = json!({ "source": source, "id": id, "size": payload.len(), "sha256": sha256 });
    fs::write(&journal, fields.to_string()).unwrap();
    fs::write(&partial, &payload[..first]).unwrap();

    let receiver = Receiver::new(account, Accepted);
    let now = SystemTime::now();
    let apply = |method, action, offset, data: Option<&[u8]>| {
        let request: Request = serde_json::from_value(json!({
            "setup": setup, "id": id, "action": action, "offset": offset, "size": payload.len(),
            "sha256": sha256, "data": data.map(|bytes| URL_SAFE_NO_PAD.encode(bytes)),
        }))
        .unwrap();
        receiver.apply(&owner, method, &source, request, now)
    };

    let resume = apply(Method::Post, Some("begin"), None, None);
    assert_eq!(resume.status, 200, "{}", resume.value);
    assert_eq!(resume.value, json!({ "offset": first }));
    let saved: Value = serde_json::from_slice(&fs::read(&journal).unwrap()).unwrap();
    assert_eq!(saved["formatVersion"], 1);

    let next = apply(
        Method::Post,
        Some("append"),
        Some(first),
        Some(&payload[first..]),
    );
    assert_eq!(next.status, 200, "{}", next.value);
    assert_eq!(next.value, json!({ "offset": payload.len() }));
    let again = apply(
        Method::Post,
        Some("append"),
        Some(first),
        Some(&payload[first..]),
    );
    assert_eq!(again.status, 200, "{}", again.value);
    assert_eq!(again.value, next.value);

    let published = root.join(source.to_string()).join(format!("{id}.kuvo"));
    for _ in 0..2 {
        let commit = apply(Method::Post, Some("commit"), None, None);
        assert_eq!(commit.status, 200, "{}", commit.value);
        assert_eq!(fs::read(&published).unwrap(), payload);
        assert!(!journal.exists());
        assert!(!partial.exists());
    }
    let erased = apply(Method::Delete, None, None, None);
    assert_eq!(erased.status, 200, "{}", erased.value);
    let late = apply(Method::Post, Some("commit"), None, None);
    assert_eq!(late.status, 409, "{}", late.value);
    assert!(!published.exists());
}
