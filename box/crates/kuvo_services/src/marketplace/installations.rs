//! Marketplace installations state (`standalone-v1.json` or setup-wide signed shared state)
//! and local skill projection (`project_skills`, `removed-skills.json`).

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use kuvo_setup::SetupHandle;
use kuvo_store::locks::OwnedLock;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    config::ConfigPaths,
    skills::{
        names::{is_item_id, is_skill_folder},
        store::{MARKETPLACE_SKILL, SkillFile, SkillPackage, SkillSource, SkillStore, sha256_hex, valid_skill_source},
    },
};

use super::{
    catalog::{is_endpoint, valid_catalog_skill_files},
    skill_sources::{BuiltinSkill, archive_skill, archived_skill, folder_skills, restore_archive},
};

const MAX_BYTES: usize = 110 * 1024;
const CORRUPT: &str = "The saved Marketplace data could not be read.";
const FULL: &str = "Your setup’s Marketplace storage is full. Remove a skill before adding another.";
const SAVE_ERR: &str = "The main computer couldn’t save the Marketplace change.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledApp { pub id: String, pub title: String, pub category: String, pub summary: String, pub url: String, pub auth: String, pub origin: String }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledSkill {
    pub name: String,
    pub files: Vec<SkillFile>,
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SkillSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovedManaged {
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SkillSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovedSkill {
    pub name: String,
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin: Option<BuiltinSkill>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed: Option<RemovedManaged>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installations {
    pub format: u32,
    pub apps: Vec<InstalledApp>,
    pub skills: Vec<InstalledSkill>,
    #[serde(rename = "removedSkills", default, skip_serializing_if = "Option::is_none")]
    pub removed_skills: Option<Vec<RemovedSkill>>,
}

impl Default for Installations {
    fn default() -> Self { Self { format: 1, apps: Vec::new(), skills: Vec::new(), removed_skills: None } }
}

pub fn removal_for<'a>(state: &'a Installations, name: &str) -> Option<&'a RemovedSkill> {
    state.removed_skills.as_deref()?.iter().find(|r| r.name == name && r.restored != Some(true))
}
pub fn skill_revision(skill: &InstalledSkill) -> String { sha256_hex(serde_json::to_string(skill).unwrap_or_default().as_bytes()) }
fn non_empty(s: &str, max: usize) -> bool { !s.trim().is_empty() && s.len() <= max }

pub fn valid_installed_app(a: &InstalledApp) -> bool {
    is_item_id(&a.id) && a.id.len() <= 52 && non_empty(&a.title, 100) && non_empty(&a.category, 50)
        && non_empty(&a.summary, 200) && is_endpoint(&a.url)
        && matches!(a.auth.as_str(), "open" | "token" | "oauth") && matches!(a.origin.as_str(), "catalog" | "custom")
}

pub fn valid_installed_skill(s: &InstalledSkill) -> bool {
    is_item_id(&s.name) && valid_catalog_skill_files(&s.files) && match s.origin.as_str() {
        "github" => s.source.as_ref().is_some_and(valid_skill_source),
        "catalog" => s.source.is_none() && s.version.as_deref().is_some_and(|v| non_empty(v, 64)),
        "folder" | "created" => s.source.is_none(),
        _ => false,
    }
}

fn valid_removed_skill(r: &RemovedSkill) -> bool {
    is_skill_folder(&r.name) && r.key.len() == 36 && r.key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'-'))
        && r.restored.is_none_or(|b| b)
        && r.builtin.as_ref().is_none_or(|b| b.id == r.name && non_empty(&b.title, 1024) && b.summary.len() <= 4096)
        && r.managed.as_ref().is_none_or(|m| match m.origin.as_str() {
            "github" => m.source.as_ref().is_some_and(valid_skill_source),
            "catalog" => m.version.as_deref().is_some_and(|v| non_empty(v, 64)),
            "folder" | "created" => true,
            _ => false,
        })
}

pub fn parse_installations(val: Option<&Value>) -> Result<Installations, String> {
    let Some(val) = val else { return Ok(Installations::default()) };
    if val.get("format").and_then(Value::as_u64) != Some(1) { return Err(CORRUPT.into()); }
    let parsed: Installations = serde_json::from_value(val.clone()).map_err(|_| CORRUPT.to_owned())?;
    if parsed.apps.iter().all(valid_installed_app) && parsed.skills.iter().all(valid_installed_skill)
        && parsed.removed_skills.as_deref().is_none_or(|l| l.iter().all(valid_removed_skill))
    { Ok(parsed) } else { Err(CORRUPT.into()) }
}

