use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

#[test]
fn versions_are_read_and_compared_part_by_part() {
    assert_eq!(
        version_in("2.1.156 (Claude Code)").as_deref(),
        Some("2.1.156")
    );
    assert_eq!(
        version_in("codex-cli 0.159.3-alpha.2\n").as_deref(),
        Some("0.159.3-alpha.2")
    );
    assert_eq!(version_in("no version"), None);
    assert!(compare_versions("0.9.0", "0.10.0").is_lt());
    assert!(compare_versions("1.2", "1.2.0").is_eq());
    assert!(compare_versions("1.0.0-beta", "1.0.0").is_lt());
    assert!(compare_versions("1.0.0-beta", "1.0.0-alpha").is_gt());
    let outdated = |installed: &str, latest: &str| Maintenance {
        installed: Some(installed.into()),
        latest: Some(latest.into()),
        ..Maintenance::default()
    };
    assert!(outdated("0.158.0", "0.159.3").is_outdated());
    assert!(!outdated("0.159.3", "0.159.3").is_outdated());
    assert!(!Maintenance::default().is_outdated());
}

#[test]
fn an_install_is_owned_by_where_it_really_is() {
    let prefix = |path: &str| npm_global_prefix(path, "@openai/codex");
    assert_eq!(
        prefix("/opt/homebrew/lib/node_modules/@openai/codex/bin/codex.js").as_deref(),
        Some("/opt/homebrew")
    );
    assert_eq!(
        prefix("/lib/node_modules/@openai/codex/bin/codex.js").as_deref(),
        Some("/")
    );
    // Someone else's node_modules (a project's) is yours to update.
    assert_eq!(
        prefix("/work/app/node_modules/x/lib/node_modules/@openai/codex/bin/codex.js"),
        None
    );
    // Mise's npm backend lays out its tools the same way; its Node's globals are npm's.
    assert_eq!(
        prefix(
            "/home/me/.local/share/mise/installs/npm-openai-codex/1.0/lib/node_modules/@openai/codex/bin/codex.js"
        ),
        None
    );
    assert_eq!(
        prefix("/home/me/.local/share/mise/installs/node/22/lib/node_modules/@openai/codex/bin/codex.js").as_deref(),
        Some("/home/me/.local/share/mise/installs/node/22"),
    );
    assert_eq!(
        homebrew_keg("/opt/homebrew/Cellar/codex/0.159.3/bin/codex"),
        Some(versions::Keg {
            cask: false,
            name: "codex".into(),
            prefix: "/opt/homebrew".into()
        }),
    );
    assert_eq!(
        homebrew_keg("/opt/homebrew/Caskroom/codex/0.159.3/codex"),
        Some(versions::Keg {
            cask: true,
            name: "codex".into(),
            prefix: "/opt/homebrew".into()
        }),
    );
    assert_eq!(homebrew_keg("/usr/local/bin/codex"), None);
    let formula = r#"{"formulae":[{"versions":{"stable":"0.159.3"}}]}"#;
    assert_eq!(
        versions::homebrew_latest(formula, false).as_deref(),
        Some("0.159.3")
    );
    let cask = r#"{"casks":[{"version":"0.160.0,abc"}]}"#;
    assert_eq!(
        versions::homebrew_latest(cask, true).as_deref(),
        Some("0.160.0")
    );
    assert_eq!(
        versions::homebrew_latest(r#"{"casks":[{"version":"latest"}]}"#, true),
        None
    );
}

#[test]
fn a_program_is_shown_as_you_type_it() {
    assert_eq!(shown_name(Path::new("/home/me/.local/bin/codex")), "codex");
    assert_eq!(shown_name(Path::new("/home/me/bin/codex.CMD")), "codex");
    assert_eq!(shown_name(Path::new("/home/me/bin/codex.exe")), "codex");
    assert_eq!(shown_name(Path::new("/opt/tools/codex.js")), "codex.js");
}

#[test]
fn what_an_installer_printed_is_read_as_words() {
    let printed = "\x1b[1;31merror\x1b[0m:\x1b[3Cfailed\x1b]0;title\x07\x1b(B\x1b[2Hnext";
    assert_eq!(versions::plain(printed), "error:   failed\nnext");
}

/// A stand-in account whose program a test updates, counting what it's told.
struct Stand {
    definition: Option<Definition>,
    told: AtomicUsize,
}

impl Program for Stand {
    fn program(&self) -> &str {
        "stand"
    }
    fn provider(&self) -> String {
        "stand-cli".into()
    }
    fn name(&self) -> String {
        "Stand".into()
    }
    fn definition(&self) -> Option<Definition> {
        self.definition.clone()
    }
    fn updated(&self) -> BoxFuture<'_, ()> {
        Box::pin(async {
            self.told.fetch_add(1, Ordering::SeqCst);
        })
    }
}

