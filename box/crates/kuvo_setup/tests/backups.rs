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
const PORTABLE: &[(&str, &[u8])] = &[
    ("bots/bot_fixture/notes.md", b"Notes kept by the bot."),
    ("bots/bot_fixture/MEMORY.md", b"Durable bot memory."),
    (
        "bots/bot_fixture/groups/group.md",
        b"Private group history.",
    ),
    ("groups/grp_fixture/messages.json", br#"["group history"]"#),
    (
        "agents/sessions/ses_fixture/messages.json",
        br#"["conversation"]"#,
    ),
    ("agents/usage/usage.json", br#"{"requests":12}"#),
    (
        "run/moves/decisions/mov_fixture.json",
        br#"{"decision":"switched"}"#,
    ),
    ("run/work-queue/acted.json", br#"{"formatVersion":1}"#),
    ("run/work-queue/seen.json", br#"{"formatVersion":1}"#),
    ("run/work-queue/handed.json", br#"{"formatVersion":1}"#),
    ("run/setup-merges/fixture.json", br#"{"formatVersion":1}"#),
    ("artifacts/copies/aa/aabb", &[20, 255]),
    (
        "wakes/ses_fixture/wakes/wake.json",
        br#"{"at":1795046400000}"#,
    ),
];
const EXCLUDED: &[&str] = &[
    "vault/values.json",
    "artifacts/key",
    "browser/profile/Cookies",
    "box/keys/GITHUB_TOKEN",
    "run/owned-data/forward-origins/fixture.json",
];

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
    for (name, value) in PORTABLE {
        write(home, name, value);
    }
    for name in EXCLUDED {
        write(home, name, b"fixture machine-local data");
    }
    write(
        home,
        "run/waiting/ses_fixture/prompt.json",
        br#"{"body":"Review me"}"#,
    );
    let bytes: Vec<_> = (0..300_001).map(|index| (index % 251) as u8).collect();
    write(home, "attachments/ses_fixture/kept.bin", &bytes);
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
    for name in PORTABLE
        .iter()
        .map(|(name, _)| *name)
        .chain(["attachments/ses_fixture/kept.bin"])
    {
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
    for name in EXCLUDED {
        assert!(!home.join(name).exists(), "{name}");
    }
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
    assert_eq!(store.list().unwrap(), std::slice::from_ref(&id));
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
    assert_eq!(store.list().unwrap(), std::slice::from_ref(&id));
    fs::rename(&source_home, scratch.path().join("old-disk")).unwrap();
    let store = SshStore::new(&target_home, scratch.path(), "fixture", &source)
        .unwrap()
        .with_program(fake);
    let recovered = service(&target_home)
        .restore(&store, &source, &key, &id, &authority())
        .unwrap();
    assert_eq!(recovered.installed, PORTABLE.len() + 3);
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
fn restore_reopens_external_attachments_and_group_recaps_and_reports_pruned_versions() {
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};

    let scratch = tempfile::tempdir().unwrap();
    let home = directory(scratch.path(), "source");
    let replacement = directory(scratch.path(), "target");
    let external = directory(scratch.path(), "external");
    let replacement_external = directory(scratch.path(), "target-external");
    let disk = directory(scratch.path(), "disk");
    let source = Source::generate().unwrap();
    let key = RecoveryKey::generate().unwrap();
    let id = PointId::generate(Reason::Wipe, AT).unwrap();
    let store = DiskStore::open(&disk, &home, &source).unwrap();
    let service = |home: &Path, attachments: &Path| {
        Backups::with_environment(
            Home::new(home.to_owned()).unwrap(),
            &[(
                "KUVO_MAC_ATTACHMENTS".into(),
                attachments.as_os_str().to_owned(),
            )]
            .into(),
        )
    };
    let file = "ses_fixture/msg_upload/.keep";
    let proof = [3, 0, 255];
    write(&external, file, &proof);
    let occupied = "ses_fixture/msg_upload/occupied.bin";
    let missing = "ses_fixture/msg_upload/expired.bin";
    write(&external, occupied, b"saved bytes");
    write(&replacement_external, occupied, b"replacement bytes");
    let agent = "agents/sessions/ses_fixture/messages.json";
    let parts: Vec<_> = [file, occupied, missing]
        .iter()
        .map(|leaf| json!({ "type": "file", "url": external.join(leaf) }))
        .collect();
    write(
        &home,
        agent,
        json!([{ "parts": parts }]).to_string().as_bytes(),
    );
    let transcript = "bots/bot_a/groups/prefix-grp_a.md";
    write(&home, transcript, b"Private transcript");
    let memory = |root: &Path| {
        json!({
            "bot_a": {
                "peer/grp_a": { "name": "Elsewhere", "recap": "Decision", "file": root.join(transcript), "at": 1 },
                "peer/grp_b": { "forgotten": true, "at": 2 },
            }
        })
    };
    write(
        &home,
        "groups/peer-memory.json",
        memory(&home).to_string().as_bytes(),
    );
    let journal = "artifacts/log/fixture.jsonl";
    let hash = hex::encode(Sha256::digest(b"latest artifact bytes"));
    let copy = format!("artifacts/copies/{}/{hash}", &hash[..2]);
    write(&home, &copy, b"latest artifact bytes");
    let artifact = |v| json!({ "formatVersion": 1, "a": { "id": "art_fixture" }, "v": v });
    let log = [
        artifact(json!({ "n": 1, "kept": false, "status": "superseded" })),
        artifact(json!({ "n": 2, "kept": true, "size": 21, "hash": format!("sha256:{hash}") })),
    ]
    .iter()
    .map(Value::to_string)
    .collect::<Vec<_>>()
    .join("\n")
        + "\n";
    write(&home, journal, log.as_bytes());
    service(&home, &external)
        .export(&store, &source, &key, &id, AT, &authority())
        .unwrap();
    let restored = service(&replacement, &replacement_external)
        .restore(&store, &source, &key, &id, &authority())
        .unwrap();
    assert_eq!(restored.unavailable.len(), 3);
    assert_eq!(
        restored.unavailable[0].entry,
        "artifacts/art_fixture/versions/1"
    );
    assert_eq!(
        restored.unavailable[0].reason,
        "artifact version bytes were not kept at the restore point"
    );
    assert!(
        restored
            .unavailable
            .iter()
            .any(|item| item.entry == format!("macAttachments/{occupied}")
                && item.reason
                    == "existing local attachment; recovered copy stored at a separate address")
    );
    assert!(restored.unavailable.iter().any(|item|
        item.entry == format!("macAttachments/{missing}")
        && item.reason == "referenced session attachment has no restored copy (missing, expired or moved)"));
    assert_eq!(
        fs::read(replacement.join(transcript)).unwrap(),
        fs::read(home.join(transcript)).unwrap()
    );
    let recovered: Value =
        serde_json::from_slice(&fs::read(replacement.join("groups/peer-memory.json")).unwrap())
            .unwrap();
    assert_eq!(recovered, memory(&replacement));
    let located = replacement_external.join(file);
    assert_eq!(fs::read(located).unwrap(), proof);
    let messages: Value =
        serde_json::from_slice(&fs::read(replacement.join(agent)).unwrap()).unwrap();
    assert_eq!(
        messages[0]["parts"][0]["url"],
        json!(replacement_external.join(file))
    );
    let replaced = Path::new(messages[0]["parts"][1]["url"].as_str().unwrap());
    assert!(replaced.starts_with(replacement.join("attachments")));
    assert_eq!(fs::read(replaced).unwrap(), b"saved bytes");
    assert_eq!(
        fs::read(replacement_external.join(occupied)).unwrap(),
        b"replacement bytes"
    );
    let unavailable = Path::new(messages[0]["parts"][2]["url"].as_str().unwrap());
    assert!(
        unavailable.starts_with(
            replacement
                .join("backup/recovered")
                .join(id.to_string())
                .join("unavailable-attachments")
        )
    );
    assert!(!unavailable.exists());
    assert_eq!(
        fs::read(replacement.join(copy)).unwrap(),
        b"latest artifact bytes"
    );
    assert_eq!(fs::read(replacement.join(journal)).unwrap(), log.as_bytes());
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

#[tokio::test]
async fn wipe_exports_match_the_selected_bytes_before_recording_verification() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use kuvo_setup::backup::WipeExport;
    use sha2::{Digest, Sha256};
    use std::{cell::Cell, collections::BTreeMap};

    let scratch = tempfile::tempdir().unwrap();
    let home = directory(scratch.path(), "source");
    let disk = directory(scratch.path(), "disk");
    let leaf = "attachments/ses_a/msg_a/proof.bin";
    let source = Source::generate().unwrap();
    let key = RecoveryKey::generate().unwrap();
    let id = PointId::generate(Reason::Wipe, AT).unwrap();
    let store = DiskStore::open(&disk, &home, &source).unwrap();
    let backup = service(&home);
    let hashes = BTreeMap::from([(
        leaf.into(),
        URL_SAFE_NO_PAD.encode(Sha256::digest(b"proof")),
    )]);
    let plan = || WipeExport {
        source: &source,
        id: &id,
        at: AT,
        key: &key,
        projects: &[],
        attachment_hashes: Some(&hashes),
    };
    for (bytes, message) in [
        (None, "Attachment omitted during export: "),
        (Some(b"other"), "Attachment changed during export: "),
    ] {
        if let Some(bytes) = bytes {
            write(&home, leaf, bytes);
        }
        let error = backup
            .export_before_wipe(&store, plan(), &authority(), || Ok(()))
            .unwrap_err();
        assert_eq!(error.to_string(), format!("{message}{leaf}."));
        assert!(store.list().unwrap().is_empty());
    }
    write(&home, leaf, b"proof");
    let added = "attachments/ses_a/msg_a/new.bin";
    write(&home, added, b"new");
    let error = backup
        .export_before_wipe(&store, plan(), &authority(), || Ok(()))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Attachment added during export: {added}.")
    );
    fs::remove_file(home.join(added)).unwrap();

    let scans = Cell::new(0);
    for _ in 0..2 {
        backup
            .export_before_wipe(&store, plan(), &authority(), || {
                scans.set(scans.get() + 1);
                Ok(())
            })
            .unwrap();
    }
    assert_eq!(scans.get(), 1);
    assert_eq!(store.list().unwrap(), std::slice::from_ref(&id));
    let receipt = home.join("verified");
    let wrong = RecoveryKey::generate().unwrap();
    assert!(
        backup
            .verify_for_wipe(&store, &source, &wrong, &id, || {
                fs::write(&receipt, b"unexpected")?;
                Ok(())
            })
            .is_err()
    );
    assert!(!receipt.exists());
    backup
        .verify_for_wipe(&store, &source, &key, &id, || {
            fs::write(&receipt, b"verified")?;
            Ok(())
        })
        .unwrap();
    assert_eq!(fs::read(receipt).unwrap(), b"verified");

    let setup = kuvo_setup::SetupRuntime::prepare(
        &home,
        "Wipe source".into(),
        false,
        Some("ws://127.0.0.1:1".into()),
    )
    .await
    .unwrap()
    .start(None)
    .unwrap();
    let mut routes = kuvo_server::Registry::new();
    let resident = kuvo_setup::backup::routes::register(
        &mut routes,
        Home::new(home.clone()).unwrap(),
        scratch.path().into(),
        setup.handle(),
        None,
        None,
        None,
    );
    let mut config = serde_json::json!({
        "version": 1, "enabled": true, "source": source, "key": key.expose(), "projects": [],
        "dest": { "kind": "disk", "path": disk }, "disk": 0,
    });
    let configuration = "backup/configuration.json";
    let device = kuvo_store::files::file_stamp(&disk)
        .unwrap()
        .unwrap()
        .device;
    config["disk"] = (device ^ 1).into();
    write(&home, configuration, &serde_json::to_vec(&config).unwrap());
    assert_eq!(
        resident
            .verify_configured_wipe(&source, &id, || Ok(()))
            .unwrap_err()
            .to_string(),
        "Reconnect the selected backup disk."
    );
    config["disk"] = device.into();
    write(&home, configuration, &serde_json::to_vec(&config).unwrap());
    let receipt = resident
        .verify_configured_wipe(&source, &id, || Ok("configured point verified"))
        .unwrap();
    assert_eq!(receipt, "configured point verified");
}

#[test]
fn the_beta_copy_is_complete_before_publication_and_unchanged_on_retry() {
    let scratch = tempfile::tempdir().unwrap();
    let home = directory(scratch.path(), "source");
    let beta_archive = "backups/beta/archive.kuvo";
    let beta_config = "backup/configuration.json";
    let wrapped_key = "box/vault/key.json";
    write(&home, beta_archive, b"the beta ciphertext");
    write(&home, beta_config, b"the beta recovery configuration");
    write(&home, wrapped_key, b"the beta wrapped key");
    let stale = "backups/.before-rust-stage-interrupted/incomplete";
    write(&home, stale, b"no published copy");
    #[cfg(unix)]
    std::os::unix::fs::symlink("configuration.json", home.join("backup/current")).unwrap();

    let backup = service(&home);
    let copy = backup.preserve_before_rust().unwrap();
    assert_eq!(copy, home.join("backups/before-rust"));
    for file in [beta_archive, beta_config, wrapped_key] {
        assert_eq!(
            fs::read(copy.join(file)).unwrap(),
            fs::read(home.join(file)).unwrap(),
            "{file}"
        );
    }
    #[cfg(unix)]
    assert_eq!(
        fs::read_link(copy.join("backup/current")).unwrap(),
        Path::new("configuration.json")
    );
    assert!(!copy.join("backups/before-rust").exists());
    assert!(!home.join(stale).exists());
    assert!(!copy.join(stale).exists());
    write(&home, beta_config, b"the Rust configuration");
    assert_eq!(backup.preserve_before_rust().unwrap(), copy);
    assert_eq!(
        fs::read(copy.join(beta_config)).unwrap(),
        b"the beta recovery configuration"
    );
}

#[test]
fn concurrent_disk_publication_keeps_the_first_point_and_its_bytes() {
    use std::sync::Barrier;

    let scratch = tempfile::tempdir().unwrap();
    let home = directory(scratch.path(), "home");
    let no_links = std::env::var_os("KUVO_TEST_BACKUP_NO_LINKS");
    let disk = match no_links.as_deref() {
        Some(folder) => tempfile::tempdir_in(folder).unwrap(),
        None => tempfile::tempdir_in(scratch.path()).unwrap(),
    };
    if no_links.is_some() {
        write(disk.path(), "link-test", b"fixture");
        assert!(
            fs::hard_link(disk.path().join("link-test"), disk.path().join("link-copy")).is_err(),
            "the fixture filesystem supports hard links"
        );
    }
    let source = Source::generate().unwrap();
    let id = PointId::generate(Reason::Wipe, AT).unwrap();
    let store = DiskStore::open(disk.path(), &home, &source).unwrap();
    let candidates = [("a", b"first candidate"), ("b", b"other candidate")];
    let gate = Barrier::new(candidates.len());
    let winner = std::thread::scope(|threads| {
        let attempts: Vec<_> = candidates
            .into_iter()
            .map(|(file, contents)| {
                write(scratch.path(), file, contents);
                let file = scratch.path().join(file);
                let (store, id, gate) = (&store, &id, &gate);
                threads.spawn(move || {
                    gate.wait();
                    (contents, store.put(id, &file))
                })
            })
            .collect();
        let mut winner = None;
        for attempt in attempts {
            let (contents, result) = attempt.join().unwrap();
            match result {
                Ok(()) => assert!(winner.replace(contents).is_none()),
                Err(error) => assert_eq!(error.to_string(), "That backup point already exists."),
            }
        }
        winner.expect("one publication must win")
    });
    assert_eq!(store.list().unwrap(), std::slice::from_ref(&id));
    assert_eq!(
        fs::read(store.get(&id, scratch.path()).unwrap()).unwrap(),
        winner
    );
}

#[tokio::test]
async fn vault_import_uses_a_separate_key_and_leaves_old_grants_behind() {
    use kuvo_server::{Caller, DispatchRequest, LocalOrigin, Transport};
    use kuvo_setup::backup::{
        Result,
        vault::{VaultBackend, VaultExportItem, VaultItem, VaultState},
    };
    use serde_json::json;
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };

    struct Vault {
        original: bool,
        imported: Mutex<Vec<VaultExportItem>>,
    }
    impl VaultBackend for Vault {
        fn holds_vault(&self) -> bool {
            true
        }
        fn state(&self) -> Result<VaultState> {
            Ok(VaultState {
                made: true,
                locked: false,
                tampered: false,
                generations: BTreeMap::new(),
            })
        }
        fn items(&self) -> Result<Vec<VaultItem>> {
            let item = |name: &str| VaultItem {
                id: "fixture-old-id".into(),
                name: name.into(),
                fields: vec!["token".into()],
                metadata: json!({ "kind": "key", "grants": { "everybody": "allow" }, "used": 21 })
                    .as_object()
                    .unwrap()
                    .clone(),
            };
            Ok(if self.original {
                vec![item("Example")]
            } else {
                self.imported
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|entry| item(&entry.name))
                    .collect()
            })
        }
        fn values(&self, _: &str) -> Result<BTreeMap<String, String>> {
            Ok(BTreeMap::from([(
                "token".into(),
                "synthetic vault value".into(),
            )]))
        }
        fn import_item(&self, item: VaultExportItem) -> Result<()> {
            self.imported.lock().unwrap().push(item);
            Ok(())
        }
    }
    let scratch = tempfile::tempdir().unwrap();
    let home = directory(scratch.path(), "source");
    let replacement = directory(scratch.path(), "target");
    let disk = directory(scratch.path(), "disk");
    let source = Source::generate().unwrap();
    let archive_key = RecoveryKey::generate().unwrap();
    let store = DiskStore::open(&disk, &home, &source).unwrap();
    let original = Vault {
        original: true,
        imported: Mutex::new(Vec::new()),
    };
    let target = Arc::new(Vault {
        original: false,
        imported: Mutex::new(Vec::new()),
    });
    let export = serde_json::to_value(
        service(&home)
            .export_vault(&store, &source, &original, AT)
            .unwrap(),
    )
    .unwrap();
    let id: PointId = serde_json::from_value(export["id"].clone()).unwrap();
    let key = RecoveryKey::parse(export["recoveryKey"].as_str().unwrap()).unwrap();
    assert_eq!(id.reason(), Reason::Vault);
    assert_ne!(key.expose(), archive_key.expose());

    let setup = kuvo_setup::SetupRuntime::prepare(
        &replacement,
        "Vault target".into(),
        false,
        Some("ws://127.0.0.1:1".into()),
    )
    .await
    .unwrap()
    .start(None)
    .unwrap();
    let mut routes = kuvo_server::Registry::new();
    let _backups = kuvo_setup::backup::routes::register(
        &mut routes,
        Home::new(replacement.clone()).unwrap(),
        scratch.path().into(),
        setup.handle(),
        None,
        Some(target.clone()),
        None,
    );
    let routes = routes.build(Default::default());
    for (given, status) in [(&archive_key, 409), (&key, 200)] {
        let reply = routes
            .dispatch(
                DispatchRequest {
                    method: http::Method::POST,
                    path: "kuvo/box/backups/vault/import".into(),
                    headers: Default::default(),
                    body: serde_json::to_vec(&json!({
                        "destination": { "kind": "disk", "path": disk },
                        "source": source, "id": id, "recoveryKey": given.expose(), "confirm": true,
                    }))
                    .unwrap()
                    .into(),
                    caller: Caller::Local(Transport::Http {
                        origin: LocalOrigin::App,
                        internal: false,
                    }),
                },
                Default::default(),
            )
            .await;
        assert_eq!(reply.status.as_u16(), status);
        assert_eq!(reply.headers[http::header::CACHE_CONTROL], "no-store");
        if status == 409 {
            assert!(target.imported.lock().unwrap().is_empty());
        }
    }
    {
        let imported = target.imported.lock().unwrap();
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].name, "Example");
        assert_eq!(imported[0].values["token"], "synthetic vault value");
        assert_eq!(
            imported[0].metadata,
            json!({ "kind": "key", "fields": ["token"] })
                .as_object()
                .unwrap()
                .clone()
        );
    }
    let replay = service(&replacement)
        .import_vault(&store, &source, &id, &key, &*target)
        .unwrap();
    assert_eq!(
        serde_json::to_value(replay).unwrap(),
        json!({
            "imported": 0, "skipped": ["Example"], "grants": "approval required",
        })
    );
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
    // The link is already visible when the receiver dies before syncing and
    // retiring the reservation. Retry must authenticate that exact publication.
    fs::create_dir_all(published.parent().unwrap()).unwrap();
    fs::hard_link(&partial, &published).unwrap();
    assert!(journal.exists());
    assert!(partial.exists());
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

    assert_eq!(apply(Method::Post, Some("begin"), None, None).status, 200);
    assert_eq!(
        apply(Method::Post, Some("append"), Some(0), Some(payload)).status,
        200
    );
    fs::create_dir_all(published.parent().unwrap()).unwrap();
    fs::hard_link(&partial, &published).unwrap();
    fs::remove_file(&published).unwrap(); // The directory entry was lost before its sync.
    assert_eq!(apply(Method::Delete, None, None, None).status, 200);
    assert!(!journal.exists());
    assert!(!partial.exists());
    assert_eq!(apply(Method::Post, Some("commit"), None, None).status, 409);
    assert!(!published.exists());
}

#[test]
fn relay_enforces_reservation_limits_and_reclaims_only_stale_uploads() {
    use std::time::{Duration, SystemTime};

    use kuvo_setup::backup::{
        Result,
        relay::{Method, Receiver, Request, Scope},
    };
    use serde_json::json;

    struct Accepted;
    impl Scope for Accepted {
        fn owners(&self, sender: &str, _: &str, _: &Source, _: bool) -> Result<Vec<String>> {
            Ok(vec![sender.into()])
        }
    }

    let scratch = tempfile::tempdir().unwrap();
    let account = directory(scratch.path(), "account");
    let source = Source::generate().unwrap();
    let id = PointId::generate(Reason::Daily, AT).unwrap();
    let setup = format!("set_{}", "a".repeat(22));
    let owner = "A".repeat(43);
    let root = account.join(".kuvo-backups/relay").join(&setup);
    let receiver = Receiver::new(account, Accepted);
    let begin = |sender: &str, size, now| {
        let request: Request = serde_json::from_value(json!({
            "setup": setup, "id": id, "action": "begin", "size": size, "sha256": "a".repeat(64),
        }))
        .unwrap();
        receiver.apply(sender, Method::Post, &source, request, now)
    };
    let now = SystemTime::now();
    let capacity = "The receiving computer doesn’t have enough backup capacity.";
    let archive_limit =
        "The encrypted backup exceeds the Relay archive limit or has an invalid digest.";
    for size in [0, 2 * 1024_u64.pow(3) + 1] {
        let denied = begin(&owner, size, now);
        assert_eq!(denied.status, 409, "{}", denied.value);
        assert_eq!(denied.value, json!({ "message": archive_limit }));
    }
    #[cfg(unix)]
    {
        let old_id = PointId::generate(Reason::Daily, AT - 86_400_000).unwrap();
        let old = directory(&root.join(&owner), &source.to_string()).join(format!("{old_id}.kuvo"));
        fs::File::create(&old)
            .unwrap()
            .set_len(32 * 1024_u64.pow(3))
            .unwrap();
        let denied = begin(&owner, 512, now);
        assert_eq!(denied.status, 409, "{}", denied.value);
        assert_eq!(denied.value, json!({ "message": capacity }));
        fs::remove_file(&old).unwrap();
    }

    let a = root.join(&owner);
    let journal = a.join(".upload.json");
    let partial = a.join(".upload.partial");
    let fresh = begin(&owner, 512, now);
    assert_eq!(fresh.status, 200, "{}", fresh.value);
    assert_eq!(fresh.value, json!({ "offset": 0 }));
    fs::remove_file(&partial).unwrap();
    let cut_before_partial = begin(&owner, 512, now);
    assert_eq!(
        cut_before_partial.status, 200,
        "{}",
        cut_before_partial.value
    );
    assert_eq!(cut_before_partial.value, fresh.value);

    let mark = |file: &Path, modified| {
        fs::File::options()
            .write(true)
            .open(file)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
    };
    let stale = now - Duration::from_secs(3601);
    let future = br#"{"formatVersion":99}"#;
    fs::write(&journal, future).unwrap();
    mark(&journal, stale);
    mark(&partial, stale);
    let unsupported = begin(&owner, 512, now);
    assert_eq!(unsupported.status, 409, "{}", unsupported.value);
    assert!(
        unsupported.value["message"]
            .as_str()
            .unwrap()
            .contains("format version 99 is unsupported")
    );
    assert_eq!(fs::read(&journal).unwrap(), future);

    fs::write(&journal, b"{").unwrap();
    mark(&journal, stale);
    mark(&partial, now);
    let broken = begin(&owner, 512, now);
    assert_eq!(broken.status, 409, "{}", broken.value);
    assert_eq!(fs::read(&journal).unwrap(), b"{");
    mark(&partial, stale);
    assert_eq!(begin(&owner, 512, now).status, 200);

    for sender in ['B', 'C', 'D'] {
        let opened = begin(&sender.to_string().repeat(43), 512, now);
        assert_eq!(opened.status, 200, "{}", opened.value);
    }
    let fifth = "E".repeat(43);
    let denied = begin(&fifth, 512, now);
    assert_eq!(denied.status, 409, "{}", denied.value);
    assert_eq!(denied.value, json!({ "message": capacity }));
    mark(&journal, stale);
    mark(&partial, stale);
    let opened = begin(&fifth, 512, now);
    assert_eq!(opened.status, 200, "{}", opened.value);
    assert!(!journal.exists());
    assert!(!partial.exists());
}

#[test]
#[ignore = "requires a scratch home from the current golden generator"]
fn a_golden_relay_reservation_is_resumed_and_committed() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use kuvo_setup::backup::{
        Result,
        relay::{Method, Receiver, Request, Scope},
    };
    use serde_json::{Value, json};

    struct Accepted;
    impl Scope for Accepted {
        fn owners(&self, sender: &str, _: &str, _: &Source, _: bool) -> Result<Vec<String>> {
            Ok(vec![sender.into()])
        }
    }
    let golden = PathBuf::from(std::env::var_os("KUVO_TEST_BACKUP_GOLDEN").unwrap());
    let setup = "set_8gOLfHXdUhmcVfSh9NOsAQ";
    let owner = "K9OVBGEgLiQvK66DwHcRnVukrjAFg9frt5I7Im2FM0M";
    let folder = format!(".kuvo-backups/relay/{setup}/{owner}");
    let scratch = tempfile::tempdir().unwrap();
    let account = directory(scratch.path(), "account");
    let staged = account.join(&folder);
    kuvo_store::files::create_private_dirs(&staged).unwrap();
    for name in [".upload.json", ".upload.partial"] {
        fs::copy(
            golden.join("account").join(&folder).join(name),
            staged.join(name),
        )
        .unwrap();
    }
    let journal = staged.join(".upload.json");
    let current: Value = serde_json::from_slice(&fs::read(&journal).unwrap()).unwrap();
    assert_eq!(current["formatVersion"], 1);
    let source: Source = serde_json::from_value(current["source"].clone()).unwrap();
    let id: PointId = serde_json::from_value(current["id"].clone()).unwrap();
    let now = journal.metadata().unwrap().modified().unwrap();
    let receiver = Receiver::new(account, Accepted);
    let ask = |method, action: &str, extra: Value| {
        let mut request = current.clone();
        request["setup"] = json!(setup);
        request["action"] = json!(action);
        request
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let request: Request = serde_json::from_value(request).unwrap();
        let answer = receiver.apply(owner, method, &source, request, now);
        assert_eq!(answer.status, 200, "{}", answer.value);
        answer.value
    };
    assert_eq!(
        ask(Method::Post, "begin", json!({})),
        json!({ "offset": 0 })
    );
    let bytes = [0; 16];
    assert_eq!(
        ask(
            Method::Post,
            "append",
            json!({
                "offset": 0, "data": URL_SAFE_NO_PAD.encode(bytes),
            })
        ),
        json!({ "offset": 16 })
    );
    ask(Method::Post, "commit", json!({}));
    assert_eq!(
        fs::read(staged.join(source.to_string()).join(format!("{id}.kuvo"))).unwrap(),
        bytes
    );
    assert!(!journal.exists());
    assert!(!staged.join(".upload.partial").exists());
}
