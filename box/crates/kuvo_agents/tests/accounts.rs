// The real account index and account folders, with a disposable Kuvo home.
use kuvo_agents::accounts::{Accounts, AgentKind, Provider};
use kuvo_store::home::Home;
use serde_json::{Value, json};

#[tokio::test]
async fn changes_to_an_added_account_leave_the_default_and_other_accounts_in_place() {
    let temp = tempfile::tempdir().unwrap();
    let home = Home::new(temp.path().to_owned()).unwrap();
    let runtime = kuvo_agents::runtime::Runtime::for_home(&home);
    let index = runtime.accounts();
    let path = home.root().join("agents/accounts.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec(
            &json!([{"kind":"claude","id":"legacy","folder":"claude/earlier","futureField":true}]),
        )
        .unwrap(),
    )
    .unwrap();
    let before = index.all().await.unwrap();
    assert_eq!(before.len(), 6);
    let providers: Vec<_> = before[..5]
        .iter()
        .map(|account| account.provider().to_string())
        .collect();
    assert_eq!(
        providers,
        [
            "claude-code",
            "codex-cli",
            "cursor",
            "grok-cli",
            "antigravity"
        ]
    );
    assert!(index.home(&before[0]).is_none());
    assert_eq!(
        index.home(&before[5]).unwrap(),
        home.root().join("agents/claude/earlier")
    );
    let added = index
        .add(AgentKind::Codex, Some(" Work ".into()))
        .await
        .unwrap();
    assert_eq!(added.label.as_deref(), Some("Work"));
    let folder = index.home(&added).unwrap();
    std::fs::write(folder.join("fixture"), b"private").unwrap();
    index
        .rename(added.provider(), Some(" Office ".into()))
        .await
        .unwrap();
    let reopened = Accounts::for_home(&home);
    let accounts = reopened.all().await.unwrap();
    assert_eq!(accounts[6].label.as_deref(), Some("Office"));
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved[0], json!({"formatVersion":1}));
    assert_eq!(saved[1]["futureField"], true);
    assert!(
        reopened
            .remove(Provider::parse("codex-cli").unwrap())
            .await
            .unwrap()
            .is_none()
    );
    assert!(reopened.remove(added.provider()).await.unwrap().is_some());
    assert!(!folder.exists());
    assert_eq!(reopened.all().await.unwrap(), before);
}

