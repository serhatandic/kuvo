//! Claude Code's command line and stream-json protocol.

mod model;
mod program;
mod wire;

use std::{collections::BTreeMap, io, path::PathBuf, sync::Arc, time::Duration};

use futures_util::future::BoxFuture;
use kuvo_store::home::Home;

use crate::{
    account_check::{AccountStatus, CheckFailure},
    accounts::Account,
    catalog::Models,
    hold::{Controller, Record, TurnNote},
    roster::{Agent, Factory},
    runtime::{AdapterCall, Adoption},
    sign_in::{Attempt, Request},
};

#[derive(Clone)]
pub struct ClaudeFactory {
    home: Home,
}

impl ClaudeFactory {
    pub fn new(home: Home) -> Self {
        Self { home }
    }
}

/// Older Macs ran Claude Console in Kuvo's own folder. Its native sign-in belongs to
/// that exact path, so register the folder without copying its contents.
pub async fn adopt_console_folder(home: &Home) -> io::Result<()> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    const FOLDER: &str = "claude/console";
    let agents = home.root().join("agents");
    let directory = agents.join(FOLDER);
    let used = tokio::task::spawn_blocking(move || {
        if directory.ancestors().take(3).any(|folder| {
            !std::fs::symlink_metadata(folder).is_ok_and(|metadata| metadata.is_dir())
        }) {
            return false;
        }
        if directory.join(".claude.json").exists() || directory.join("projects").exists() {
            return true;
        }
        // Preserve any unrecognized files left by the old CLI.
        let _ = std::fs::remove_dir(directory);
        false
    })
    .await?;
    if used {
        crate::accounts::Accounts::for_home(home)
            .adopt_home(
                crate::accounts::AgentKind::Claude,
                FOLDER.into(),
                Some("Anthropic Console".into()),
            )
            .await?;
    }
    Ok(())
}

impl Factory for ClaudeFactory {
    fn create(&self, account: &Account, home: Option<PathBuf>) -> io::Result<Arc<dyn Agent>> {
        Ok(Arc::new(Claude {
            program: program::Program::new(self.home.clone(), account.id.clone(), home),
            models: model::Catalog::new()?,
        }))
    }
}

struct Claude {
    program: program::Program,
    models: model::Catalog,
}

impl Agent for Claude {
    fn probe_account(&self) -> BoxFuture<'_, Result<AccountStatus, CheckFailure>> {
        Box::pin(async {
            let (status, replaced) =
                self.program
                    .probe()
                    .await
                    .map_err(|error| match error.kind() {
                        io::ErrorKind::TimedOut => CheckFailure::TimedOut,
                        _ => CheckFailure::Failed(error.to_string()),
                    })?;
            if replaced {
                self.models.clear().await;
            }
            Ok(status)
        })
    }

    fn models(&self, refresh: bool) -> AdapterCall<'_, Models> {
        Box::pin(self.models.read(self.program.clone(), refresh))
    }

    fn keep_account(&self) -> Duration {
        Duration::from_secs(30)
    }

    fn remote_sign_in(&self) -> bool {
        true
    }

    fn sign_in(&self, request: Request) -> AdapterCall<'_, Arc<dyn Attempt>> {
        Box::pin(self.program.login(request, self.models.clone()))
    }

    fn sign_out(&self) -> AdapterCall<'_, ()> {
        Box::pin(async {
            self.program.logout().await?;
            self.models.clear().await;
            Ok(())
        })
    }

    fn remove_credentials(&self) -> AdapterCall<'_, ()> {
        Box::pin(async {
            self.program.logout().await?;
            self.models.clear().await;
            Ok(())
        })
    }

    fn shutdown(&self) -> AdapterCall<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

impl Adoption for Claude {
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
        Box::pin(async { Err(io::Error::from(io::ErrorKind::Unsupported)) })
    }
}
