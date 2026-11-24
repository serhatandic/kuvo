//! The real router uses an adapter stand-in that only answers account and model queries.

use super::*;
use futures_util::future::BoxFuture;
use http::{HeaderMap, Method, StatusCode};
use http_body_util::BodyExt;
use kuvo_agents::{
    account_check::{AccountStatus, CheckFailure},
    accounts::Account,
    catalog::{Model, Models as VendorModels},
    hold::{Controller, Record, TurnNote},
    roster::Agent,
    runtime::{AdapterCall, Adoption},
    sign_in::{Attempt, Mode, Page, Status},
};
use kuvo_server::{Caller, DispatchRequest, Dispatcher, LineTiming, LocalOrigin, Transport};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};
use tokio_util::sync::CancellationToken;

struct FixtureFactory;
struct Fixture;

impl Factory for FixtureFactory {
    fn create(&self, _: &Account, _: Option<PathBuf>) -> io::Result<Arc<dyn Agent>> {
        Ok(Arc::new(Fixture))
    }
}

impl Adoption for Fixture {
    fn accepts(&self, _: &Record, _: &BTreeMap<String, TurnNote>) -> bool {
        false
    }

    fn attach<'a>(
        &'a self,
        _: &'a Account,
        _: Record,
        _: BTreeMap<String, TurnNote>,
        _: Controller,
    ) -> AdapterCall<'a, ()> {
        Box::pin(async { unreachable!("this account fixture does not start a process") })
    }
}

impl Agent for Fixture {
    fn probe_account(&self) -> BoxFuture<'_, Result<AccountStatus, CheckFailure>> {
        Box::pin(async {
            Ok(AccountStatus {
                installed: true,
                signed_in: true,
                label: Some("Fixture".into()),
                identity: None,
                label_describes_method: false,
                message: None,
                ended: false,
                unchecked: false,
            })
        })
    }

    fn models(&self, _: bool) -> AdapterCall<'_, VendorModels> {
        Box::pin(async {
            Ok(VendorModels {
                generation: Some("fixture".into()),
                complete: true,
                models: vec![Model {
                    id: "model".into(),
                    name: "Model".into(),
                    aliases: vec!["latest".into()],
                    efforts: vec!["low".into()],
                    speeds: Vec::new(),
                    hidden: false,
                    retiring: false,
                    provisional: false,
                }],
            })
        })
    }

    fn remote_sign_in(&self) -> bool {
        true
    }

    fn sign_in(&self, request: Request) -> AdapterCall<'_, Arc<dyn Attempt>> {
        Box::pin(async move {
            assert_eq!(request.method, "oauth");
            assert!(request.remote && request.browser_return);
            Ok(Arc::new(FixtureAttempt(AtomicBool::new(false))) as Arc<dyn Attempt>)
        })
    }

    fn shutdown(&self) -> AdapterCall<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

struct FixtureAttempt(AtomicBool);

impl Attempt for FixtureAttempt {
    fn status(&self) -> Status {
        if self.0.load(Ordering::Acquire) {
            return Status::Complete;
        }
        Status::Pending {
            page: Page {
                url: "https://example.invalid/sign-in".into(),
                instructions: "Enter the code.".into(),
                mode: Mode::Code,
                accepts_code: true,
                user_code: None,
            },
        }
    }

    fn submit_code(&self, code: String) -> AdapterCall<'_, ()> {
        Box::pin(async move {
            assert_eq!(code, "fixture");
            self.0.store(true, Ordering::Release);
            Ok(())
        })
    }

    fn cancel(&self) {}
}

