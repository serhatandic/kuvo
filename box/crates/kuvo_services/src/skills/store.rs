//! Local skills folder (`config/opencode/skills`), Recently Deleted (`trash/skills`), atomic
//! skill package staging, and `kuvo/skills*` + `kuvo/skill-trash*` routes.

use std::{
    collections::{BTreeMap, HashSet},
    fs, io::{self, Read}, path::{Path, PathBuf}, sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use http::StatusCode;
use kuvo_protocol::routes::{
    delete_kuvo_skills_name, get_kuvo_skill_trash, get_kuvo_skills, get_kuvo_skills_name,
    post_kuvo_skill_trash_id_restore, put_kuvo_skills_name,
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::integrations::routes::EngineClient;

use super::names::{
    decode_uri_component, is_github_folder, is_item_id, is_relative_file, is_skill_folder,
    skill_about, skill_name, skill_trash_entry,
};

pub const MARKETPLACE_SKILL: &str = ".kuvo-marketplace.json";
pub const SKILL_SOURCE_FILE: &str = ".kuvo-source.json";
pub const SKILL_TRASH_DAYS: i64 = 30;
pub const SKILL_FILE_LIMIT: usize = 64 * 1024;
pub const SKILL_MAX_FILES: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillFile { pub path: String, pub sha256: String, pub data: String }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSource {
    pub repository: String, pub path: String, pub commit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub r#ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPackage {
    pub name: String, pub files: Vec<SkillFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub source: Option<SkillSource>,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct SkillFailure { pub status: u16, pub message: String }

impl SkillFailure {
    pub fn new(status: u16, message: impl Into<String>) -> Self { Self { status, message: message.into() } }
    pub fn into_refusal(self) -> Refusal {
        let code = StatusCode::from_u16(self.status).unwrap_or(StatusCode::BAD_GATEWAY);
        if self.message.is_empty() { Refusal::json(code, json!({})) } else { Refusal::new(code, self.message) }
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String { hex::encode(Sha256::digest(bytes)) }
pub fn pack_file(rel: &str, bytes: &[u8]) -> SkillFile {
    SkillFile { path: rel.into(), data: STANDARD.encode(bytes), sha256: sha256_hex(bytes) }
}
pub fn is_commit_id(s: &str) -> bool { s.len() == 40 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) }
pub fn is_repo_id(s: &str) -> bool {
    s.split_once('/').is_some_and(|(o, r)| {
        (1..=39).contains(&o.len()) && (1..=100).contains(&r.len()) && !r.contains('/')
            && o.as_bytes()[0].is_ascii_alphanumeric()
            && o.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            && r.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    })
}
pub fn valid_skill_source(s: &SkillSource) -> bool {
    is_repo_id(&s.repository) && is_github_folder(&s.path) && is_commit_id(&s.commit) && s.r#ref.as_ref().is_none_or(|r| r.len() <= 240)
}
pub fn valid_skill_files(files: &[SkillFile]) -> bool {
    if files.is_empty() || files.len() > SKILL_MAX_FILES { return false; }
    let (mut seen, mut total, mut has_md) = (HashSet::new(), 0usize, false);
    for f in files {
        if !is_relative_file(&f.path) || !seen.insert(&f.path) { return false; }
        if f.path == "SKILL.md" { has_md = true; }
        let Ok(bytes) = STANDARD.decode(&f.data) else { return false };
        total = total.saturating_add(bytes.len());
        if total > SKILL_FILE_LIMIT || sha256_hex(&bytes) != f.sha256 { return false; }
    }
    has_md
}

pub fn named_package(fallback: &str, files: Vec<SkillFile>) -> Result<SkillPackage, SkillFailure> {
    let bad = || SkillFailure::new(422, "That folder does not contain a complete skill.");
    if !valid_skill_files(&files) { return Err(bad()); }
    let readme = files.iter().find(|f| f.path == "SKILL.md").ok_or_else(bad)?;
    let bytes = STANDARD.decode(&readme.data).map_err(|_| bad())?;
    let about = skill_about(&String::from_utf8_lossy(&bytes));
    let name = skill_name(about.name.as_deref().filter(|s| !s.is_empty()).unwrap_or(fallback));
    if !is_item_id(&name) { return Err(SkillFailure::new(422, "Give the skill a name in SKILL.md.")); }
    Ok(SkillPackage { name, files, source: None })
}

pub fn local_skill(folder: &Path) -> Result<SkillPackage, SkillFailure> {
    if !folder.is_absolute() || !fs::symlink_metadata(folder).is_ok_and(|m| m.file_type().is_dir()) {
        return Err(SkillFailure::new(400, "Choose a folder on this computer that contains SKILL.md."));
    }
    let (mut files, mut size) = (Vec::new(), 0usize);
    read_local_dir(folder, "", &mut files, &mut size)?;
    let fb = folder.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    named_package(&fb, files)
}

fn read_local_dir(root: &Path, prefix: &str, files: &mut Vec<SkillFile>, size: &mut usize) -> Result<(), SkillFailure> {
    let dir = if prefix.is_empty() { root.to_path_buf() } else { root.join(prefix) };
    for entry in fs::read_dir(&dir).map_err(|e| SkillFailure::new(400, format!("Could not read skill folder: {e}")))?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') { continue; }
        let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let ft = entry.file_type().map_err(|e| SkillFailure::new(422, e.to_string()))?;
        if !is_relative_file(&rel) || rel.split('/').count() > 12 || (!ft.is_dir() && !ft.is_file()) {
            return Err(SkillFailure::new(422, "Skill folders can contain regular files and folders only."));
        }
        if ft.is_dir() { read_local_dir(root, &rel, files, size)?; continue; }
        let bytes = read_regular_nofollow(&root.join(&rel), files.len(), *size)?;
        *size += bytes.len();
        files.push(pack_file(&rel, &bytes));
    }
    Ok(())
}

fn read_regular_nofollow(path: &Path, count: usize, size: usize) -> Result<Vec<u8>, SkillFailure> {
    let mut opts = fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    { use std::os::unix::fs::OpenOptionsExt; opts.custom_flags(libc::O_NOFOLLOW); }
    let only = || SkillFailure::new(422, "Skill folders can contain regular files and folders only.");
    let mut file = opts.open(path).map_err(|_| only())?;
    let meta = file.metadata().map_err(|_| only())?;
    let len = usize::try_from(meta.len()).unwrap_or(usize::MAX);
    if !meta.is_file() || count >= SKILL_MAX_FILES || size.saturating_add(len) > SKILL_FILE_LIMIT {
        return Err(SkillFailure::new(422, "The skill is too large to share with the setup."));
    }
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf).map_err(|_| SkillFailure::new(409, "A skill file changed while it was read."))?;
    Ok(buf)
}

#[derive(Debug, Clone)]
pub struct EngineSkill { pub id: String, pub path: Option<String> }

pub struct SkillStore {
    pub home: PathBuf, pub account_home: PathBuf, pub skills_dir: PathBuf,
    pub trash_dir: PathBuf, pub engine: EngineClient,
}

impl SkillStore {
    pub fn for_home(home: &Path, account_home: &Path) -> Self {
        let cfg = crate::config::box_env(home, "KUVO_ENGINE_CONFIG").map(PathBuf::from).unwrap_or_else(|| home.join("config"));
        Self { home: home.to_owned(), account_home: account_home.to_owned(), skills_dir: cfg.join("opencode").join("skills"), trash_dir: home.join("trash").join("skills"), engine: EngineClient::for_home(home) }
    }
    pub fn trash_record_path(&self, id: &str) -> PathBuf { self.trash_dir.join(format!(".{id}.json")) }

    pub fn skill_folders(&self) -> Vec<String> {
        let mut list: Vec<String> = kuvo_store::files::list_of(&self.skills_dir).into_iter().filter_map(|o| o.into_string().ok())
            .filter(|n| !n.starts_with('.') && fs::metadata(self.skills_dir.join(n)).is_ok_and(|m| m.is_dir())).collect();
        list.sort();
        list
    }

    pub fn skill_sources(&self) -> BTreeMap<String, Value> {
        let mut out = BTreeMap::new();
        for name in kuvo_store::files::list_of(&self.skills_dir).into_iter().filter_map(|o| o.into_string().ok()) {
            let Some(Value::Object(obj)) = fs::read_to_string(self.skills_dir.join(&name).join(SKILL_SOURCE_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()) else { continue };
            let Some(repo) = obj.get("repository").and_then(Value::as_str) else { continue };
            let mut m = Map::from_iter([("repository".into(), json!(repo)), ("path".into(), json!(obj.get("path").and_then(Value::as_str).unwrap_or("")))]);
            if let Some(c) = obj.get("commit").and_then(Value::as_str).filter(|c| is_commit_id(c)) { m.insert("commit".into(), json!(c)); }
            out.insert(name, Value::Object(m));
        }
        out
    }

    pub fn clear_skill_trash(&self, now: i64) {
        let cutoff = now - SKILL_TRASH_DAYS * 86_400;
        for id in kuvo_store::files::list_of(&self.trash_dir).into_iter().filter_map(|o| o.into_string().ok()) {
            if skill_trash_entry(&id).is_some_and(|e| e.deleted <= cutoff) {
                let p = self.trash_dir.join(&id);
                let _ = fs::remove_dir_all(&p).or_else(|_| fs::remove_file(&p));
                let _ = fs::remove_file(self.trash_record_path(&id));
            }
        }
    }

    pub fn skill_trash_entries(&self) -> Vec<Value> {
        let mut entries: Vec<(i64, String, Value)> = Vec::new();
        for id in kuvo_store::files::list_of(&self.trash_dir).into_iter().filter_map(|o| o.into_string().ok()) {
            let Some(e) = skill_trash_entry(&id) else { continue };
            let away = self.trash_dir.join(&id);
            if !fs::metadata(&away).is_ok_and(|m| m.is_dir()) && !fs::symlink_metadata(&away).is_ok_and(|m| m.file_type().is_symlink()) { continue; }
            let about = skill_about(&kuvo_store::files::text_of(&away.join("SKILL.md")).unwrap_or_default());
            let mut m = Map::from_iter([
                ("id".into(), json!(id)), ("folder".into(), json!(e.folder)),
                ("name".into(), json!(about.name.filter(|s| !s.is_empty()).unwrap_or_else(|| e.folder.clone()))),
                ("deleted".into(), json!(e.deleted)), ("goes".into(), json!(e.deleted + SKILL_TRASH_DAYS * 86_400)),
            ]);
            if let Some(d) = about.description.filter(|s| !s.is_empty()) { m.insert("description".into(), json!(d)); }
            entries.push((e.deleted, id, Value::Object(m)));
        }
        entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        entries.into_iter().map(|(_, _, v)| v).collect()
    }

    pub fn write_skill_package(&self, skill: &SkillPackage, managed: Option<&str>) -> Result<(), SkillFailure> {
        let ok = if managed.is_some() { is_item_id(&skill.name) && valid_skill_files(&skill.files) }
            else { is_skill_folder(&skill.name) && skill.files.iter().any(|f| f.path == "SKILL.md") };
        if !ok { return Err(SkillFailure::new(422, "That skill’s files could not be verified.")); }
        let target = self.skills_dir.join(&skill.name);
        let replacing = target.exists();
        if replacing && (managed.is_none() || kuvo_store::files::text_of(&target.join(MARKETPLACE_SKILL)).is_none()) {
            return Err(SkillFailure::new(409, format!("A skill named {} is already installed on this computer.", skill.name)));
        }
        let mut nonce = [0u8; 8];
        let _ = getrandom::fill(&mut nonce);
        let staging = self.skills_dir.join(format!(".{}.kuvo-new-{}", skill.name, hex::encode(nonce)));
        let prev = PathBuf::from(format!("{}-before", staging.display()));
        let res = (|| -> io::Result<()> {
            for f in &skill.files {
                let bytes = STANDARD.decode(&f.data).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                kuvo_store::files::write_atomic(&staging.join(&f.path), &bytes)?;
            }
            if let Some(src) = &skill.source {
                kuvo_store::files::write_atomic(&staging.join(SKILL_SOURCE_FILE), &serde_json::to_vec(src).map_err(io::Error::other)?)?;
            }
            if let Some(m) = managed { kuvo_store::files::write_atomic(&staging.join(MARKETPLACE_SKILL), m.as_bytes())?; }
            if replacing { fs::rename(&target, &prev)?; }
            if let Err(err) = fs::rename(&staging, &target) { if replacing { let _ = fs::rename(&prev, &target); } return Err(err); }
            if replacing { let _ = fs::remove_dir_all(&prev); }
            Ok(())
        })();
        let _ = fs::remove_dir_all(&staging);
        res.map_err(|e| SkillFailure::new(502, e.to_string()))
    }

    pub async fn engine_skills(&self) -> Vec<EngineSkill> {
        let Some(t) = self.engine.target() else { return Vec::new() };
        let _ = self.engine.call(&t, reqwest::Method::GET, "/api/provider", None).await;
        let Some(Value::Array(items)) = self.engine.call(&t, reqwest::Method::GET, "/api/skill", None).await else { return Vec::new() };
        items.into_iter().filter_map(|i| {
            let o = i.as_object()?;
            Some(EngineSkill { id: o.get("id")?.as_str()?.into(), path: o.get("path").and_then(Value::as_str).map(str::to_owned) })
        }).collect()
    }

    fn skill_place(&self, file: Option<&str>) -> String {
        let Some(f) = file.filter(|s| !s.is_empty()) else { return "elsewhere".into() };
        if f.starts_with("/builtin/") { return "built in".into(); }
        let dir = Path::new(f).parent().and_then(Path::parent).unwrap_or_else(|| Path::new(f));
        match dir.strip_prefix(&self.account_home) {
            Ok(rel) if rel.as_os_str().is_empty() => "in ~".into(),
            Ok(rel) => format!("in ~/{}", rel.to_string_lossy()),
            Err(_) => format!("in {}", dir.display()),
        }
    }

    pub fn start_trash_sweeper(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let s = self.clone();
        s.clear_skill_trash(now_secs());
        tokio::spawn(async move {
            let mut iv = tokio::time::interval(Duration::from_secs(86_400));
            iv.tick().await;
            loop {
                iv.tick().await;
                let s = s.clone();
                let _ = tokio::task::spawn_blocking(move || s.clear_skill_trash(now_secs())).await;
            }
        })
    }
}

pub fn now_secs() -> i64 { SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64) }
fn decode_param(raw: Option<&str>) -> Result<String, Refusal> {
    decode_uri_component(raw.unwrap_or("")).ok_or_else(|| Refusal::new(StatusCode::BAD_REQUEST, "That isn’t a path."))
}
fn check_guards(folder: &Path, file: &Path, body: &Value) -> Result<(), Refusal> {
    if kuvo_store::files::text_of(&folder.join(MARKETPLACE_SKILL)).is_some() {
        return Err(Refusal::new(StatusCode::CONFLICT, "Change this setup-wide skill in Marketplace."));
    }
    if let Some(was) = body.as_object().and_then(|o| o.get("was"))
        && kuvo_store::files::text_of(file).map(Value::String).unwrap_or(Value::Null) != *was
    { return Err(Refusal::new(StatusCode::CONFLICT, "It changed since you read it.")); }
    Ok(())
}

pub fn register(routes: &mut Registry, store: Arc<SkillStore>) {
    let s = store.clone();
    routes.register::<get_kuvo_skills::Spec, _, _>(Access::Any, move |_| {
        let s = s.clone();
        async move { Ok(RouteReply::json_as_kept::<200>(json!({ "folder": s.skills_dir.to_string_lossy(), "folders": s.skill_folders(), "github": s.skill_sources() }))) }
    });
    let s = store.clone();
    routes.register::<get_kuvo_skills_name::Spec, _, _>(Access::Any, move |call| {
        let (s, raw) = (s.clone(), call.param("name").map(str::to_owned));
        async move {
            let name = decode_param(raw.as_deref())?;
            if !is_skill_folder(&name) { return Err(Refusal::new(StatusCode::BAD_REQUEST, "That isn’t a skill’s name.")); }
            kuvo_store::files::text_of(&s.skills_dir.join(&name).join("SKILL.md"))
                .map(|text| RouteReply::json::<200>(get_kuvo_skills_name::Status200 { text }))
                .ok_or_else(|| Refusal::json(StatusCode::NOT_FOUND, json!({})))
        }
    });
    let s = store.clone();
    routes.register::<put_kuvo_skills_name::Spec, _, _>(Access::Any, move |call| {
        let (s, raw, bytes) = (s.clone(), call.param("name").map(str::to_owned), call.request.body.clone());
        async move {
            let name = decode_param(raw.as_deref())?;
            if !is_skill_folder(&name) { return Err(Refusal::new(StatusCode::BAD_REQUEST, "That isn’t a skill’s name.")); }
            let (folder, body) = (s.skills_dir.join(&name), serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null));
            let file = folder.join("SKILL.md");
            check_guards(&folder, &file, &body)?;
            let text = body.get("text").and_then(Value::as_str).filter(|t| !t.trim().is_empty())
                .ok_or_else(|| Refusal::new(StatusCode::BAD_REQUEST, r#"Send the skill's SKILL.md as {"text": …}."#))?;
            if body.get("new").and_then(Value::as_bool) == Some(true) {
                if skill_name(&name) != name { return Err(Refusal::new(StatusCode::BAD_REQUEST, "Names use lowercase letters a to z, digits and hyphens.")); }
                if folder.exists() || s.engine_skills().await.iter().any(|e| e.id == name) { return Err(Refusal::new(StatusCode::CONFLICT, format!("There’s a skill called {name} already."))); }
            } else if !folder.exists() { return Err(Refusal::json(StatusCode::NOT_FOUND, json!({}))); }
            else if s.skill_sources().contains_key(&name) { return Err(Refusal::new(StatusCode::CONFLICT, format!("{name} came from GitHub, so it isn’t edited here."))); }
            fs::create_dir_all(&folder).map_err(|e| Refusal::new(StatusCode::BAD_GATEWAY, e.to_string()))?;
            let tmp = PathBuf::from(format!("{}.new", file.display()));
            fs::write(&tmp, text).and_then(|()| fs::rename(&tmp, &file)).map_err(|e| Refusal::new(StatusCode::BAD_GATEWAY, e.to_string()))?;
            Ok(RouteReply::json::<200>(put_kuvo_skills_name::Status200 {}))
        }
    });
    let s = store.clone();
    routes.register::<delete_kuvo_skills_name::Spec, _, _>(Access::Any, move |call| {
        let (s, raw, bytes) = (s.clone(), call.param("name").map(str::to_owned), call.request.body.clone());
        async move {
            let name = decode_param(raw.as_deref())?;
            if !is_skill_folder(&name) { return Err(Refusal::new(StatusCode::BAD_REQUEST, "That isn’t a skill’s name.")); }
            let (folder, body) = (s.skills_dir.join(&name), serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null));
            check_guards(&folder, &folder.join("SKILL.md"), &body)?;
            if !folder.exists() { return Err(Refusal::json(StatusCode::NOT_FOUND, json!({}))); }
            fs::create_dir_all(&s.trash_dir).map_err(|e| Refusal::new(StatusCode::BAD_GATEWAY, e.to_string()))?;
            let mut when = now_secs();
            while s.trash_dir.join(format!("{name}-{when}")).exists() { when += 1; }
            fs::rename(&folder, s.trash_dir.join(format!("{name}-{when}"))).map_err(|e| Refusal::new(StatusCode::BAD_GATEWAY, e.to_string()))?;
            s.clear_skill_trash(now_secs());
            Ok(RouteReply::json::<200>(delete_kuvo_skills_name::Status200 {}))
        }
    });
    let s = store.clone();
    routes.register::<get_kuvo_skill_trash::Spec, _, _>(Access::Any, move |_| {
        let s = s.clone();
        async move { s.clear_skill_trash(now_secs()); Ok(RouteReply::json_as_kept::<200>(json!({ "entries": s.skill_trash_entries() }))) }
    });
    routes.register::<post_kuvo_skill_trash_id_restore::Spec, _, _>(Access::Any, move |call| {
        let (s, raw) = (store.clone(), call.param("id").map(str::to_owned));
        async move {
            s.clear_skill_trash(now_secs());
            let id = decode_param(raw.as_deref())?;
            let Some(parsed) = skill_trash_entry(&id) else { return Err(Refusal::new(StatusCode::BAD_REQUEST, "That isn’t an entry in Recently Deleted.")); };
            if s.trash_record_path(&id).exists() { return Err(Refusal::new(StatusCode::CONFLICT, "Restore this setup-wide skill in Marketplace.")); }
            let away = s.trash_dir.join(&id);
            if !fs::metadata(&away).is_ok_and(|m| m.is_dir()) { return Err(Refusal::json(StatusCode::NOT_FOUND, json!({}))); }
            let name = parsed.folder;
            let back = s.skills_dir.join(&name);
            if back.exists() { return Err(Refusal::new(StatusCode::CONFLICT, format!("There’s a skill called {name} now. Delete it first to restore this one."))); }
            let prefix = format!("{}{}", s.skills_dir.display(), std::path::MAIN_SEPARATOR);
            if let Some(other) = s.engine_skills().await.into_iter().find(|e| e.id == name && !e.path.as_deref().is_some_and(|p| p.starts_with(&prefix))) {
                return Err(Refusal::new(StatusCode::CONFLICT, format!("There’s a skill called {name} {}, so this one can’t be restored under that name.", s.skill_place(other.path.as_deref()))));
            }
            fs::create_dir_all(&s.skills_dir).and_then(|()| fs::rename(&away, &back)).map_err(|e| Refusal::new(StatusCode::BAD_GATEWAY, e.to_string()))?;
            Ok(RouteReply::json::<200>(post_kuvo_skill_trash_id_restore::Status200 { name }))
        }
    });
}
