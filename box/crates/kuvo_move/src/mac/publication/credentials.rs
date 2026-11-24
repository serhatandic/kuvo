//! Move credentials named by a Claude Code configuration directory.

use super::{CutoverStep, Plan, invalid};
use crate::{Disk, Error, entries, metadata};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    io,
    path::{Component, Path, PathBuf},
};
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroize as _;

/// The generic-password operations supplied by Helper. A missing item is
/// `Ok(None)`. Implementations must preserve the bytes and the named account.
pub trait KeychainItems {
    fn read(&self, service: &str) -> io::Result<Option<GenericPassword>>;
    fn write(&self, service: &str, item: &GenericPassword) -> io::Result<()>;
    fn delete(&self, service: &str, account: &str) -> io::Result<()>;
}

#[derive(PartialEq, Eq)]
pub struct GenericPassword {
    pub account: String,
    pub value: Vec<u8>,
}

impl Drop for GenericPassword {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Credentials(Vec<Relocation>);

#[derive(Serialize, Deserialize)]
struct Relocation {
    folder: PathBuf,
    state: State,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "step")]
enum State {
    Check,
    Missing,
    Shared {
        account: String,
    },
    Copy {
        account: String,
        previous: Option<String>,
    },
}

impl Credentials {
    pub(super) fn plan(plan: &Plan) -> Result<Self, Error> {
        let Some(agents) = agents(plan) else {
            return Ok(Self::default());
        };
        let disk = Disk(agents.from.clone());
        let console = Path::new("claude/console");
        let accounts = Path::new("claude/accounts");
        let mut folders = Vec::new();
        if metadata(&disk.path(console)?)?.is_some_and(|entry| entry.is_dir()) {
            folders.push(console.to_owned());
        }
        if metadata(&disk.path(accounts)?)?.is_some_and(|entry| entry.is_dir()) {
            for entry in entries(&disk.path(accounts)?)? {
                if entry.file_type()?.is_dir() {
                    folders.push(accounts.join(entry.file_name()));
                }
            }
        }
        folders.sort();
        let credentials = Self(
            folders
                .into_iter()
                .map(|folder| Relocation {
                    folder,
                    state: State::Check,
                })
                .collect(),
        );
        credentials.validate(plan)?;
        Ok(credentials)
    }

