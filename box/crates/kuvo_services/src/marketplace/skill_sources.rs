//! Local skill discovery across agent roots, projects and Other Places, plus archiving/restoring.

use std::{collections::{BTreeMap, HashSet}, fs, io, path::{Path, PathBuf}};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    config::{ConfigPaths, SettingsRead},
    integrations::routes::EngineClient,
    skills::{names::{is_skill_folder, skill_about, skill_trash_entry}, store::{MARKETPLACE_SKILL, now_secs}},
};

pub const AGENT_PLACES: &[&str] = &[".claude", ".codex", ".agents", ".cursor", ".grok", ".gemini", ".opencode"];

#[derive(Debug, Clone)]
pub struct FolderSkill { pub id: String, pub title: String, pub summary: String, pub folder: PathBuf, pub text: String }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuiltinSkill { pub id: String, pub title: String, pub summary: String }

#[derive(Debug, Clone)]
pub struct BuiltinOffer { pub skill: BuiltinSkill, pub text: Option<String> }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivedSkill { pub name: String, pub key: String, pub folder: String, pub managed: bool }

pub fn builtin_path(place: &str) -> bool { place.starts_with("skill://") || place.starts_with("/builtin/") }
fn trash_record(trash_dir: &Path, id: &str) -> PathBuf { trash_dir.join(format!(".{id}.json")) }

fn expand_tilde(raw: &str, account_home: &Path) -> Option<PathBuf> {
    if let Some(rest) = raw.strip_prefix("~/") { Some(account_home.join(rest)) }
    else if raw == "~" { Some(account_home.to_path_buf()) }
    else if Path::new(raw).is_absolute() { Some(PathBuf::from(raw)) }
    else { None }
}

fn native_agent_skill_roots(home: &Path) -> Vec<PathBuf> {
    let bridge = crate::config::box_env(home, "KUVO_AGENTS_HOME").map(PathBuf::from)
        .unwrap_or_else(|| if crate::config::box_env(home, "KUVO_BOX").as_deref() == Some("mac") { home.parent().unwrap_or(home).join("agents") } else { home.join("agents") });
    let mut out = vec![bridge.join("antigravity").join("profile").join("antigravity-cli").join("skills")];
    let Some(items) = fs::read_to_string(bridge.join("accounts.json")).ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v.get("accounts")?.as_array().cloned()) else { return out };
    for acc in items {
        let Some(kind) = acc.get("kind").and_then(Value::as_str) else { continue };
        let id = acc.get("id").and_then(Value::as_str).unwrap_or("");
        let folder = acc.get("home").and_then(Value::as_str).map(PathBuf::from).or_else(|| (!id.is_empty()).then(|| bridge.join(kind).join(id)));
        if kind == "antigravity" { out.push(folder.map_or_else(|| bridge.join("antigravity").join("profile"), |f| f.join("profile")).join("antigravity-cli").join("skills")); }
        else if let Some(f) = folder { out.push(f.join("skills")); }
    }
    out
}

pub fn skill_roots(home: &Path, account_home: &Path, skills_dir: &Path, config: &ConfigPaths) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut roots = vec![skills_dir.to_path_buf()];
    roots.extend(native_agent_skill_roots(home));
    let mut bases = vec![account_home.to_path_buf()];
    if let Ok(projects) = crate::git::projects::read_projects(home) {
        for proj in projects {
            if let Some(d) = proj.get("directory").and_then(Value::as_str).filter(|s| Path::new(s).is_absolute()) { bases.push(PathBuf::from(d)); }
            if let Some(fs) = proj.get("folders").and_then(Value::as_array) {
                bases.extend(fs.iter().filter_map(Value::as_str).filter(|s| Path::new(s).is_absolute()).map(PathBuf::from));
            }
        }
    }
    for base in bases { for agent in AGENT_PLACES { roots.push(base.join(agent).join("skills")); } }
    for var in ["CLAUDE_CONFIG_DIR", "CODEX_HOME", "GROK_HOME"] {
        if let Some(p) = std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from).filter(|p| p.is_absolute()) { roots.push(p.join("skills")); }
    }
    let places: Vec<PathBuf> = match config.read_settings() {
        SettingsRead::Ok(map) => map.get("skills").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str)
            .filter_map(|p| expand_tilde(p, account_home)).filter(|p| p != account_home && !account_home.starts_with(p) && p.parent().is_some()).collect(),
        SettingsRead::Problem(_) => Vec::new(),
    };
    let mut seen = HashSet::new();
    roots.retain(|p| seen.insert(p.clone()));
    (roots, places)
}

