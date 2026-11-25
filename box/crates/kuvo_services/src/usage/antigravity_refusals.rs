//! The maker's refusals, in the same account-specific derived file as the
//! reference reader. No credential is stored in this journal.
use super::{Problem, ProblemKind, fingerprint, problem};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
};
#[derive(Clone, Default, Serialize, Deserialize)]
struct Saved {
    generations: BTreeSet<String>,
    access: BTreeSet<String>,
    csrf: BTreeSet<String>,
    #[serde(default)]
    refresh: BTreeSet<String>,
}
#[derive(Default)]
pub(super) struct Refusals {
    loaded: Option<Saved>,
    dirty: bool,
}
#[derive(Clone, Copy)]
pub(super) enum Kind {
    Generation,
    Access,
    Refresh,
    Csrf,
}
impl Saved {
    fn set(&self, kind: Kind) -> &BTreeSet<String> {
        match kind {
            Kind::Generation => &self.generations,
            Kind::Access => &self.access,
            Kind::Refresh => &self.refresh,
            Kind::Csrf => &self.csrf,
        }
    }
    fn set_mut(&mut self, kind: Kind) -> &mut BTreeSet<String> {
        match kind {
            Kind::Generation => &mut self.generations,
            Kind::Access => &mut self.access,
            Kind::Refresh => &mut self.refresh,
            Kind::Csrf => &mut self.csrf,
        }
    }
    fn valid(&self) -> bool {
        [&self.generations, &self.access, &self.csrf, &self.refresh]
            .into_iter()
            .flat_map(|set| set.iter())
            .all(|hash| {
                hash.len() == 16
                    && hash
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
    }
}
fn path(root: &Path, account: &str, replace_leaf: bool) -> io::Result<PathBuf> {
    if account.is_empty()
        || account.len() > 100
        || !account
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err(io::Error::other("Invalid Antigravity account"));
    }
    let root = root.canonicalize()?;
    let mut path = root.clone();
    for part in ["usage", "antigravity", account, "refusals.json"] {
        path.push(part);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) => {
                if part == "refusals.json" && replace_leaf && meta.file_type().is_symlink() {
                    continue;
                }
                if meta.file_type().is_symlink() || !path.canonicalize()?.starts_with(&root) {
                    return Err(io::Error::other(
                        "Antigravity refusal record is not in its own folder",
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(path)
}
fn unreadable() -> Problem {
    problem(
        ProblemKind::Failed,
        "Antigravity's credential refusal record cannot be read; no credential was sent.",
    )
}
fn unwritable() -> Problem {
    problem(
        ProblemKind::Failed,
        "Antigravity's credential refusal record cannot be saved; the last quota reading is kept.",
    )
}
impl Refusals {
    pub fn prepare(&mut self, root: &Path, account: &str) -> Result<(), Problem> {
        if self.loaded.is_none() {
            let path = path(root, account, false).map_err(|_| unreadable())?;
            let saved = match kuvo_store::files::read(&path).map_err(|_| unreadable())? {
                None => Saved::default(),
                Some(raw) => serde_json::from_slice::<Saved>(&raw).map_err(|_| unreadable())?,
            };
            if !saved.valid() {
                return Err(unreadable());
            }
            self.loaded = Some(saved);
        } else if !self.dirty {
            path(root, account, false).map_err(|_| unreadable())?;
        }
        self.flush(root, account)
    }
    pub fn denied(&self, kind: Kind, secret: &str) -> bool {
        self.denied_hash(kind, &fingerprint(secret))
    }
    pub fn denied_hash(&self, kind: Kind, hash: &str) -> bool {
        self.loaded
            .as_ref()
            .is_some_and(|s| s.set(kind).contains(hash))
    }
    pub fn refuse(
        &mut self,
        root: &Path,
        account: &str,
        kind: Kind,
        secret: &str,
    ) -> Result<(), Problem> {
        self.refuse_hash(root, account, kind, &fingerprint(secret))
    }
    pub fn refuse_hash(
        &mut self,
        root: &Path,
        account: &str,
        kind: Kind,
        hash: &str,
    ) -> Result<(), Problem> {
        let saved = self.loaded.as_mut().ok_or_else(unreadable)?;
        self.dirty |= saved.set_mut(kind).insert(hash.to_owned());
        self.flush(root, account)
    }
    fn flush(&mut self, root: &Path, account: &str) -> Result<(), Problem> {
        if !self.dirty {
            return Ok(());
        }
        let path = path(root, account, true).map_err(|_| unwritable())?;
        let bytes = serde_json::to_vec(self.loaded.as_ref().ok_or_else(unreadable)?)
            .map_err(|_| unwritable())?;
        kuvo_store::files::write_secret(&path, &bytes).map_err(|_| unwritable())?;
        self.dirty = false;
        Ok(())
    }
    pub fn clear(&mut self, root: &Path, account: &str) -> io::Result<()> {
        let file = path(root, account, true)?;
        match std::fs::remove_file(file) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        *self = Self::default();
        Ok(())
    }
}