#[tokio::test]
async fn future_account_index_keeps_its_bytes_on_read_and_write() {
    let temp = tempfile::tempdir().unwrap();
    let home = Home::new(temp.path().to_owned()).unwrap();
    let index = Accounts::for_home(&home);
    let path = home.root().join("agents/accounts.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let future = r#"[{"formatVersion":42},{"kind":"codex","id":"work"}]"#;
    std::fs::write(&path, future).unwrap();
    assert_eq!(
        index.all().await.unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
    assert!(index.add(AgentKind::Codex, None).await.is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), future);
    assert!(!home.root().join("agents/codex").exists());
}

#[tokio::test]
async fn the_runtime_reads_a_saved_agent_session_directory_without_a_vendor_process() {
    let temp = tempfile::tempdir().unwrap();
    let home = Home::new(temp.path().to_owned()).unwrap();
    let runtime = kuvo_agents::runtime::Runtime::for_home(&home);
    let session = home.root().join("agents/sessions/ses_test");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(
        session.join("session.json"),
        br#"{"formatVersion":1,"id":"ses_test","location":{"directory":"/project/folder"}} "#,
    )
    .unwrap();
    assert_eq!(
        runtime.session_folder("ses_test").await.unwrap().as_deref(),
        Some("/project/folder")
    );
    assert_eq!(runtime.session_folder("ses_missing").await.unwrap(), None);
    assert_eq!(
        runtime
            .session_folder("ses_test/../ses_test")
            .await
            .unwrap(),
        None
    );
}

fn known_sign_in(label: &str) -> kuvo_agents::account_check::AccountStatus {
    kuvo_agents::account_check::AccountStatus {
        installed: true,
        signed_in: true,
        label: Some(label.into()),
        identity: None,
        label_describes_method: false,
        message: None,
        ended: false,
        unchecked: false,
    }
}

#[tokio::test]
async fn the_last_sign_in_stays_visible_when_the_agent_cannot_answer() {
    use kuvo_agents::account_check::{CheckFailure, KnownAccount};
    let known = KnownAccount::new(std::time::Duration::ZERO);
    assert_eq!(
        known
            .read(false, || async { Ok(known_sign_in("Work")) })
            .await
            .unwrap(),
        known_sign_in("Work")
    );
    let old = known
        .read(false, || async { Err(CheckFailure::TimedOut) })
        .await
        .unwrap();
    assert_eq!(old.label.as_deref(), Some("Work"));
    assert!(old.signed_in && old.unchecked);
    assert_eq!(
        known
            .read(false, || async { panic!("A failed check is cached") })
            .await
            .unwrap(),
        old
    );
    let signed_out = known
        .read(true, || async {
            Ok(kuvo_agents::account_check::AccountStatus {
                signed_in: false,
                ..known_sign_in("Work")
            })
        })
        .await
        .unwrap();
    assert!(!signed_out.signed_in && !signed_out.unchecked);
}

#[tokio::test]
async fn a_probe_begun_before_sign_in_changed_cannot_restore_the_old_account() {
    use kuvo_agents::account_check::KnownAccount;
    use std::sync::{Arc, Mutex};
    use tokio::sync::oneshot;
    let known = Arc::new(KnownAccount::new(std::time::Duration::from_secs(60)));
    let (started, waiting) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let in_flight = tokio::spawn({
        let known = known.clone();
        let old = Mutex::new(Some((started, released)));
        async move {
            known
                .read(false, || {
                    let previous = old.lock().unwrap().take();
                    async move {
                        if let Some((started, release)) = previous {
                            started.send(()).unwrap();
                            release.await.unwrap();
                            return Ok(known_sign_in("Old"));
                        }
                        Ok(known_sign_in("New"))
                    }
                })
                .await
        }
    });
    waiting.await.unwrap();
    known.reset().await;
    release.send(()).unwrap();
    assert_eq!(in_flight.await.unwrap().unwrap(), known_sign_in("New"));
}

#[tokio::test]
async fn a_late_sign_in_code_belongs_only_to_the_account_that_started_it() {
    use kuvo_agents::{
        runtime::AdapterCall,
        sign_in::{Attempt, Mode, Page, SignIns, Status},
    };
    use std::sync::{Arc, Mutex};
    use tokio::time::Instant;

    struct Fixture(Mutex<Status>);

    impl Attempt for Fixture {
        fn status(&self) -> Status {
            self.0.lock().unwrap().clone()
        }

        fn submit_code(&self, code: String) -> AdapterCall<'_, ()> {
            Box::pin(async move {
                assert_eq!(code, "fixture-code");
                *self.0.lock().unwrap() = Status::Complete;
                Ok(())
            })
        }

        fn cancel(&self) {
            *self.0.lock().unwrap() = Status::Expired;
        }
    }

    let provider = Provider::parse("codex-cli@work").unwrap();
    let other = Provider::parse("codex-cli").unwrap();
    let now = Instant::now();
    let mut flow = SignIns::default();
    let page = Page {
        url: String::new(),
        instructions: String::new(),
        mode: Mode::Auto,
        accepts_code: false,
        user_code: None,
    };
    let attempt = Arc::new(Fixture(Mutex::new(Status::Pending { page: page.clone() })));
    let id = flow.track(provider.clone(), attempt.clone(), now).unwrap();
    assert!(flow.waiting(now));
    assert!(flow.for_code(&provider, &id, now).is_err());
    let late = Status::Pending {
        page: Page {
            url: "https://example.invalid/test".into(),
            mode: Mode::Code,
            ..page
        },
    };
    *attempt.0.lock().unwrap() = late.clone();
    assert_eq!(flow.status(&provider, &id, now), Some(late));
    assert!(flow.status(&other, &id, now).is_none());
    assert!(!flow.cancel(&other, &id));
    flow.for_code(&provider, &id, now)
        .unwrap()
        .submit_code("fixture-code".into())
        .await
        .unwrap();
    assert_eq!(flow.status(&provider, &id, now), Some(Status::Complete));
    assert!(flow.status(&provider, &id, now).is_none());
    assert!(!flow.waiting(now));
}

