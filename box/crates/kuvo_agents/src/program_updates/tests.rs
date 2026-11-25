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
    fn updated(&self) {
        self.told.fetch_add(1, Ordering::SeqCst);
    }
}

fn updates(home: &tempfile::TempDir, account: Arc<Stand>) -> (ProgramUpdates, Arc<AtomicUsize>) {
    let changes = Arc::new(AtomicUsize::new(0));
    let counted = changes.clone();
    let accounts: Accounts = Arc::new(move || vec![account.clone() as Arc<dyn Program>]);
    let changed = Arc::new(move |_: &str| {
        counted.fetch_add(1, Ordering::SeqCst);
    });
    (ProgramUpdates::at(home.path(), accounts, changed), changes)
}

struct Refuses;

impl Hold for Refuses {
    fn hold<'a>(
        &'a self,
        _: &'a str,
        _: u64,
    ) -> BoxFuture<'a, Result<Box<dyn FnOnce() + Send>, String>> {
        Box::pin(async { Err("The program's update wait expired.".to_owned()) })
    }
}

#[tokio::test]
async fn one_its_installer_cant_update_says_so_and_holds_nothing() {
    let home = tempfile::tempdir().unwrap();
    let account = Arc::new(Stand {
        definition: None,
        told: AtomicUsize::new(0),
    });
    let (updates, changes) = updates(&home, account.clone());
    let result = updates.update(account.clone(), None).await;
    assert_eq!(
        result.message.as_deref(),
        Some("Stand isn't installed.")
    );
    assert!(matches!(
        updates.update_of("stand", None),
        Some(ProgramUpdate::Failed { .. })
    ));
    assert!(!updates.is_updating("stand"));
    // Told as it started and as it ended; its record is gone.
    assert_eq!(changes.load(Ordering::SeqCst), 2);
    assert!(
        !home
            .path()
            .join("agents/program-draining/stand.json")
            .exists()
    );
    assert_eq!(account.told.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn turns_that_dont_finish_in_time_fail_it_with_why() {
    let home = tempfile::tempdir().unwrap();
    let account = Arc::new(Stand {
        definition: None,
        told: AtomicUsize::new(0),
    });
    let (updates, _) = updates(&home, account.clone());
    let updates = updates.with_hold(Arc::new(Refuses));
    let result = updates.update(account, None).await;
    assert_eq!(
        result.message.as_deref(),
        Some("The program's update wait expired.")
    );
    let Some(ProgramUpdate::Failed { at, .. }) = updates.update_of("stand", None) else {
        panic!("It didn't fail.");
    };
    // Seen as an older one, it stays; seen as itself, it's gone for every client.
    assert!(!updates.forget("stand", Some(at + 1)));
    assert!(updates.forget("stand", Some(at)));
    assert_eq!(updates.update_of("stand", None), None);
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
            Some(to) => format!("printf {to} > '{}'", version.display()),
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
        let found = maintenance(account.definition.as_ref().unwrap(), true).await;
        assert_eq!(found.installed.as_deref(), Some("1.0.0"));
        assert_eq!(found.shown.as_deref(), Some("stand update"));
        let running = updates.update(account.clone(), None);
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
        // Its records are retired: a next resident has nothing to take up.
        assert!(
            !home
                .path()
                .join("agents/program-draining/stand.json")
                .exists()
        );
        assert!(!home.path().join("agents/program-installing/stand").exists());
    }

    #[tokio::test]
    async fn one_left_as_it_was_says_why_in_the_installers_words() {
        let home = tempfile::tempdir().unwrap();
        let account = Arc::new(Stand {
            definition: Some(program(home.path(), None)),
            told: AtomicUsize::new(0),
        });
        let (updates, _) = updates(&home, account.clone());
        let result = updates.update(account.clone(), None).await;
        assert!(!result.updated);
        assert_eq!(
            result.message.as_deref(),
            Some("Stand is still 1.0.0 after “stand update”.\n\nno network")
        );
        assert_eq!(account.told.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_next_resident_takes_up_an_update_whose_installer_hadnt_started() {
        let home = tempfile::tempdir().unwrap();
        let account = Arc::new(Stand {
            definition: Some(program(home.path(), Some("1.1.0"))),
            told: AtomicUsize::new(0),
        });
        let (updates, _) = updates(&home, account.clone());
        let drains = home.path().join("agents/program-draining");
        std::fs::create_dir_all(&drains).unwrap();
        let attempt = uuid::Uuid::new_v4().to_string();
        let drain = json_drain(&attempt);
        std::fs::write(drains.join("stand.json"), &drain).unwrap();
        // One whose installer started is that installer's: its record holds turns until it exits.
        let other = tempfile::tempdir().unwrap();
        let (left, _) = updates_at(&other, account.clone());
        let other_drains = other.path().join("agents/program-draining");
        std::fs::create_dir_all(&other_drains).unwrap();
        std::fs::write(other_drains.join("stand.json"), &drain).unwrap();
        std::fs::create_dir_all(other.path().join("agents/program-installing")).unwrap();
        std::fs::write(
            other.path().join("agents/program-installing/stand"),
            format!("{} 0 {attempt}", std::process::id()),
        )
        .unwrap();
        left.resume();
        assert!(!left.is_updating("stand"));
        assert!(!other_drains.join("stand.json").exists());
        assert_eq!(
            installation(&other.path().join("agents"), "stand"),
            Some(Installation::Running)
        );

        updates.resume();
        assert!(updates.is_updating("stand"));
        let running = lock(&updates.running).get("stand").cloned().unwrap();
        assert!(running.await.updated);
        assert!(!drains.join("stand.json").exists());
    }

    fn json_drain(attempt: &str) -> String {
        serde_json::to_string(&Drain {
            attempt: attempt.into(),
            provider: "stand-cli".into(),
            deadline: now_ms() + HOLD_MOST,
        })
        .unwrap()
    }

    fn updates_at(
        home: &tempfile::TempDir,
        account: Arc<Stand>,
    ) -> (ProgramUpdates, Arc<AtomicUsize>) {
        super::updates(home, account)
    }
}
