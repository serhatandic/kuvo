//! A deleted conversation's files go only after both runtimes can account for them.

use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    io,
    path::{Path, PathBuf},
    pin::Pin,
};

use serde_json::Value;

use crate::{
    Engine, api,
    attachments::session_folder,
    lifetime::{Demand, Lease},
    sessions,
};

pub type AttachmentRead<'a, T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send + 'a>>;

pub struct AttachmentConversation {
    pub id: String,
    pub ancestors: Vec<String>,
}

/// The coding-agent runtime supplies its own complete roster and transcripts.
/// Move protection is read at deletion as well as during inventory.
pub trait AttachmentSource: Send + Sync {
    fn conversations(&self) -> AttachmentRead<'_, Vec<AttachmentConversation>>;
    fn transcript<'a>(&'a self, session: &'a str) -> AttachmentRead<'a, Vec<Value>>;
    fn protected(&self, session: &str) -> bool;
}

#[derive(Default)]
struct Owners {
    engine: bool,
    agents: bool,
    ancestors: Vec<String>,
}

fn upstream(error: Box<dyn std::error::Error + Send + Sync>) -> io::Error {
    io::Error::other(error)
}

pub(crate) async fn sweep(
    engine: &Engine,
    source: &dyn AttachmentSource,
    deleted: Option<&str>,
) -> io::Result<()> {
    let lease = engine
        .acquire(Demand::Existing)
        .await
        .ok_or_else(|| io::Error::other("The engine is unavailable for attachment retention."))?;
    let attachments = engine.home.join("attachments");
    let mut candidates = BTreeSet::new();
    if let Some(session) = deleted {
        if session_folder(session) && !source.protected(session) {
            candidates.insert(session.to_owned());
        }
    } else if let Ok(mut entries) = tokio::fs::read_dir(&attachments).await {
        while let Some(entry) = entries.next_entry().await? {
            if let Ok(session) = entry.file_name().into_string()
                && session_folder(&session)
                && !source.protected(&session)
            {
                candidates.insert(session);
            }
        }
    }
    if candidates.is_empty() {
        return Ok(());
    }
    let inventory = inventory(engine, &lease, source).await?;
    candidates.retain(|session| !inventory.contains_key(session));
    if candidates.is_empty() {
        return Ok(());
    }
    let referenced = shown(engine, &lease, source, &inventory, &candidates).await?;
    for session in candidates.difference(&referenced) {
        if !source.protected(session) {
            remove(&attachments, session).await?;
            if deleted.is_some()
                && let Some(root) = &engine.legacy_attachments
            {
                remove(root, session).await?;
            }
        }
    }
    Ok(())
}

async fn inventory(
    engine: &Engine,
    lease: &Lease,
    source: &dyn AttachmentSource,
) -> io::Result<BTreeMap<String, Owners>> {
    let (local, agents) = tokio::join!(
        api::Scan::Sessions.read(engine, lease, "api/session"),
        source.conversations()
    );
    let mut sessions = BTreeMap::<String, Owners>::new();
    for value in local.map_err(upstream)? {
        let session: sessions::Session = serde_json::from_value(value)?;
        let row = sessions.entry(session.id.clone()).or_default();
        row.engine = true;
        row.ancestors.extend(session.parent.iter().cloned());
        if let Some(notes) = session.notes() {
            row.ancestors.extend(
                [
                    "branchedFrom",
                    "handoffFrom",
                    "carriedFrom",
                    "startedBy",
                    "beside",
                ]
                .into_iter()
                .filter_map(|key| notes.get(key)?.as_str().map(str::to_owned)),
            );
        }
    }
    for agent in agents? {
        let row = sessions.entry(agent.id).or_default();
        row.agents = true;
        row.ancestors.extend(agent.ancestors);
    }
    Ok(sessions)
}

fn leads_back(id: &str, inventory: &BTreeMap<String, Owners>) -> bool {
    let mut seen = BTreeSet::from([id]);
    let mut rest = inventory[id]
        .ancestors
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    while let Some(ancestor) = rest.pop() {
        if !seen.insert(ancestor) {
            continue;
        }
        let Some(above) = inventory.get(ancestor) else {
            return true;
        };
        rest.extend(above.ancestors.iter().map(String::as_str));
    }
    false
}

async fn shown(
    engine: &Engine,
    lease: &Lease,
    source: &dyn AttachmentSource,
    inventory: &BTreeMap<String, Owners>,
    candidates: &BTreeSet<String>,
) -> io::Result<BTreeSet<String>> {
    let mut retain = BTreeSet::new();
    for (id, owners) in inventory.iter().filter(|(id, _)| leads_back(id, inventory)) {
        let mut messages = Vec::new();
        if owners.engine {
            messages.extend(
                api::Scan::Messages
                    .read(engine, lease, &format!("{}/message", sessions::path(id)))
                    .await
                    .map_err(upstream)?,
            );
        }
        if owners.agents {
            messages.extend(source.transcript(id).await?);
        }
        let mut kept = serde_json::to_string(&messages)?.replace(r"\\", "/");
        kept.make_ascii_lowercase();
        for file in candidates {
            if kept.contains(&format!("attachments/{}/", file.to_ascii_lowercase())) {
                retain.insert(file.clone());
            }
        }
    }
    Ok(retain)
}

async fn remove(root: &Path, session: &str) -> io::Result<()> {
    match tokio::fs::remove_dir_all(root.join(session)).await {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

pub(crate) fn watch(engine: Engine) -> Option<tokio::sync::mpsc::UnboundedSender<Option<String>>> {
    let source = engine.attachment_source.clone()?;
    let (look, mut pending) = tokio::sync::mpsc::unbounded_channel::<Option<String>>();
    tokio::spawn(async move {
        while let Some(deleted) = pending.recv().await {
            let _turn = engine.turn("attachment-retention").await;
            let _ = sweep(&engine, source.as_ref(), deleted.as_deref()).await;
        }
    });
    Some(look)
}

impl Engine {
    /// A missing coding-agent source leaves deleted files in place until both runtimes can speak.
    pub fn with_attachment_source(
        mut self,
        source: std::sync::Arc<dyn AttachmentSource>,
        legacy_attachments: Option<PathBuf>,
    ) -> Self {
        self.attachment_source = Some(source);
        self.legacy_attachments = legacy_attachments;
        self
    }

    /// Reconcile retained files when a runtime's inventory becomes available.
    pub async fn reconcile_attachments(&self) -> io::Result<()> {
        if let Some(source) = &self.attachment_source {
            let _turn = self.turn("attachment-retention").await;
            sweep(self, source.as_ref(), None).await?;
        }
        Ok(())
    }
}