#[tokio::test]
async fn a_failed_model_refresh_keeps_only_that_accounts_last_catalog_until_sign_in_changes() {
    use futures_util::future::BoxFuture;
    use kuvo_agents::{
        account_check::{AccountStatus, CheckFailure},
        catalog::{Model, Models},
        hold::{Controller, Record, TurnNote},
        roster::{Agent, Factory, Roster},
        runtime::{AdapterCall, Adoption},
    };
    use std::{
        collections::{BTreeMap, HashMap},
        io,
        path::PathBuf,
        sync::Arc,
    };

    struct FactoryFixture;
    struct Fixture {
        added: bool,
    }

    impl Factory for FactoryFixture {
        fn create(
            &self,
            account: &kuvo_agents::accounts::Account,
            home: Option<PathBuf>,
        ) -> io::Result<Arc<dyn Agent>> {
            assert_eq!(home.is_some(), account.id != "default");
            Ok(Arc::new(Fixture {
                added: home.is_some(),
            }))
        }
    }

    impl Agent for Fixture {
        fn probe_account(&self) -> BoxFuture<'_, Result<AccountStatus, CheckFailure>> {
            Box::pin(async { Ok(known_sign_in("Fixture")) })
        }

        fn models(&self, refresh: bool) -> AdapterCall<'_, Models> {
            Box::pin(async move {
                if self.added && refresh {
                    return Err(io::Error::other("fixture catalog offline"));
                }
                let id = if self.added {
                    "work-model"
                } else {
                    "own-model"
                };
                Ok(Models {
                    complete: true,
                    generation: None,
                    models: vec![Model {
                        id: id.into(),
                        name: id.into(),
                        aliases: vec!["preferred".into()],
                        efforts: Vec::new(),
                        speeds: Vec::new(),
                        hidden: false,
                        retiring: false,
                        provisional: false,
                    }],
                })
            })
        }

        fn shutdown(&self) -> AdapterCall<'_, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    impl Adoption for Fixture {
        fn accepts(&self, _: &Record, _: &BTreeMap<String, TurnNote>) -> bool {
            false
        }

        fn attach<'a>(
            &'a self,
            _: &'a kuvo_agents::accounts::Account,
            _: Record,
            _: BTreeMap<String, TurnNote>,
            _: Controller,
        ) -> AdapterCall<'a, ()> {
            Box::pin(async { unreachable!("this fixture has no held process") })
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let home = Home::new(temp.path().to_owned()).unwrap();
    let accounts = Accounts::for_home(&home);
    let added = accounts
        .add(AgentKind::Codex, Some("Work".into()))
        .await
        .unwrap();
    let factories = HashMap::from([(
        AgentKind::Codex,
        Arc::new(FactoryFixture) as Arc<dyn Factory>,
    )]);
    let mut roster = Roster::default();
    roster.reconcile(&accounts, &factories).await.unwrap();
    let before = roster.catalog(false).await;
    assert_eq!(before[0].models.as_ref().unwrap().models[0].id, "own-model");
    assert_eq!(
        before[1].models.as_ref().unwrap().models[0].id,
        "work-model"
    );

    accounts
        .rename(added.provider(), Some("Office".into()))
        .await
        .unwrap();
    roster.reconcile(&accounts, &factories).await.unwrap();
    let failed = roster.catalog(true).await;
    assert_eq!(failed[1].account.label.as_deref(), Some("Office"));
    assert!(!failed[0].unavailable);
    assert!(failed[1].unavailable);
    let work = failed[1].models.as_ref().unwrap();
    assert!(!work.complete);
    assert_eq!(work.models[0].id, "work-model");
    assert_eq!(work.models[0].aliases, ["preferred"]);

    roster
        .find(&added.provider())
        .unwrap()
        .credentials_changed()
        .await;
    let changed = roster.catalog(true).await;
    assert!(changed[1].models.is_none());
    assert_eq!(
        changed[0].models.as_ref().unwrap().models[0].id,
        "own-model"
    );
}