fn read_skill_folder(folder: &Path) -> Option<FolderSkill> {
    let meta = fs::symlink_metadata(folder).ok()?;
    if !meta.file_type().is_dir() && !meta.file_type().is_symlink() { return None; }
    let file = folder.join("SKILL.md");
    let info = fs::metadata(&file).ok()?;
    if !info.is_file() || info.len() > 1024 * 1024 { return None; }
    let text = kuvo_store::files::text_of(&file)?;
    let about = skill_about(&text);
    let base = folder.file_name()?.to_string_lossy().into_owned();
    let id = about.name.as_deref().filter(|n| is_skill_folder(n)).map(str::to_owned).unwrap_or(base);
    if !is_skill_folder(&id) { return None; }
    Some(FolderSkill { title: about.name.filter(|s| !s.is_empty()).unwrap_or_else(|| id.clone()), id, summary: about.description.unwrap_or_default(), folder: folder.to_path_buf(), text })
}

fn discovered_folder(file: &str) -> Option<PathBuf> {
    let p = Path::new(file);
    if !p.is_absolute() || p.file_name().and_then(|n| n.to_str()) != Some("SKILL.md") { return None; }
    let folder = p.parent()?.to_path_buf();
    let parts: Vec<String> = folder.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    parts.iter().enumerate().any(|(i, part)| part == "skills" && i > 0 && i + 1 < parts.len() && AGENT_PLACES.contains(&parts[i - 1].as_str())).then_some(folder)
}

pub async fn engine_skill_offers(engine: &EngineClient, places: &[PathBuf]) -> (Vec<BuiltinOffer>, Vec<PathBuf>) {
    let (mut builtins, mut folders) = (Vec::new(), Vec::new());
    let Some(t) = engine.target() else { return (builtins, folders) };
    let _ = engine.call(&t, reqwest::Method::GET, "/api/provider", None).await;
    let Some(Value::Array(list)) = engine.call(&t, reqwest::Method::GET, "/api/skill", None).await else { return (builtins, folders) };
    for entry in list {
        let Some(id) = entry.get("id").and_then(Value::as_str).filter(|s| is_skill_folder(s)) else { continue };
        let Some(path) = entry.get("path").and_then(Value::as_str) else { continue };
        if builtin_path(path) {
            builtins.push(BuiltinOffer {
                skill: BuiltinSkill { id: id.into(), title: entry.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(id).into(), summary: entry.get("description").and_then(Value::as_str).unwrap_or("").into() },
                text: entry.get("content").and_then(Value::as_str).map(str::to_owned),
            });
        } else {
            let p = PathBuf::from(path);
            let f = discovered_folder(path).or_else(|| (p.is_absolute() && p.file_name().and_then(|n| n.to_str()) == Some("SKILL.md") && places.iter().any(|pl| p.starts_with(pl))).then(|| p.parent().map(Path::to_path_buf)).flatten());
            if let Some(f) = f { folders.push(f); }
        }
    }
    (builtins, folders)
}

pub fn folder_skills(home: &Path, account_home: &Path, skills_dir: &Path, config: &ConfigPaths, discovered: &[PathBuf], only: Option<&HashSet<String>>) -> Vec<FolderSkill> {
    let (mut found, mut seen) = (BTreeMap::new(), HashSet::new());
    fn visit(folder: &Path, depth: usize, only: Option<&HashSet<String>>, seen: &mut HashSet<PathBuf>, found: &mut BTreeMap<PathBuf, FolderSkill>) {
        let (Some(parent), Some(base)) = (folder.parent(), folder.file_name()) else { return };
        let Ok(real_parent) = fs::canonicalize(parent) else { return };
        let canonical = real_parent.join(base);
        let Ok(info) = fs::symlink_metadata(&canonical) else { return };
        if !seen.insert(canonical.clone()) { return; }
        if let Some(skill) = read_skill_folder(&canonical) {
            if only.is_none_or(|set| set.contains(&skill.id) || set.contains(base.to_string_lossy().as_ref())) { found.insert(canonical, skill); }
            return;
        }
        if !info.file_type().is_dir() || depth == 0 { return; }
        if let Ok(entries) = fs::read_dir(&canonical) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if is_skill_folder(&name) && e.file_type().is_ok_and(|ft| ft.is_dir() || ft.is_symlink()) { visit(&canonical.join(name), depth - 1, only, seen, found); }
            }
        }
    }
    for folder in discovered { visit(folder, 0, only, &mut seen, &mut found); }
    let (mut all_roots, places) = skill_roots(home, account_home, skills_dir, config);
    for place in &places { visit(place, 0, only, &mut seen, &mut found); }
    for p in places { if !all_roots.contains(&p) { all_roots.push(p); } }
    for root in all_roots {
        if !found.contains_key(&root) && fs::metadata(&root).is_ok_and(|m| m.is_dir()) && let Ok(entries) = fs::read_dir(&root) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if is_skill_folder(&name) { visit(&root.join(name), 7, only, &mut seen, &mut found); }
            }
        }
    }
    found.into_values().collect()
}