fn updates(home: &tempfile::TempDir, account: Arc<Stand>) -> (ProgramUpdates, Arc<AtomicUsize>) {
    let changes = Arc::new(AtomicUsize::new(0));
    let counted = changes.clone();
    let accounts: Accounts = Arc::new(move || vec![account.clone() as Arc<dyn Program>]);
    let changed = Arc::new(move |_: &str, _: Option<&UpdateResult>| {
        counted.fetch_add(1, Ordering::SeqCst);
    });
    (ProgramUpdates::at(home.path(), accounts, changed), changes)
}

#[tokio::test]
async fn one_its_installer_cant_update_says_so_and_can_be_forgotten() {
    let home = tempfile::tempdir().unwrap();
    let account = Arc::new(Stand {
        definition: None,
        told: AtomicUsize::new(0),
    });
    let (updates, changes) = updates(&home, account.clone());
    let result = updates.update(account.clone()).await;
    assert_eq!(result.message.as_deref(), Some("Stand isn't installed."));
    let Some(ProgramUpdate::Failed { at, .. }) = updates.update_of("stand", None) else {
        panic!("It didn't fail.");
    };
    assert!(!updates.is_updating("stand"));
    // Told as it started and as it ended.
    assert_eq!(changes.load(Ordering::SeqCst), 2);
    assert_eq!(account.told.load(Ordering::SeqCst), 0);
    // Seen as an older one, it stays; seen as itself, it's gone for every client.
    assert!(!updates.forget("stand", Some(at + 1)));
    assert!(updates.forget("stand", Some(at)));
    assert_eq!(updates.update_of("stand", None), None);
}

#[test]
fn side_by_side_keeps_leased_old_version_until_last_release_and_deletes_it() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let source = bin.join("stand");
    std::fs::write(&source, b"version-1-binary-payload").unwrap();

    let staged_v1 = store::prepare(home.path(), "stand", &source, None).unwrap();
    let v1_dir = staged_v1.version_dir.clone().unwrap();
    let v1_exe = staged_v1.executable.clone();
    assert!(v1_exe.is_file());
    assert!(!store::is_outdated_executable(home.path(), "stand", &v1_exe));

    // Two concurrent holders take leases on v1; the launch helper drops its own.
    let holder_a = store::Lease::acquire_for_executable(&v1_exe).unwrap();
    let holder_b = store::Lease::acquire_for_executable(&v1_exe).unwrap();
    drop(staged_v1);

    // Update replaces the user's binary and stages v2 beside v1.
    std::fs::write(&source, b"version-2-binary-payload-updated").unwrap();
    store::stage_current(home.path(), "stand", &source, None).unwrap();
    let staged_v2 = store::prepare(home.path(), "stand", &source, None).unwrap();
    let v2_dir = staged_v2.version_dir.clone().unwrap();
    let v2_exe = staged_v2.executable.clone();
    drop(staged_v2);

    assert_ne!(v1_dir, v2_dir);
    assert!(v1_dir.is_dir(), "v1 must stay while holders lease it");
    assert!(v2_dir.is_dir(), "v2 must be staged beside v1");
    assert!(store::is_outdated_executable(home.path(), "stand", &v1_exe));
    assert!(!store::is_outdated_executable(home.path(), "stand", &v2_exe));

    // Releasing the first holder keeps v1 alive for the second.
    drop(holder_a);
    assert!(v1_dir.is_dir(), "v1 must stay until the last holder drops");

    // Releasing the last holder deletes v1 right away while v2 remains.
    drop(holder_b);
    assert!(!v1_dir.exists(), "v1 must be removed once nothing uses it");
    assert!(v2_dir.is_dir(), "current v2 must remain");
}

