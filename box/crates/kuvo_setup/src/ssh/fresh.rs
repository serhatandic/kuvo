//! Private recovery metadata for a fresh SSH installation. It holds only hashes.

use std::{fmt, path::Path};

use serde_json::json;

use super::Selection;

pub(crate) const FILE: &str = "run/.ssh-fresh-add";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Prepared,
    Copying,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Fresh {
    scope: [u8; 32],
    selection: [u8; 32],
    pub(super) phase: Phase,
}

impl Fresh {
    pub(super) fn new(setup: &str, primary: &str, selection: &Selection) -> Self {
        Self {
            scope: kuvo_wire::json::digest(&json!([setup, primary])),
            selection: selection.ssh_hash(),
            phase: Phase::Prepared,
        }
    }

    pub(super) fn read(line: &str) -> Option<Self> {
        let mut fields = line.trim_end_matches('\n').split(' ');
        if fields.next()? != "v1" {
            return None;
        }
        let hash = |field: &str| {
            if field.len() != 64
                || !field
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return None;
            }
            hex::decode(field).ok()?.try_into().ok()
        };
        let scope = hash(fields.next()?)?;
        let selection = hash(fields.next()?)?;
        let phase = match fields.next()? {
            "prepared" => Phase::Prepared,
            "copying" => Phase::Copying,
            _ => return None,
        };
        fields.next().is_none().then_some(Self {
            scope,
            selection,
            phase,
        })
    }

    pub(super) fn saved(home: &Path) -> Option<Self> {
        let file = home.join(FILE);
        let metadata = file.symlink_metadata().ok()?;
        if !metadata.is_file() || metadata.len() > 142 {
            return None;
        }
        Self::read(&std::fs::read_to_string(file).ok()?)
    }

    pub(super) fn same_scope(&self, other: &Self) -> bool {
        self.scope == other.scope
    }

    pub(super) fn same_selection(&self, other: &Self) -> bool {
        self.selection == other.selection
    }
}

pub(crate) fn finish(home: &Path, link: &crate::model::Link, own: &str) -> std::io::Result<()> {
    let Some(fresh) = Fresh::saved(home) else {
        return Ok(());
    };
    let Ok(setup) = link.setup() else {
        return Ok(());
    };
    if setup.role != crate::model::Role::Member
        || !link
            .member_list()
            .is_ok_and(|roster| roster.boxes.iter().any(|peer| peer.key == own))
    {
        return Ok(());
    }
    let accepted = Fresh::new(
        &setup.position.id,
        &setup.position.primary,
        &Selection::default(),
    );
    if !fresh.same_scope(&accepted) {
        return Ok(());
    }
    let path = home.join(FILE);
    match std::fs::remove_file(&path) {
        Ok(()) => kuvo_store::files::sync_parent(&path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

impl fmt::Display for Fresh {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        let phase = match self.phase {
            Phase::Prepared => "prepared",
            Phase::Copying => "copying",
        };
        write!(
            out,
            "v1 {} {} {phase}",
            hex::encode(self.scope),
            hex::encode(self.selection)
        )
    }
}