pub enum SaveOutcome { Saved { state: Installations, receipt: Option<Value> }, Done { status: u16, value: Value } }
fn done(status: u16, msg: &str) -> SaveOutcome { SaveOutcome::Done { status, value: json!({ "message": msg }) } }

pub struct InstallationsStore {
    pub home: PathBuf,
    pub account_home: PathBuf,
    pub marketplace_dir: PathBuf,
    standalone_file: PathBuf,
    install_lock: PathBuf,
    skill_lock: PathBuf,
    removed_file: PathBuf,
    setup: Option<SetupHandle>,
    pub skills: Arc<SkillStore>,
    pub config: ConfigPaths,
}

impl InstallationsStore {
    pub fn new(home: &Path, account_home: &Path, setup: Option<SetupHandle>, skills: Arc<SkillStore>, config: ConfigPaths) -> Self {
        let dir = home.join("box/marketplace");
        Self {
            home: home.to_owned(), account_home: account_home.to_owned(),
            standalone_file: dir.join("standalone-v1.json"), install_lock: dir.join("installations.lock"),
            skill_lock: dir.join("skills.lock"), removed_file: dir.join("removed-skills.json"),
            marketplace_dir: dir, setup, skills, config,
        }
    }

    pub fn setup_state(&self) -> Option<kuvo_setup::MarketplaceSetup> { self.setup.as_ref().and_then(SetupHandle::marketplace_setup) }