#[test]
fn startup_sweeps_unused_versions_retired_dirs_and_legacy_markers() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let source = bin.join("stand");

    std::fs::write(&source, b"v1").unwrap();
    let v1 = store::prepare(home.path(), "stand", &source, None).unwrap();
    let v1_dir = v1.version_dir.clone().unwrap();
    let leased_v1 = store::Lease::acquire_for_executable(&v1.executable).unwrap();
    drop(v1);

    std::fs::write(&source, b"v2").unwrap();
    let v2 = store::prepare(home.path(), "stand", &source, None).unwrap();
    let v2_dir = v2.version_dir.clone().unwrap();
    // Hold v2 during v3 staging, then drop v2 AFTER v3 without triggering another sweep —
    // simulate a leftover unleased dir by creating a third stale version directly.
    drop(v2);

    std::fs::write(&source, b"v3").unwrap();
    let v3 = store::prepare(home.path(), "stand", &source, None).unwrap();
    let v3_dir = v3.version_dir.clone().unwrap();
    drop(v3);

    let program_dir = store::program_root(home.path(), "stand");
    let stale_dir = program_dir.join("v_deadbeef00000000");
    std::fs::create_dir_all(&stale_dir).unwrap();
    std::fs::write(stale_dir.join(".ready"), b"").unwrap();
    let retired_dir = program_dir.join(".retired-leftover");
    std::fs::create_dir_all(&retired_dir).unwrap();
    let staging_dir = program_dir.join(".staging-aborted");
    std::fs::create_dir_all(&staging_dir).unwrap();
    let legacy_draining = home.path().join("agents/program-draining");
    let legacy_installing = home.path().join("agents/program-installing");
    std::fs::create_dir_all(&legacy_draining).unwrap();
    std::fs::create_dir_all(&legacy_installing).unwrap();
    std::fs::write(legacy_draining.join("stand.json"), b"{}").unwrap();

    // Creating ProgramUpdates runs the startup sweep.
    let account = Arc::new(Stand {
        definition: None,
        told: AtomicUsize::new(0),
    });
    let _ = updates(&home, account);

    assert!(v1_dir.is_dir(), "actively leased v1 must survive startup sweep");
    assert!(v3_dir.is_dir(), "current v3 must survive startup sweep");
    assert!(!v2_dir.exists(), "unused v2 was already cleaned");
    assert!(!stale_dir.exists(), "unleased stale version must be swept");
    assert!(!retired_dir.exists(), "retired dir must be swept");
    assert!(!staging_dir.exists(), "aborted staging dir must be swept");
    assert!(!legacy_draining.exists(), "legacy drain markers must be swept");
    assert!(!legacy_installing.exists(), "legacy install markers must be swept");

    drop(leased_v1);
    assert!(!v1_dir.exists(), "v1 is removed as soon as its lease drops");
}

#[test]
#[ignore = "helper spawned by running_native_executable_does_not_block_side_by_side_update"]
fn hold_open_helper() {
    let started = std::env::var_os("KUVO_STORE_TEST_STARTED").unwrap();
    let gate = std::env::var_os("KUVO_STORE_TEST_GATE").unwrap();
    std::fs::write(&started, b"ready").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !Path::new(&gate).exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn running_native_executable_does_not_block_side_by_side_update() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let current_exe = std::env::current_exe().unwrap();
    let name = if cfg!(windows) { "stand.exe" } else { "stand" };
    let source = bin.join(name);
    std::fs::copy(&current_exe, &source).unwrap();

    let staged_v1 = store::prepare(home.path(), "stand", &source, None).unwrap();
    let v1_dir = staged_v1.version_dir.clone().unwrap();
    let v1_exe = staged_v1.executable.clone();
    let holder_lease = store::Lease::acquire_for_executable(&v1_exe).unwrap();
    drop(staged_v1);

    let started = home.path().join("started");
    let gate = home.path().join("gate");
    let mut child = std::process::Command::new(&v1_exe)
        .args([
            "--exact",
            "program_updates::tests::hold_open_helper",
            "--ignored",
            "--nocapture",
        ])
        .env("KUVO_STORE_TEST_STARTED", &started)
        .env("KUVO_STORE_TEST_GATE", &gate)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !started.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "staged native executable did not start in time"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    // While v1 is actively running, overwrite the user's binary in place (on Windows this fails
    // with sharing violation if the child ran directly from `source`) and stage v2 beside v1.
    let mut v2_bytes = std::fs::read(&current_exe).unwrap();
    v2_bytes.extend_from_slice(b"kuvo-v2-marker");
    std::fs::write(&source, &v2_bytes).unwrap();
    store::stage_current(home.path(), "stand", &source, None).unwrap();

    let staged_v2 = store::prepare(home.path(), "stand", &source, None).unwrap();
    let v2_dir = staged_v2.version_dir.clone().unwrap();
    drop(staged_v2);
    assert_ne!(v1_dir, v2_dir);
    assert!(v1_dir.is_dir(), "v1 must stay while running process holds lease");
    assert!(v2_dir.is_dir(), "v2 must be staged side by side");

    // Let the running v1 process exit, wait for it, then drop the holder lease.
    std::fs::write(&gate, b"go").unwrap();
    let status = child.wait().unwrap();
    assert!(status.success(), "v1 child exited with {status}");
    drop(holder_lease);

    assert!(!v1_dir.exists(), "v1 must be deleted after the running executable exits");
    assert!(v2_dir.is_dir(), "v2 must remain after v1 cleanup");
}

#[test]
fn news_of_a_version_moved_past_is_none() {
    let home = tempfile::tempdir().unwrap();
    let account = Arc::new(Stand {
        definition: None,
        told: AtomicUsize::new(0),
    });
    let (updates, _) = updates(&home, account);
    let at = now_ms();
    lock(&updates.kept).insert(
        "stand".into(),
        ProgramUpdate::Updated {
            version: "1.1.0".into(),
            at,
        },
    );
    assert!(updates.update_of("stand", Some("1.1.0")).is_some());
    assert_eq!(updates.update_of("stand", Some("1.2.0")), None);
    lock(&updates.kept).insert(
        "stand".into(),
        ProgramUpdate::Current {
            version: "1.1.0".into(),
            at: at - NEWS_KEPT - 1,
        },
    );
    assert_eq!(updates.update_of("stand", None), None);
}