    pub(super) fn len(&self) -> usize {
        self.0.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(super) fn validate(&self, plan: &Plan) -> Result<(), Error> {
        let mut services = BTreeSet::new();
        for relocation in &self.0 {
            let names = names(plan, &relocation.folder)?;
            let fresh = if names.from == names.to {
                services.insert(names.from)
            } else {
                [names.from, names.to, names.aside]
                    .into_iter()
                    .all(|name| services.insert(name))
            };
            if !fresh {
                return Err(invalid(
                    "Two Claude Code accounts use the same Keychain service in this move.",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn actions(&self, plan: &Plan) -> Result<Vec<Value>, Error> {
        self.0.iter().filter_map(|relocation| match &relocation.state {
            State::Copy {account, previous} => Some((relocation, account, previous)),
            _ => None,
        }).map(|(relocation, account, previous)| {
            let names = names(plan, &relocation.folder)?;
            let mut action = json!({"act":"keychain", "service":names.to, "account":previous.as_ref().unwrap_or(account), "formatVersion":1});
            if previous.is_some() { action["aside"] = json!(names.aside); }
            Ok(action)
        }).collect()
    }

    /// Record the old account name before writing anything into the Keychain.
    pub(super) fn prepare(
        &mut self,
        index: usize,
        plan: &Plan,
        items: &dyn KeychainItems,
    ) -> Result<bool, Error> {
        let relocation = &mut self.0[index];
        if !matches!(relocation.state, State::Check) {
            return Ok(false);
        }
        let names = names(plan, &relocation.folder)?;
        let original = if names.from == names.to {
            None
        } else {
            items.read(&names.from)?
        };
        relocation.state = match original {
            None => State::Missing,
            Some(original) => {
                let there = items.read(&names.to)?;
                if there
                    .as_ref()
                    .is_some_and(|target| target.value == original.value)
                {
                    State::Shared {
                        account: original.account.clone(),
                    }
                } else {
                    if items.read(&names.aside)?.is_some() {
                        return Err(changed());
                    }
                    State::Copy {
                        account: original.account.clone(),
                        previous: there.map(|item| item.account.clone()),
                    }
                }
            }
        };
        Ok(true)
    }

    pub(super) fn copy(
        &self,
        index: usize,
        plan: &Plan,
        items: &dyn KeychainItems,
        complete: &mut impl FnMut(CutoverStep),
    ) -> Result<(), Error> {
        let relocation = &self.0[index];
        let names = names(plan, &relocation.folder)?;
        let (account, previous) = match &relocation.state {
            State::Check => return Err(changed()),
            State::Missing => return Ok(()),
            State::Shared { account } => {
                let original = account_item(items, &names.from, account)?.ok_or_else(changed)?;
                if items
                    .read(&names.to)?
                    .is_some_and(|target| target.value == original.value)
                {
                    return Ok(());
                }
                return Err(changed());
            }
            State::Copy { account, previous } => (account, previous),
        };
        let original = account_item(items, &names.from, account)?.ok_or_else(changed)?;
        let target = items.read(&names.to)?;
        let kept = items.read(&names.aside)?;
        if let Some(previous) = previous {
            let prior = match (&kept, &target) {
                (Some(kept), _) if &kept.account == previous => kept,
                (None, Some(target)) if &target.account == previous && target != &original => {
                    write_checked(items, &names.aside, target)?;
                    complete(CutoverStep::CredentialAside);
                    target
                }
                _ => return Err(changed()),
            };
            if target
                .as_ref()
                .is_some_and(|target| target != prior && target != &original)
            {
                return Err(changed());
            }
            if previous != account && target.as_ref() == Some(prior) {
                items.delete(&names.to, previous)?;
            }
        } else if kept.is_some() || target.as_ref().is_some_and(|item| item != &original) {
            return Err(changed());
        }
        if target.as_ref() != Some(&original) {
            items.write(&names.to, &original)?;
            complete(CutoverStep::CredentialCopy);
        }
        verify(items, &names.to, &original)
    }

    pub(super) fn undo(&self, plan: &Plan, items: &dyn KeychainItems) -> Result<(), Error> {
        for relocation in self.0.iter().rev() {
            let State::Copy { account, previous } = &relocation.state else {
                continue;
            };
            let names = names(plan, &relocation.folder)?;
            let source = account_item(items, &names.from, account)?;
            let target = items.read(&names.to)?;
            if let Some(previous) = previous {
                let Some(kept) = account_item(items, &names.aside, previous)? else {
                    if target.as_ref().is_some_and(|item| {
                        &item.account == previous && Some(item) != source.as_ref()
                    }) {
                        continue;
                    }
                    return Err(changed());
                };
                if target.as_ref() != Some(&kept) {
                    if let Some(target) = &target {
                        if Some(target) != source.as_ref() {
                            return Err(changed());
                        }
                        if &target.account != previous {
                            items.delete(&names.to, &target.account)?;
                        }
                    }
                    write_checked(items, &names.to, &kept)?;
                }
                items.delete(&names.aside, previous)?;
            } else if let Some(target) = target {
                if Some(&target) != source.as_ref() {
                    return Err(changed());
                }
                items.delete(&names.to, &target.account)?;
            }
        }
        Ok(())
    }

    pub(super) fn retire(
        &self,
        plan: &Plan,
        items: &dyn KeychainItems,
        complete: &mut impl FnMut(CutoverStep),
    ) -> Result<(), Error> {
        for relocation in &self.0 {
            let (account, previous, copied) = match &relocation.state {
                State::Check => return Err(changed()),
                State::Missing => continue,
                State::Shared { account } => (account, None, false),
                State::Copy { account, previous } => (account, previous.as_deref(), true),
            };
            let names = names(plan, &relocation.folder)?;
            let target = items.read(&names.to)?.ok_or_else(changed)?;
            if copied && &target.account != account {
                return Err(changed());
            }
            if let Some(original) = account_item(items, &names.from, account)? {
                if target.value != original.value {
                    return Err(changed());
                }
                items.delete(&names.from, account)?;
                complete(CutoverStep::CredentialRetired);
            }
            if let Some(previous) = previous
                && account_item(items, &names.aside, previous)?.is_some()
            {
                items.delete(&names.aside, previous)?;
            }
        }
        Ok(())
    }
}

struct Names {
    from: String,
    to: String,
    aside: String,
}

fn agents(plan: &Plan) -> Option<&super::Placement> {
    plan.placements
        .iter()
        .find(|item| !item.installed && item.to == plan.to.join("agents"))
}

fn names(plan: &Plan, relative: &Path) -> Result<Names, Error> {
    let fields = relative.components().collect::<Vec<_>>();
    if relative != Path::new("claude/console")
        && !(relative.starts_with("claude/accounts")
            && fields.len() == 3
            && fields
                .iter()
                .all(|part| matches!(part, Component::Normal(_))))
    {
        return Err(invalid(
            "The move has an invalid Claude Code account folder.",
        ));
    }
    let agents = agents(plan).ok_or_else(changed)?;
    let from = service_name(&agents.from.join(relative))?;
    let to = service_name(&agents.to.join(relative))?;
    let aside = format!("{to}.before-kuvo-move");
    Ok(Names { from, to, aside })
}

fn service_name(folder: &Path) -> Result<String, Error> {
    let path = folder
        .to_str()
        .ok_or_else(|| invalid("A Claude Code account folder needs a UTF-8 name."))?;
    let folded: String = path.nfc().collect();
    let hash = Sha256::digest(folded.as_bytes());
    Ok(format!(
        "Claude Code-credentials-{}",
        hex::encode(&hash[..4])
    ))
}

fn account_item(
    items: &dyn KeychainItems,
    service: &str,
    account: &str,
) -> Result<Option<GenericPassword>, Error> {
    match items.read(service)? {
        Some(item) if item.account != account => Err(changed()),
        other => Ok(other),
    }
}

fn write_checked(
    items: &dyn KeychainItems,
    service: &str,
    item: &GenericPassword,
) -> Result<(), Error> {
    items.write(service, item)?;
    verify(items, service, item)
}

fn verify(items: &dyn KeychainItems, service: &str, item: &GenericPassword) -> Result<(), Error> {
    if items.read(service)?.as_ref() != Some(item) {
        return Err(invalid(
            "Claude Code's sign-in didn't read back as it was written.",
        ));
    }
    Ok(())
}

fn changed() -> Error {
    invalid("A Claude Code sign-in changed during the move. Try again.")
}