    pub fn read(&self) -> Result<Installations, String> {
        if let Some(shared) = self.setup_state() { return parse_installations(shared.value.as_ref()); }
        match fs::read_to_string(&self.standalone_file) {
            Ok(text) => parse_installations(Some(&serde_json::from_str::<Value>(&text).map_err(|_| CORRUPT.to_owned())?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Installations::default()),
            Err(e) => Err(e.to_string()),
        }
    }

    pub async fn save<F>(&self, mut change: F) -> SaveOutcome where F: FnMut(Installations) -> Result<Installations, (u16, Value)> {
        if let Some(setup) = &self.setup && let Some(shared) = setup.marketplace_setup() {
            let before = match parse_installations(shared.value.as_ref()) { Ok(v) => v, Err(m) => return done(503, &m) };
            let after = match change(before) { Ok(v) => v, Err((status, value)) => return SaveOutcome::Done { status, value } };
            let encoded = serde_json::to_value(&after).unwrap_or_default();
            if serde_json::to_vec(&encoded).map_or(usize::MAX, |b| b.len()) > MAX_BYTES { return done(413, FULL); }
            return match setup.save_marketplace_installations(Some(shared.sequence), encoded).await {
                Ok(r) => SaveOutcome::Saved { state: after, receipt: Some(json!(r)) },
                Err(_) => done(503, SAVE_ERR),
            };
        }
        let _ = kuvo_store::files::create_private_dirs(&self.marketplace_dir);
        let Ok(_guard) = OwnedLock::acquire(&self.install_lock, Duration::from_secs(15)) else { return done(503, SAVE_ERR); };
        if self.setup_state().is_some() { return done(409, "Your setup changed. Try again."); }
        let before = match self.read() { Ok(v) => v, Err(m) => return done(503, &m) };
        let after = match change(before) { Ok(v) => v, Err((status, value)) => return SaveOutcome::Done { status, value } };
        let bytes = serde_json::to_vec(&after).unwrap_or_default();
        if bytes.len() > MAX_BYTES { return done(413, FULL); }
        match kuvo_store::files::write_atomic(&self.standalone_file, &bytes) {
            Ok(()) => SaveOutcome::Saved { state: after, receipt: None },
            Err(_) => done(503, SAVE_ERR),
        }
    }

    pub fn project_skills(&self, state: Option<&Installations>, discovered: &[PathBuf]) -> Vec<String> {
        let _ = kuvo_store::files::create_private_dirs(&self.marketplace_dir);
        let Ok(_guard) = OwnedLock::acquire(&self.skill_lock, Duration::from_secs(15)) else {
            return vec!["Couldn’t lock Marketplace skill projection.".into()];
        };
        let owned;
        let state = match state {
            Some(s) => s,
            None => match self.read() { Ok(s) => { owned = s; &owned } Err(e) => return vec![e] },
        };
        let mut problems = Vec::new();
        let removals: HashMap<&str, &RemovedSkill> = state.removed_skills.as_deref().into_iter().flatten()
            .filter(|r| r.restored != Some(true)).map(|r| (r.name.as_str(), r)).collect();
        let mut removed_names: Vec<&str> = removals.keys().copied().collect();
        removed_names.sort();
        let names_json = serde_json::to_string(&removed_names).unwrap_or_else(|_| "[]".into());
        if (!removals.is_empty() || self.removed_file.exists()) && kuvo_store::files::text_of(&self.removed_file).as_deref() != Some(names_json.as_str()) {
            let _ = kuvo_store::files::write_atomic(&self.removed_file, names_json.as_bytes());
        }

        let restored: HashMap<&str, &RemovedSkill> = state.removed_skills.as_deref().into_iter().flatten()
            .filter(|r| r.restored == Some(true)).map(|r| (r.key.as_str(), r)).collect();
        if !restored.is_empty() {
            let mut archives: Vec<_> = kuvo_store::files::list_of(&self.skills.trash_dir).into_iter()
                .filter_map(|os| os.into_string().ok())
                .filter_map(|id| Some((id.clone(), archived_skill(&self.skills.trash_dir, &id)?))).collect();
            archives.sort_by(|a, b| b.0.cmp(&a.0));
            let mut returned = HashSet::new();
            for (id, rec) in archives {
                if !restored.contains_key(rec.key.as_str()) || !returned.insert(rec.folder.clone()) { continue; }
                if rec.managed && let Some(skill) = state.skills.iter().find(|s| s.name == rec.name) {
                    let _ = kuvo_store::files::write_atomic(&self.skills.trash_dir.join(&id).join(MARKETPLACE_SKILL), skill_revision(skill).as_bytes());
                }
                if restore_archive(&self.skills.trash_dir, &id, &rec).is_err() {
                    problems.push(format!("Couldn’t restore {} on this computer.", rec.name));
                }
            }
        }

        if !removals.is_empty() {
            let only: HashSet<String> = removals.keys().map(|s| (*s).to_owned()).collect();
            for skill in folder_skills(&self.home, &self.account_home, &self.skills.skills_dir, &self.config, discovered, Some(&only)) {
                let base = skill.folder.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let Some(rem) = removals.get(skill.id.as_str()).or_else(|| removals.get(base.as_str())) else { continue };
                if archive_skill(&self.skills.trash_dir, &skill.folder, &rem.name, &rem.key).is_err() {
                    problems.push(format!("Couldn’t move {} to Recently Deleted on this computer.", skill.title));
                }
            }
        }

        let wanted: HashSet<&str> = state.skills.iter().map(|s| s.name.as_str()).collect();
        for name in kuvo_store::files::list_of(&self.skills.skills_dir).into_iter().filter_map(|os| os.into_string().ok()) {
            let folder = self.skills.skills_dir.join(&name);
            if !wanted.contains(name.as_str()) && is_item_id(&name)
                && fs::symlink_metadata(&folder).is_ok_and(|m| m.file_type().is_dir())
                && kuvo_store::files::text_of(&folder.join(MARKETPLACE_SKILL)).is_some()
            {
                let res = match removals.get(name.as_str()) {
                    Some(rem) => archive_skill(&self.skills.trash_dir, &folder, &name, &rem.key),
                    None => fs::remove_dir_all(&folder),
                };
                if res.is_err() { problems.push(format!("Couldn’t remove {name} on this computer.")); }
            }
        }

        for skill in &state.skills {
            let rev = skill_revision(skill);
            if kuvo_store::files::text_of(&self.skills.skills_dir.join(&skill.name).join(MARKETPLACE_SKILL)).as_deref() == Some(rev.as_str()) { continue; }
            let pkg = SkillPackage { name: skill.name.clone(), files: skill.files.clone(), source: skill.source.clone() };
            if let Err(err) = self.skills.write_skill_package(&pkg, Some(&rev)) { problems.push(err.message); }
        }
        problems
    }
}