#[cfg(unix)]
mod programs {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// A program at `<home>/.codex/bin/stand` (its own installer's), whose `update` moves it to
    /// `to` (or leaves it, saying why).
    fn program(home: &Path, to: Option<&str>) -> Definition {
        let bin = home.join(".codex/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let version = home.join("version");
        std::fs::write(&version, "1.0.0").unwrap();
        let update = match to {
            Some(to) => format!(
                "printf {to} > '{}'; printf '\\n' >> \"$0\"",
                version.display()
            ),
            None => "echo 'no network' >&2; exit 1".into(),
        };
        let script = format!(
            "#!/bin/sh\nif [ \"$1\" = update ]; then {update}; else printf 'stand %s\\n' \"$(cat '{}')\"; fi\n",
            version.display()
        );
        let executable = bin.join("stand");
        std::fs::write(&executable, script).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        Definition {
            name: "Stand".into(),
            executable,
            npm_package: None,
            native: Some(Native {
                args: vec!["update".into()],
                markers: vec!["/.codex/".into()],
                dist_tag: None,
            }),
        }
    }

    #[tokio::test]
    async fn an_update_that_moves_the_version_is_updated_and_every_account_runs_it() {
        let home = tempfile::tempdir().unwrap();
        let account = Arc::new(Stand {
            definition: Some(program(home.path(), Some("1.1.0"))),
            told: AtomicUsize::new(0),
        });
        let (updates, _) = updates(&home, account.clone());
        let found = updates
            .maintenance(account.definition.as_ref().unwrap(), true)
            .await;
        assert_eq!(found.installed.as_deref(), Some("1.0.0"));
        assert_eq!(found.shown.as_deref(), Some("stand update"));
        let running = updates.update(account.clone());
        assert!(updates.is_updating("stand"));
        assert!(matches!(
            updates.update_of("stand", None),
            Some(ProgramUpdate::Updating { .. })
        ));
        let result = running.await;
        assert_eq!(
            (result.updated, result.version.as_deref()),
            (true, Some("1.1.0"))
        );
        assert!(matches!(
            updates.update_of("stand", Some("1.1.0")),
            Some(ProgramUpdate::Updated { .. })
        ));
        assert_eq!(account.told.load(Ordering::SeqCst), 1);
        // The updated version is staged under run/agents/programs/stand.
        assert!(store::program_root(home.path(), "stand").join("current").is_file());
    }

    #[tokio::test]
    async fn one_left_as_it_was_says_why_in_the_installers_words() {
        let home = tempfile::tempdir().unwrap();
        let account = Arc::new(Stand {
            definition: Some(program(home.path(), None)),
            told: AtomicUsize::new(0),
        });
        let (updates, _) = updates(&home, account.clone());
        let result = updates.update(account.clone()).await;
        assert!(!result.updated);
        assert_eq!(
            result.message.as_deref(),
            Some("Stand is still 1.0.0 after “stand update”.\n\nno network")
        );
        assert_eq!(account.told.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn one_update_per_program_runs_at_a_time() {
        let home = tempfile::tempdir().unwrap();
        let count_file = home.path().join("invocations");
        let bin = home.path().join(".codex/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let version = home.path().join("version");
        std::fs::write(&version, "1.0.0").unwrap();
        let script = format!(
            "#!/bin/sh\nif [ \"$1\" = update ]; then echo x >> '{}'; sleep 0.15; printf 1.1.0 > '{}'; printf '\\n' >> \"$0\"; else printf 'stand %s\\n' \"$(cat '{}')\"; fi\n",
            count_file.display(),
            version.display(),
            version.display()
        );
        let executable = bin.join("stand");
        std::fs::write(&executable, script).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let account = Arc::new(Stand {
            definition: Some(Definition {
                name: "Stand".into(),
                executable,
                npm_package: None,
                native: Some(Native {
                    args: vec!["update".into()],
                    markers: vec!["/.codex/".into()],
                    dist_tag: None,
                }),
            }),
            told: AtomicUsize::new(0),
        });
        let (updates, _) = updates(&home, account.clone());
        let (first, second) = tokio::join!(
            updates.update(account.clone()),
            updates.update(account.clone()),
        );
        assert!(first.updated);
        assert!(second.updated);
        assert_eq!(
            std::fs::read_to_string(&count_file).unwrap().lines().count(),
            1
        );
        assert_eq!(account.told.load(Ordering::SeqCst), 1);
    }
}
