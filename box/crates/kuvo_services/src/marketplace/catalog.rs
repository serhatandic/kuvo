//! Bundled and refreshed Marketplace catalog (`catalog/index.json` and `box/marketplace/catalog-v1.json`).

use std::{collections::HashSet, fs, path::{Path, PathBuf}, sync::{Mutex, OnceLock}, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::{
    config::in_test_folder,
    skills::{
        library::limited_bytes,
        names::{is_item_id, is_relative_file},
        store::{SKILL_FILE_LIMIT, SKILL_MAX_FILES, SkillFailure, SkillFile, sha256_hex},
    },
};

pub const CATALOG_URL: &str = "https://raw.githubusercontent.com/serhatandic/kuvo-catalog/main/index.json";
const MAX_CATALOG: usize = 2 * 1024 * 1024;
const BUNDLED_CATALOG: &str = include_str!("../../../../../catalog/index.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogApp { pub id: String, pub title: String, pub category: String, pub summary: String, pub url: String, pub auth: String, #[serde(default, skip_serializing_if = "Option::is_none")] pub logo: Option<String> }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSkill { pub id: String, pub title: String, pub category: String, pub summary: String, pub version: String, pub files: Vec<SkillFile> }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Routine { pub id: String, pub title: String, pub every: String, pub note: String }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestedModel { pub id: String, #[serde(rename = "providerID")] pub provider_id: String }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogAgent {
    pub id: String, pub title: String, pub category: String, pub summary: String, pub version: String, pub instructions: String,
    pub apps: Vec<String>, pub skills: Vec<String>, #[serde(rename = "suggestedModel")] pub suggested_model: SuggestedModel, pub routines: Vec<Routine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog { pub format: u32, pub apps: Vec<CatalogApp>, pub agents: Vec<CatalogAgent>, pub skills: Vec<CatalogSkill> }

pub fn is_endpoint(value: &str) -> bool {
    value.len() <= 1024 && !value.chars().any(|c| c.is_whitespace() || matches!(c, '{' | '}'))
        && Url::parse(value).is_ok_and(|u| u.scheme() == "https" && u.username().is_empty() && u.password().is_none() && u.query().is_none() && u.fragment().is_none())
}

fn non_empty(s: &str, max: usize) -> bool { !s.trim().is_empty() && s.len() <= max }
fn valid_id_63(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=63).contains(&b.len()) && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit()) && b.iter().all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}
fn valid_logo(raw: Option<&str>) -> bool {
    let Some(s) = raw.filter(|s| non_empty(s, 170_000)) else { return false };
    let Ok(b) = STANDARD.decode(s) else { return false };
    STANDARD.encode(&b) == s && b.len() >= 24 && &b[..8] == b"\x89PNG\r\n\x1a\n"
        && (1..=1024).contains(&u32::from_be_bytes([b[16], b[17], b[18], b[19]]))
        && (1..=1024).contains(&u32::from_be_bytes([b[20], b[21], b[22], b[23]]))
}

pub fn valid_catalog_skill_files(files: &[SkillFile]) -> bool {
    if files.is_empty() || files.len() > SKILL_MAX_FILES { return false; }
    let (mut total, mut paths) = (0usize, HashSet::new());
    for f in files {
        if !is_relative_file(&f.path) || f.path.contains(':') || f.data.len() > 90_000 { return false; }
        let Ok(bytes) = STANDARD.decode(&f.data) else { return false };
        total = total.saturating_add(bytes.len());
        if total > SKILL_FILE_LIMIT || STANDARD.encode(&bytes) != f.data || sha256_hex(&bytes) != f.sha256 || !paths.insert(f.path.as_str()) { return false; }
    }
    paths.contains("SKILL.md") && paths.iter().all(|f| { let p: Vec<&str> = f.split('/').collect(); (1..p.len()).all(|n| !paths.contains(p[..n].join("/").as_str())) })
}

fn distinct<'a>(mut iter: impl Iterator<Item = &'a str>) -> bool { let mut s = HashSet::new(); iter.all(|x| s.insert(x)) }