pub fn archived_skill(trash_dir: &Path, id: &str) -> Option<ArchivedSkill> {
    skill_trash_entry(id)?;
    let parsed: ArchivedSkill = serde_json::from_str(&fs::read_to_string(trash_record(trash_dir, id)).ok()?).ok()?;
    (is_skill_folder(&parsed.name) && Path::new(&parsed.folder).is_absolute()).then_some(parsed)
}

fn copy_tree_verbatim(from: &Path, to: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        let target = fs::read_link(from)?;
        #[cfg(unix)]
        { std::os::unix::fs::symlink(target, to)?; }
        #[cfg(windows)]
        { if fs::metadata(from).is_ok_and(|m| m.is_dir()) { std::os::windows::fs::symlink_dir(target, to)?; } else { std::os::windows::fs::symlink_file(target, to)?; } }
        return Ok(());
    }
    if meta.file_type().is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? { let e = entry?; copy_tree_verbatim(&e.path(), &to.join(e.file_name()))?; }
        return Ok(());
    }
    fs::copy(from, to)?;
    Ok(())
}

fn move_folder(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::CrossesDevices => {
            let mut nonce = [0u8; 8];
            let _ = getrandom::fill(&mut nonce);
            let staged = to.parent().unwrap_or_else(|| Path::new(".")).join(format!(".kuvo-skill-{}", hex::encode(nonce)));
            let res = copy_tree_verbatim(from, &staged).and_then(|()| fs::rename(&staged, to)).and_then(|()| fs::remove_dir_all(from).or_else(|_| fs::remove_file(from)));
            let _ = fs::remove_dir_all(&staged).or_else(|_| fs::remove_file(&staged));
            res
        }
        Err(err) => Err(err),
    }
}

pub fn archive_skill(trash_dir: &Path, folder: &Path, name: &str, key: &str) -> io::Result<()> {
    kuvo_store::files::create_private_dirs(trash_dir)?;
    let mut when = now_secs();
    let mut id = format!("{name}-{when}");
    while fs::symlink_metadata(trash_dir.join(&id)).is_ok() || trash_record(trash_dir, &id).exists() { when += 1; id = format!("{name}-{when}"); }
    let rec = ArchivedSkill { folder: folder.to_string_lossy().into_owned(), name: name.into(), key: key.into(), managed: kuvo_store::files::text_of(&folder.join(MARKETPLACE_SKILL)).is_some() };
    let rec_path = trash_record(trash_dir, &id);
    kuvo_store::files::write_atomic(&rec_path, &serde_json::to_vec(&rec).map_err(io::Error::other)?)?;
    let away = trash_dir.join(&id);
    if let Err(err) = move_folder(folder, &away) {
        if fs::symlink_metadata(&away).is_err() { let _ = fs::remove_file(&rec_path); }
        return Err(err);
    }
    Ok(())
}

pub fn restore_archive(trash_dir: &Path, id: &str, rec: &ArchivedSkill) -> io::Result<()> {
    let dest = Path::new(&rec.folder);
    if let Some(p) = dest.parent() { fs::create_dir_all(p)?; }
    if fs::symlink_metadata(dest).is_ok() { return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("A skill folder already exists at {}.", rec.folder))); }
    move_folder(&trash_dir.join(id), dest)?;
    let _ = fs::remove_file(trash_record(trash_dir, id));
    Ok(())
}