async fn request(
    dispatcher: &Dispatcher,
    method: Method,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let answer = dispatcher
        .dispatch(
            DispatchRequest {
                method,
                path: path.into(),
                headers: HeaderMap::new(),
                body: body.to_string().into(),
                caller: Caller::Local(Transport::Http {
                    origin: LocalOrigin::App,
                    internal: false,
                }),
            },
            CancellationToken::new(),
        )
        .await;
    let bytes = answer.body.collect().await.unwrap().to_bytes();
    (answer.status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn agent_account_routes_keep_the_models_and_signup_on_their_own_account() {
    const ACCOUNTS: &str = "kuvo/agents/api/kuvo/accounts";
    let scratch = tempfile::tempdir().unwrap();
    let home = kuvo_store::home::Home::new(scratch.path().to_owned()).unwrap();
    let runtime = Runtime::for_home(&home);
    let (agents, failures) = ResidentAgents::start(
        runtime.clone(),
        HashMap::from([(
            AgentKind::Codex,
            Arc::new(FixtureFactory) as Arc<dyn Factory>,
        )]),
    )
    .await
    .unwrap();
    assert!(failures.is_empty());
    let mut routes = Registry::new();
    register(agents.clone(), &mut routes);
    let dispatcher = routes.build(LineTiming::default());
    let source = Models { runtime, agents };

    let (status, added) = request(
        &dispatcher,
        Method::POST,
        ACCOUNTS,
        json!({"provider":"codex-cli","label":"Work"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let provider = added["data"]["providerID"].as_str().unwrap();
    assert!(provider.starts_with("codex-cli@"));
    let encoded = provider.replace('@', "%40");
    let path = format!("{ACCOUNTS}/{encoded}");
    assert_eq!(
        request(
            &dispatcher,
            Method::PATCH,
            &path,
            json!({"label":" Office "})
        )
        .await,
        (StatusCode::OK, json!({})),
    );

    let signed_in = source.catalog(false).await.unwrap();
    assert_eq!(signed_in.accounts.len(), 2);
    assert_eq!(signed_in.accounts[1].label.as_deref(), Some("Office"));
    assert_eq!(signed_in.models[0].provider, "codex-cli");
    assert_eq!(signed_in.models[1].provider, provider);
    assert_eq!(signed_in.models[1].aliases, ["latest"]);
    assert_eq!(signed_in.generations[provider], "fixture");

    let unsigned_only = format!("{path}?onlyUnsigned=1");
    assert_eq!(
        request(&dispatcher, Method::DELETE, &unsigned_only, json!(null))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(source.catalog(false).await.unwrap().accounts.len(), 2);
    assert_eq!(
        request(&dispatcher, Method::DELETE, &path, json!(null))
            .await
            .0,
        StatusCode::OK
    );
    let remaining = source.catalog(false).await.unwrap();
    assert_eq!(remaining.accounts.len(), 1);
    assert_eq!(remaining.models[0].provider, "codex-cli");
    assert_eq!(
        request(
            &dispatcher,
            Method::DELETE,
            &format!("{ACCOUNTS}/codex-cli"),
            json!(null)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn agent_account_routes_keep_an_oauth_attempt_on_its_provider() {
    const OAUTH: &str = "kuvo/agents/api/integration/codex-cli/connect/oauth";
    let scratch = tempfile::tempdir().unwrap();
    let home = kuvo_store::home::Home::new(scratch.path().to_owned()).unwrap();
    let runtime = Runtime::for_home(&home);
    let (agents, failures) = ResidentAgents::start(
        runtime,
        HashMap::from([(
            AgentKind::Codex,
            Arc::new(FixtureFactory) as Arc<dyn Factory>,
        )]),
    )
    .await
    .unwrap();
    assert!(failures.is_empty());
    let mut routes = Registry::new();
    register(agents, &mut routes);
    let dispatcher = routes.build(LineTiming::default());

    let (code, started) = request(
        &dispatcher,
        Method::POST,
        OAUTH,
        json!({
            "methodID":"oauth","remote":true,"browserReturn":true,
        }),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{started}");
    let id = started["data"]["attemptID"].as_str().unwrap();
    assert_eq!(started["data"]["mode"], "code");
    assert_eq!(
        started["data"]["time"]["expires"].as_u64().unwrap()
            - started["data"]["time"]["created"].as_u64().unwrap(),
        30 * 60_000
    );
    let status = format!("{OAUTH}/{id}");
    assert_eq!(
        request(&dispatcher, Method::GET, &status, json!(null))
            .await
            .1["data"]["status"],
        "pending"
    );
    assert_eq!(
        request(
            &dispatcher,
            Method::POST,
            &format!("{status}/complete"),
            json!({"code":"fixture"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&dispatcher, Method::GET, &status, json!(null))
            .await
            .1["data"]["status"],
        "complete"
    );
    assert_eq!(
        request(&dispatcher, Method::GET, &status, json!(null))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}