pub fn parse_catalog(raw: &Value) -> Result<Catalog, String> {
    if raw.get("format").and_then(Value::as_u64) != Some(1) { return Err("The catalog has an unsupported format.".into()); }
    let cat: Catalog = serde_json::from_value(raw.clone()).map_err(|_| "The catalog contains invalid items.".to_owned())?;
    if cat.apps.len() > 100 || cat.skills.len() > 100 || cat.agents.len() > 100 { return Err("The catalog contains invalid items.".into()); }
    let app_ids: HashSet<&str> = cat.apps.iter().map(|a| a.id.as_str()).collect();
    let skill_ids: HashSet<&str> = cat.skills.iter().map(|s| s.id.as_str()).collect();
    let ok = distinct(cat.apps.iter().map(|a| a.id.as_str())) && cat.apps.iter().all(|a| {
        valid_id_63(&a.id) && a.id.len() <= 52 && non_empty(&a.title, 100) && non_empty(&a.category, 50)
            && non_empty(&a.summary, 200) && is_endpoint(&a.url) && matches!(a.auth.as_str(), "open" | "oauth" | "token" | "broker") && valid_logo(a.logo.as_deref())
    }) && distinct(cat.skills.iter().map(|s| s.id.as_str())) && cat.skills.iter().all(|s| {
        valid_id_63(&s.id) && non_empty(&s.title, 100) && non_empty(&s.category, 50) && non_empty(&s.summary, 200) && non_empty(&s.version, 64) && valid_catalog_skill_files(&s.files)
    }) && distinct(cat.agents.iter().map(|a| a.id.as_str())) && cat.agents.iter().all(|a| {
        valid_id_63(&a.id) && non_empty(&a.title, 100) && non_empty(&a.category, 50) && non_empty(&a.summary, 200) && non_empty(&a.version, 64)
            && non_empty(&a.instructions, 8000) && non_empty(&a.suggested_model.id, 200) && non_empty(&a.suggested_model.provider_id, 80)
            && distinct(a.apps.iter().map(String::as_str)) && a.apps.iter().all(|id| is_item_id(id) && app_ids.contains(id.as_str()))
            && distinct(a.skills.iter().map(String::as_str)) && a.skills.iter().all(|id| is_item_id(id) && skill_ids.contains(id.as_str()))
            && a.routines.len() <= 10 && distinct(a.routines.iter().map(|r| r.id.as_str()))
            && a.routines.iter().all(|r| valid_id_63(&r.id) && non_empty(&r.title, 100) && non_empty(&r.note, 2000) && matches!(r.every.as_str(), "1d" | "1w"))
    });
    if !ok { return Err("The catalog contains invalid items.".into()); }
    Ok(cat)
}

pub fn bundled_catalog() -> &'static Catalog {
    static SNAPSHOT: OnceLock<Catalog> = OnceLock::new();
    SNAPSHOT.get_or_init(|| parse_catalog(&serde_json::from_str(BUNDLED_CATALOG).expect("bundled catalog/index.json")).expect("valid bundled catalog/index.json"))
}

pub struct CatalogStore { cache_file: PathBuf, refresh_url: String, cached: Mutex<(bool, Option<Catalog>)>, client: reqwest::Client }

impl CatalogStore {
    pub fn new(home: &Path, account_home: &Path) -> Self {
        let refresh_url = in_test_folder(home, account_home).then(|| std::env::var("KUVO_TEST_MARKETPLACE_CATALOG").ok()).flatten().unwrap_or_else(|| CATALOG_URL.into());
        Self {
            cache_file: home.join("box/marketplace/catalog-v1.json"), refresh_url, cached: Mutex::new((false, None)),
            client: reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(15)).build().unwrap_or_default(),
        }
    }

    pub fn catalog(&self) -> Catalog {
        if let Ok(mut g) = self.cached.lock() {
            if !g.0 {
                g.0 = true;
                if let Ok(t) = fs::read_to_string(&self.cache_file) && let Ok(v) = serde_json::from_str(&t) && let Ok(p) = parse_catalog(&v) { g.1 = Some(p); }
            }
            if let Some(cat) = &g.1 { return cat.clone(); }
        }
        bundled_catalog().clone()
    }
    pub fn offered_apps(&self) -> Vec<CatalogApp> { self.catalog().apps.into_iter().filter(|a| a.auth != "broker").collect() }
    pub fn offered_agents(&self) -> Vec<CatalogAgent> {
        let offered: HashSet<String> = self.offered_apps().into_iter().map(|a| a.id).collect();
        self.catalog().agents.into_iter().filter(|a| a.apps.iter().all(|id| offered.contains(id))).collect()
    }

    pub async fn refresh(&self) -> Result<bool, SkillFailure> {
        if self.refresh_url.is_empty() { return Ok(false); }
        let resp = self.client.get(&self.refresh_url).send().await.map_err(|e| SkillFailure::new(502, e.to_string()))?;
        if !resp.status().is_success() { return Err(SkillFailure::new(502, "The catalog download failed.")); }
        let bytes = limited_bytes(resp, MAX_CATALOG).await?;
        let next = parse_catalog(&serde_json::from_slice(&bytes).map_err(|e| SkillFailure::new(422, e.to_string()))?).map_err(|e| SkillFailure::new(422, e))?;
        kuvo_store::files::write_atomic(&self.cache_file, &bytes).map_err(|e| SkillFailure::new(502, e.to_string()))?;
        if let Ok(mut g) = self.cached.lock() { *g = (true, Some(next)); }
        Ok(true)
    }
}
