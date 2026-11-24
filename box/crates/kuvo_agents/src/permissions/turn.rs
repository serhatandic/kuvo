//! Apply a fresh view of Blocks to a submitted agent ask.

use super::{
    Allowance, ConfigLocation, Decision as Verdict, Effect, Policy, Rule, check, read_blocks,
};
use crate::{
    runtime::AdapterCall,
    session::{Decision, PermissionAsk, Preflight, Thread},
    session_store::SessionStore,
    turn_sink::TurnPolicy,
};
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::watch;

#[derive(Clone)]
pub(crate) struct Settings {
    pub home: PathBuf,
    pub location: ConfigLocation,
}

pub(crate) struct TurnRules {
    pub settings: Settings,
    pub current: Policy,
    pub plan: bool,
    pub thread: Arc<dyn Thread>,
    pub remembered: watch::Sender<Vec<Allowance>>,
    pub store: SessionStore,
    pub session: String,
}

impl TurnRules {
    pub(crate) async fn verify_reply(&self, ask: &PermissionAsk) -> io::Result<()> {
        let header = self
            .store
            .read(&self.session)
            .await?
            .ok_or(io::ErrorKind::NotFound)?
            .into_fields();
        let policy = self.load(true).await?;
        let mut policy = policy;
        policy.session_rules = header
            .get("permissions")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|rule| serde_json::from_value::<Rule>(rule.clone()).ok())
            .collect();
        let verdict = self.evaluate(policy, ask.clone(), false, true).await?;
        if verdict == Some(Verdict::Deny) {
            Err(io::ErrorKind::PermissionDenied.into())
        } else {
            Ok(())
        }
    }

    async fn load(&self, strict: bool) -> io::Result<Policy> {
        let mut policy = self.current.clone();
        policy
            .allowed
            .extend(self.remembered.borrow().iter().cloned());
        let directory = policy.directory.clone();
        let layers = tokio::task::spawn_blocking(move || project_layers(&directory)).await?;
        let blocks = read_blocks(&self.settings.home, self.settings.location, layers).await;
        policy.blocks = if strict {
            Some(blocks.map_err(|error| {
                io::Error::other(format!(
                    "Kuvo couldn’t verify this computer’s current permissions. {error}",
                ))
            })?)
        } else {
            blocks.ok()
        };
        Ok(policy)
    }

    async fn resolve(
        &self,
        ask: PermissionAsk,
        native_plan: bool,
        interactive: bool,
    ) -> io::Result<Option<Verdict>> {
        let policy = self.load(false).await?;
        self.evaluate(policy, ask, native_plan, interactive).await
    }

    async fn evaluate(
        &self,
        mut policy: Policy,
        ask: PermissionAsk,
        native_plan: bool,
        interactive: bool,
    ) -> io::Result<Option<Verdict>> {
        let plan = self.plan && !native_plan;
        if ask.sandboxed == Some(true) {
            policy.auto_without_sandbox = false;
        }
        let thread = self.thread.clone();
        tokio::task::spawn_blocking(move || {
            use super::Decision::Ask;
            let weighed = interactive && ask.already_weighed;
            if weighed {
                // Upstream already asked about file boundaries. Keep Blocks
                // and explicit session rules in force for the action itself.
                policy
                    .agent_rules
                    .retain(|rule| rule.effect == Effect::Deny);
            }
            policy.observe_paths(&ask);
            let verdict = check(&policy, &ask, plan, |path| {
                weighed || thread.owns_path(path)
            });
            if interactive
                && verdict.is_none()
                && ask.unclassified
                && policy.session_rules.iter().any(|rule| {
                    rule.effect == Effect::Ask
                        && matches!(
                            rule.action.as_str(),
                            "edit" | "shell" | "write" | "patch" | "*"
                        )
                })
            {
                return Some(Ask);
            }
            verdict
        })
        .await
        .map_err(Into::into)
    }
}

impl TurnPolicy for TurnRules {
    fn card(&self, ask: PermissionAsk) -> AdapterCall<'_, PermissionAsk> {
        Box::pin(async move {
            let mut policy = self.load(false).await?;
            let thread = self.thread.clone();
            tokio::task::spawn_blocking(move || {
                policy.observe_paths(&ask);
                super::outside_card(&policy, &ask, |path| thread.owns_path(path)).unwrap_or(ask)
            })
            .await
            .map_err(Into::into)
        })
    }

    fn preflight(&self, ask: PermissionAsk, native_plan: bool) -> AdapterCall<'_, Preflight> {
        Box::pin(async move {
            Ok(match self.resolve(ask, native_plan, false).await? {
                Some(Verdict::Deny) => Preflight::Deny,
                Some(Verdict::Ask) => Preflight::Ask,
                None => Preflight::NoOverride,
            })
        })
    }

    fn permission(&self, ask: &PermissionAsk) -> AdapterCall<'_, Option<Decision>> {
        let ask = ask.clone();
        Box::pin(async move {
            let weighed = ask.already_weighed;
            Ok(match self.resolve(ask, false, true).await? {
                Some(Verdict::Deny) => Some(Decision::Reject),
                Some(Verdict::Ask) => None,
                None if weighed => None,
                None => Some(Decision::Once),
            })
        })
    }
}

fn project_layers(folder: &Path) -> Vec<PathBuf> {
    let mut ancestry = Vec::new();
    for directory in folder.ancestors() {
        ancestry.push(directory);
        if directory.join(".git").exists() {
            break;
        }
    }
    // Without a repository marker, this session's own folder is its project.
    if ancestry
        .last()
        .is_none_or(|directory| !directory.join(".git").exists())
    {
        ancestry.truncate(1);
    }
    ancestry
        .into_iter()
        .rev()
        .flat_map(|root| {
            [
                root.join("opencode.json"),
                root.join("opencode.jsonc"),
                root.join(".opencode/opencode.json"),
                root.join(".opencode/opencode.jsonc"),
            ]
        })
        .collect()
}
