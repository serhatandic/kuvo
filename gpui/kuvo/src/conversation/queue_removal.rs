//! The desktop's record of Edit/Remove operations awaiting the box's answer.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};

static ACTIVE: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(Mutex::default);

#[derive(Serialize, Deserialize)]
pub(super) struct Removal {
    pub item: String,
    pub edit: bool,
    receipt: String,
}

pub(super) struct Pending {
    pub removal: Removal,
    file: Option<PathBuf>,
}

impl Pending {
    fn claim(file: &Path, read: impl FnOnce() -> io::Result<Removal>) -> io::Result<Option<Self>> {
        let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
        if active.contains(file) { return Ok(None) }
        let removal = read()?;
        active.insert(file.to_owned());
        Ok(Some(Self { removal, file: Some(file.to_owned()) }))
    }

    pub fn begin(folder: Option<&Path>, item: String, edit: bool) -> io::Result<Option<Self>> {
        let Some(folder) = folder else {
            return Ok(Some(Self { removal: Removal { item, edit, receipt: String::new() }, file: None }));
        };
        let file = folder.join(format!("item-{}.json", kuvo_box::paths::segment(&item)));
        Self::claim(&file, || match read(&file) {
            Ok(saved) => Ok(saved),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let removal = Removal { item, edit, receipt: kuvo_box::ids::message() };
                let beside = file.with_extension("new");
                kuvo_box::boxes::write_private(&beside, &serde_json::to_vec(&removal)?)?;
                fs::File::open(&beside)?.sync_all()?;
                fs::rename(&beside, &file)?;
                fs::File::open(folder)?.sync_all()?;
                Ok(removal)
            }
            Err(error) => Err(error),
        })
    }

    pub fn recover(folder: &Path) -> io::Result<Vec<Self>> {
        let entries = match fs::read_dir(folder) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut work = Vec::new();
        for entry in entries {
            let file = entry?.path();
            if file.extension().is_some_and(|ext| ext == "json") && let Some(pending) = Self::claim(&file, || read(&file))? {
                work.push(pending);
            }
        }
        Ok(work)
    }

    pub fn receipt(&self) -> Option<&str> {
        self.file.as_ref().map(|_| self.removal.receipt.as_str())
    }

    pub fn finish(&self) {
        if let Some(file) = &self.file { let _ = fs::remove_file(file); }
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(file) = &self.file { ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).remove(file); }
    }
}

fn read(file: &Path) -> io::Result<Removal> {
    Ok(serde_json::from_slice(&fs::read(file)?)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_edit_keeps_its_operation_id_until_it_has_been_applied_and_acknowledged() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("session");
        let first = Pending::begin(Some(&folder), "msg_row".into(), true).unwrap().unwrap();
        let receipt = first.receipt().unwrap().to_owned();
        assert!(Pending::recover(&folder).unwrap().is_empty());
        drop(first);

        let recovered = Pending::recover(&folder).unwrap().pop().unwrap();
        assert_eq!(recovered.receipt(), Some(receipt.as_str()));
        assert_eq!(recovered.removal.item, "msg_row");
        assert!(recovered.removal.edit);
        recovered.finish();
        drop(recovered);
        assert!(Pending::recover(&folder).unwrap().is_empty());
    }
}
