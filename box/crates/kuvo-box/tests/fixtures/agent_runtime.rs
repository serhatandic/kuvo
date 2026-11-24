//! Only linked into the dedicated test build. Quoted agent lines are a stand-in;
//! the resident, roster, holder, sockets, files and keeper are the shipped implementations.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use kuvo_agents::{
    account_check::{AccountStatus, CheckFailure},
    accounts::Account,
    catalog::Models,
    hold::{Controller, HoldKind, Limits, Output, Record, Start, TurnNote},
    roster::{Agent, Factory},
    runtime::{AdapterCall, Adoption, Runtime},
};
use kuvo_store::{files, home::Home};
use serde_json::json;

const ASK: &str = r#"{"id":17,"method":"fixture/approve"}"#;
const REPLY: &str = r#"{"id":17,"result":{"allow":true}}"#;

pub(super) fn for_home(root: &Path) -> Option<Arc<dyn Factory>> {
    let path = root.join("run/agent-fixture-program");
    let program = PathBuf::from(fs::read_to_string(path).ok()?);
    if !program.is_absolute() || !program.is_file() {
        return None;
    }
    let home = Home::new(root.to_owned()).ok()?;
    Some(Arc::new(Fixture { home, program }))
}

#[derive(Clone)]
struct Fixture {
    home: Home,
    program: PathBuf,
}

impl Factory for Fixture {
    fn create(&self, account: &Account, _: Option<PathBuf>) -> io::Result<Arc<dyn Agent>> {
        if account.id == "default" {
            let fixture = self.clone();
            tokio::spawn(async move {
                if let Err(error) = fixture.wait_and_start().await {
                    eprintln!("Agent fixture failed: {error}");
                }
            });
        }
        Ok(Arc::new(self.clone()))
    }
}

impl Agent for Fixture {
    fn probe_account(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<AccountStatus, CheckFailure>> + Send + '_>,
    > {
        Box::pin(async { Err(CheckFailure::TimedOut) })
    }

    fn models(&self, _: bool) -> AdapterCall<'_, Models> {
        Box::pin(async { Ok(Models::default()) })
    }

    fn shutdown(&self) -> AdapterCall<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

impl Adoption for Fixture {
    fn accepts(&self, record: &Record, turns: &BTreeMap<String, TurnNote>) -> bool {
        record.meta.get("fixture") == Some(&json!(true))
            && turns
                .values()
                .any(|turn| turn.session == "ses_keeper_fixture")
    }

    fn attach<'a>(
        &'a self,
        _: &'a Account,
        _: Record,
        _: BTreeMap<String, TurnNote>,
        mut controller: Controller,
    ) -> AdapterCall<'a, ()> {
        let home = self.home.clone();
        Box::pin(async move {
            tokio::spawn(async move {
                let result: io::Result<()> = async {
                    line(&mut controller, "before").await?;
                    line(&mut controller, ASK).await?;
                    controller.input.unpin().await?;
                    controller.input.write(REPLY).await?;
                    line(&mut controller, "after").await?;
                    write_checkpoint(&home, "agent-fixture-finished", &controller)?;
                    controller.input.end().await?;
                    while !matches!(controller.output.next().await?, Output::Exited { .. }) {}
                    controller.input.bye().await
                }
                .await;
                if let Err(error) = result {
                    eprintln!("Agent fixture replay failed: {error}");
                }
            });
            Ok(())
        })
    }
}

impl Fixture {
    async fn wait_and_start(&self) -> io::Result<()> {
        let arm = self.home.run().join("agent-fixture-begin");
        let mut clock = tokio::time::interval(Duration::from_millis(25));
        loop {
            clock.tick().await;
            match fs::remove_file(&arm) {
                Ok(()) => break,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        let binary = std::env::current_exe()?;
        let runtime = Runtime::for_home(&self.home);
        let mut controller = runtime
            .holders()
            .start(
                &binary,
                Start {
                    command: vec![
                        self.program.clone(),
                        "--exact".into(),
                        "agent_probe".into(),
                        "--ignored".into(),
                        "--nocapture".into(),
                    ],
                    cwd: Some(self.home.root().to_owned()),
                    env: std::env::vars_os().collect(),
                    kind: HoldKind::JsonRpc,
                    meta: json!({
                        "provider":"codex-cli", "fixture":true,
                        "turns":{"thread":{"session":"ses_keeper_fixture","from":0}},
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                    portable_requests: None,
                    limits: Limits {
                        orphan: Some(Duration::from_secs(90)),
                        ..Limits::default()
                    },
                },
            )
            .await?;
        controller.input.write("begin").await?;
        let ask = line(&mut controller, ASK).await?;
        controller.input.acknowledge(ask).await?;
        controller.input.prepare(0).await?;
        if !matches!(
            controller.output.next().await?,
            Output::Prepared { prepared: true }
        ) {
            return Err(io::Error::other("holder did not reserve its replay"));
        }
        write_checkpoint(&self.home, "agent-fixture-started", &controller)?;
        while controller.output.next().await.is_ok() {}
        Ok(())
    }
}

async fn line(controller: &mut Controller, expected: &str) -> io::Result<u64> {
    loop {
        match controller.output.next().await? {
            Output::Line { sequence, line, .. } if line == expected => return Ok(sequence),
            Output::Exited { .. } => {
                return Err(io::Error::other("agent exited before its answer"));
            }
            _ => {}
        }
    }
}

fn write_checkpoint(home: &Home, name: &str, controller: &Controller) -> io::Result<()> {
    let result = json!({
        "agent": controller.welcome.child,
        "holder": controller.welcome.pid,
        "generation": controller.welcome.generation,
        "resident": std::process::id(),
    });
    files::write_atomic(&home.run().join(name), &serde_json::to_vec(&result)?)
}
