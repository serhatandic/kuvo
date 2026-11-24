//! Apply a fresh view of Blocks to a submitted agent ask.

use super::{ConfigLocation, Decision as Verdict, Effect, Policy, check, read_blocks};
use crate::{
    runtime::AdapterCall,
    session::{Decision, PermissionAsk, Preflight, Thread},
    turn_sink::TurnPolicy,
};
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

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
}

impl TurnRules {
    async fn resolve(
        &self,
        ask: PermissionAsk,
        native_plan: bool,
        interactive: bool,
    ) -> io::Result<Option<Verdict>> {
        let mut policy = self.current.clone();
        let directory = policy.directory.clone();
        let layers = tokio::task::spawn_blocking(move || project_layers(&directory)).await?;
        policy.blocks = read_blocks(&self.settings.home, self.settings.location, layers)
            .await
            .ok();
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
