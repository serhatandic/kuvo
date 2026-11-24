//! GitHub skill collections (`kuvo/skill-library*`) and commit-pinned GitHub skill fetching.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use futures_util::StreamExt;
use http::StatusCode;
use kuvo_protocol::routes::{
    get_kuvo_skill_library, get_kuvo_skill_library_owner_name,
    get_kuvo_skill_library_owner_name_about, post_kuvo_skill_library_install,
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use url::Url;

use crate::config::in_test_folder;

use super::{
    names::{SkillAbout, decode_uri_component, is_github_folder, is_relative_file, skill_about},
    store::{SKILL_FILE_LIMIT, SkillFailure, SkillPackage, SkillSource, SkillStore, is_commit_id, is_repo_id, named_package, pack_file},
};

const SEG: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');
const WELL_KNOWN: &[(&str, &str, &str, &str)] = &[
    ("anthropics", "skills", "Anthropic", "The reference set: documents, spreadsheets, slides, PDFs, design and more."),
    ("openai", "skills", "OpenAI", "Codex's catalog of curated skills."),
    ("obra", "superpowers", "Superpowers", "Ways of working: test-driven development, debugging, planning, code review."),
    ("mattpocock", "skills", "Matt Pocock", "Engineering and productivity skills, TypeScript-minded."),
    ("addyosmani", "agent-skills", "Addy Osmani", "Production engineering practices: performance, accessibility, testing."),
    ("vercel-labs", "agent-skills", "Vercel", "React, Next.js and deploying to Vercel."),
    ("github", "awesome-copilot", "GitHub Awesome Copilot", "Hundreds of community skills; quality varies."),
    ("huggingface", "skills", "Hugging Face", "Models, datasets and training with Hugging Face."),
    ("cloudflare", "skills", "Cloudflare", "Workers, Pages and the rest of Cloudflare's platform."),
    ("trailofbits", "skills", "Trail of Bits", "Security auditing and code review."),
    ("K-Dense-AI", "scientific-agent-skills", "Scientific Skills", "A large library for science: biology, chemistry, data analysis."),
    ("wshobson", "agents", "wshobson/agents", "A big set of plugins with skills for many languages and tools."),
];

#[derive(Debug, Clone, Serialize)]
pub struct SkillRepository {
    pub id: String, pub owner: String, pub name: String, pub title: String, pub summary: String,
    #[serde(skip_serializing)] pub branch: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepositorySkill { pub name: String, pub path: String, pub files: Vec<String> }

#[derive(Deserialize)]
struct GitHubTree { #[serde(default)] tree: Vec<GitHubTreeEntry>, #[serde(default)] truncated: bool }
#[derive(Deserialize)]
struct GitHubTreeEntry { #[serde(rename = "type")] kind: Option<String>, mode: Option<String>, path: String }

pub struct SkillLibrary {
    api: String, raw: String, client: reqwest::Client,
    looked_up: Mutex<HashMap<String, (SkillRepository, Instant)>>,
    abouts: Mutex<HashMap<String, SkillAbout>>,
    about_sem: Arc<Semaphore>,
}

impl SkillLibrary {
    pub fn for_store(store: &SkillStore) -> Self {
        let test = in_test_folder(&store.home, &store.account_home)
            .then(|| std::env::var("KUVO_TEST_GITHUB").ok().filter(|v| !v.is_empty())).flatten();
        let (api, raw) = test.map_or_else(|| ("https://api.github.com".into(), "https://raw.githubusercontent.com".into()), |b| (b.clone(), b));
        Self {
            api, raw, client: reqwest::Client::builder().timeout(Duration::from_secs(30)).build().unwrap_or_default(),
            looked_up: Mutex::new(HashMap::new()), abouts: Mutex::new(HashMap::new()), about_sem: Arc::new(Semaphore::new(16)),
        }
    }

    pub fn repositories() -> Vec<SkillRepository> {
        WELL_KNOWN.iter().map(|&(owner, name, title, summary)| SkillRepository {
            id: format!("{owner}/{name}"), owner: owner.into(), name: name.into(), title: title.into(), summary: summary.into(), branch: "main".into(),
        }).collect()
    }

    async fn github(&self, url: &str, json_accept: bool) -> Result<reqwest::Response, SkillFailure> {
        let mut req = self.client.get(url).header("user-agent", "Kuvo Code");
        if json_accept { req = req.header("accept", "application/vnd.github+json"); }
        let resp = req.send().await.map_err(|e| SkillFailure::new(502, e.to_string()))?;
        if matches!(resp.status().as_u16(), 403 | 429) { return Err(SkillFailure::new(503, "GitHub is limiting requests for now. Try again in a while.")); }
        Ok(resp)
    }

    fn raw_url(&self, repo: &SkillRepository, file: &str) -> String {
        let f = file.split('/').map(|s| utf8_percent_encode(s, SEG).to_string()).collect::<Vec<_>>().join("/");
        format!("{}/{}/{}/{}/{f}", self.raw, repo.owner, repo.name, utf8_percent_encode(&repo.branch, SEG))
    }

    pub async fn repository(&self, owner: &str, name: &str) -> Result<SkillRepository, SkillFailure> {
        if let Some(known) = Self::repositories().into_iter().find(|r| r.owner.eq_ignore_ascii_case(owner) && r.name.eq_ignore_ascii_case(name)) {
            return Ok(known);
        }
        let key = format!("{owner}/{name}").to_ascii_lowercase();
        if let Some((repo, at)) = self.looked_up.lock().ok().and_then(|m| m.get(&key).cloned()) && at.elapsed() < Duration::from_secs(900) {
            return Ok(repo);
        }
        let resp = self.github(&format!("{}/repos/{}/{}", self.api, utf8_percent_encode(owner, SEG), utf8_percent_encode(name, SEG)), true).await?;
        let info = (resp.status() == StatusCode::OK).then(|| resp.json::<Value>());
        let info = match info { Some(f) => f.await.ok(), None => None };
        let Some(branch) = info.as_ref().and_then(|v| v.get("default_branch")?.as_str()) else {
            return Err(SkillFailure::new(404, format!("{owner}/{name} wasn’t found on GitHub.")));
        };
        let repo = SkillRepository {
            id: format!("{owner}/{name}"), owner: owner.into(), name: name.into(), title: format!("{owner}/{name}"),
            summary: info.as_ref().and_then(|v| v.get("description")?.as_str()).unwrap_or("").into(), branch: branch.into(),
        };
        if let Ok(mut map) = self.looked_up.lock() { if map.len() > 50 { map.clear(); } map.insert(key, (repo.clone(), Instant::now())); }
        Ok(repo)
    }

    pub async fn skills_in(&self, repo: &SkillRepository) -> Result<(Vec<RepositorySkill>, bool), SkillFailure> {
        let resp = self.github(&format!("{}/repos/{}/{}/git/trees/{}?recursive=1", self.api, repo.owner, repo.name, utf8_percent_encode(&repo.branch, SEG)), true).await?;
        if resp.status() != StatusCode::OK { return Err(SkillFailure::new(404, format!("{} wasn’t found on GitHub.", repo.id))); }
        let tree: GitHubTree = resp.json().await.map_err(|e| SkillFailure::new(502, e.to_string()))?;
        let files: Vec<String> = tree.tree.into_iter().filter(|e| e.kind.as_deref() == Some("blob") && e.mode.as_deref() != Some("120000")).map(|e| e.path).collect();
        fn dirname(f: &str) -> &str { f.rfind('/').map_or("", |i| &f[..i]) }
        let folders: HashSet<String> = files.iter().filter(|f| f.rsplit('/').next() == Some("SKILL.md")).map(|f| dirname(f).into()).filter(|d: &String| !d.is_empty()).collect();
        let is_root = files.iter().any(|f| f == "SKILL.md");
        let mut by_folder: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for f in &files {
            let mut dir = dirname(f);
            while !dir.is_empty() && !folders.contains(dir) { dir = dirname(dir); }
            if !dir.is_empty() { by_folder.entry(dir.into()).or_default().push(f.clone()); }
            else if is_root && !f.starts_with('.') { by_folder.entry(String::new()).or_default().push(f.clone()); }
        }
        let mut sorted: Vec<String> = folders.into_iter().collect();
        sorted.sort();
        if is_root { sorted.insert(0, String::new()); }
        let skills: Vec<RepositorySkill> = sorted.into_iter().map(|folder| {
            let name = if folder.is_empty() { repo.name.clone() } else { folder.rsplit('/').next().unwrap_or(&folder).into() };
            RepositorySkill { name, files: by_folder.remove(&folder).unwrap_or_default(), path: folder }
        }).collect();
        let is_hidden = |s: &RepositorySkill| s.path.split('/').any(|p| p.starts_with('.'));
        let shown: HashSet<String> = skills.iter().filter(|s| !is_hidden(s)).map(|s| s.name.clone()).collect();
        Ok((skills.into_iter().filter(|s| !(is_hidden(s) && shown.contains(&s.name))).collect(), tree.truncated))
    }

    pub async fn about_skill(&self, repo: &SkillRepository, folder: &str) -> SkillAbout {
        let key = format!("{}@{}:{folder}", repo.id.to_ascii_lowercase(), repo.branch);
        if let Some(hit) = self.abouts.lock().ok().and_then(|m| m.get(&key).cloned()) { return hit; }
        let _permit = self.about_sem.acquire().await.ok();
        if let Some(hit) = self.abouts.lock().ok().and_then(|m| m.get(&key).cloned()) { return hit; }
        let file = if folder.is_empty() { "SKILL.md".into() } else { format!("{folder}/SKILL.md") };
        let Ok(resp) = self.github(&self.raw_url(repo, &file), false).await else { return SkillAbout::default() };
        if resp.status() != StatusCode::OK { return SkillAbout::default(); }
        let Ok(text) = resp.text().await else { return SkillAbout::default() };
        let about = skill_about(&text);
        if let Ok(mut map) = self.abouts.lock() { if map.len() > 5_000 { map.clear(); } map.insert(key, about.clone()); }
        about
    }

    pub async fn github_skill(&self, given: &Value, legacy: bool) -> Result<SkillPackage, SkillFailure> {
        let mut repo = given.get("repository").and_then(Value::as_str).map(str::to_owned);
        let mut folder = given.get("path").and_then(Value::as_str).map(str::to_owned).or_else(|| given.get("path").is_none().then(String::new));
        let mut r#ref = given.get("ref").and_then(Value::as_str).map(str::to_owned);
        let bad_ref = given.get("ref").is_some_and(|v| v.as_str().is_none_or(|s| s.is_empty() || s.len() > 240));
        if let Some(link) = given.get("link").and_then(Value::as_str) {
            let bad = || SkillFailure::new(400, "Give a GitHub repository or a link to a skill folder or SKILL.md.");
            let u = Url::parse(link).map_err(|_| bad())?;
            if u.scheme() != "https" || u.host_str() != Some("github.com") || u.port().is_some() || !u.username().is_empty() || u.password().is_some() { return Err(bad()); }
            let parts: Vec<String> = u.path().trim_matches('/').split('/').map(|p| decode_uri_component(p).ok_or_else(bad)).collect::<Result<_, _>>()?;
            if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() { return Err(bad()); }
            repo = Some(format!("{}/{}", parts[0], parts[1].strip_suffix(".git").unwrap_or(&parts[1])));
            if let Some(kind) = parts.get(2) {
                let ver = parts.get(3).filter(|v| !v.is_empty()).ok_or_else(bad)?;
                if !matches!(kind.as_str(), "tree" | "blob") { return Err(bad()); }
                r#ref = Some(ver.clone());
                let mut rest = parts[4..].to_vec();
                if kind == "blob" && rest.pop().as_deref() != Some("SKILL.md") { return Err(bad()); }
                folder = Some(rest.join("/"));
            }
        }
        let (Some(repo_str), Some(folder_str)) = (repo, folder) else { return Err(SkillFailure::new(400, "Give a GitHub repository, a skill folder, and optionally a branch, tag or commit.")); };
        if !is_repo_id(&repo_str) || !is_github_folder(&folder_str) || bad_ref {
            return Err(SkillFailure::new(400, "Give a GitHub repository, a skill folder, and optionally a branch, tag or commit."));
        }
        let (owner, name) = repo_str.split_once('/').unwrap();
        let known = self.repository(owner, name).await?;
        let resp = self.github(&format!("{}/repos/{}/commits/{}", self.api, known.id, utf8_percent_encode(r#ref.as_deref().unwrap_or(&known.branch), SEG)), true).await?;
        let commit = if resp.status().is_success() { resp.json::<Value>().await.ok().and_then(|v| v.get("sha")?.as_str().filter(|s| is_commit_id(s)).map(str::to_owned)) } else { None };
        let Some(commit) = commit else { return Err(SkillFailure::new(404, "GitHub could not resolve that version.")); };
        let pinned = SkillRepository { branch: commit.clone(), ..known };
        let (skills, partial) = self.skills_in(&pinned).await?;
        if partial && !legacy { return Err(SkillFailure::new(422, "GitHub returned an incomplete file list. Give a smaller repository or install from a folder.")); }
        let Some(entry) = skills.into_iter().find(|s| s.path == folder_str) else { return Err(SkillFailure::new(404, "There is no SKILL.md in that folder.")); };
        let (mut files, mut left) = (Vec::new(), if legacy { 20 * 1024 * 1024 } else { SKILL_FILE_LIMIT });
        for file in &entry.files {
            let rel = if folder_str.is_empty() { file.as_str() } else { &file[folder_str.len() + 1..] };
            if !is_relative_file(rel) || (!legacy && files.len() >= 100) { return Err(SkillFailure::new(422, "The skill has an unsupported path or too many files.")); }
            let resp = self.github(&self.raw_url(&pinned, file), false).await?;
            if !resp.status().is_success() { return Err(SkillFailure::new(404, format!("{rel} couldn’t be read at that commit."))); }
            let bytes = limited_bytes(resp, left).await?;
            left = left.saturating_sub(bytes.len());
            files.push(pack_file(rel, &bytes));
        }
        let mut pkg = if legacy { SkillPackage { name: entry.name, files, source: None } } else { named_package(&entry.name, files)? };
        pkg.source = Some(SkillSource { repository: repo_str, path: folder_str, commit, r#ref });
        Ok(pkg)
    }
}

pub async fn limited_bytes(resp: reqwest::Response, limit: usize) -> Result<Vec<u8>, SkillFailure> {
    if resp.content_length().is_some_and(|n| n as usize > limit) { return Err(SkillFailure::new(422, "Response exceeded size limit.")); }
    let (mut out, mut stream) = (Vec::new(), resp.bytes_stream());
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| SkillFailure::new(502, e.to_string()))?;
        if out.len().saturating_add(chunk.len()) > limit { return Err(SkillFailure::new(422, "Response exceeded size limit.")); }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

pub fn register(routes: &mut Registry, store: Arc<SkillStore>, library: Arc<SkillLibrary>) {
    routes.register::<get_kuvo_skill_library::Spec, _, _>(Access::Any, |_| async {
        Ok(RouteReply::json_as_kept::<200>(json!({ "repositories": SkillLibrary::repositories() })))
    });
    let (st, lib) = (store, library.clone());
    routes.register::<post_kuvo_skill_library_install::Spec, _, _>(Access::Any, move |call| {
        let (st, lib, body) = (st.clone(), lib.clone(), call.request.body.clone());
        async move {
            let skill = lib.github_skill(&serde_json::from_slice(&body).unwrap_or(json!({})), true).await.map_err(SkillFailure::into_refusal)?;
            st.write_skill_package(&skill, None).map_err(SkillFailure::into_refusal)?;
            Ok(RouteReply::json::<200>(post_kuvo_skill_library_install::Status200 { name: skill.name }))
        }
    });
    let lib = library.clone();
    routes.register::<get_kuvo_skill_library_owner_name::Spec, _, _>(Access::Any, move |call| {
        let (lib, o, n) = (lib.clone(), call.param("owner").map(str::to_owned), call.param("name").map(str::to_owned));
        async move {
            let bad = || Refusal::new(StatusCode::BAD_REQUEST, "That isn’t a path.");
            let (owner, name) = (decode_uri_component(o.as_deref().unwrap_or("")).ok_or_else(bad)?, decode_uri_component(n.as_deref().unwrap_or("")).ok_or_else(bad)?);
            let repo = lib.repository(&owner, &name).await.map_err(SkillFailure::into_refusal)?;
            let (skills, partial) = lib.skills_in(&repo).await.map_err(SkillFailure::into_refusal)?;
            let described = futures_util::future::join_all(skills.into_iter().map(|s| {
                let (lib, repo) = (lib.clone(), repo.clone());
                async move { json!({ "name": s.name, "path": s.path, "files": s.files, "about": lib.about_skill(&repo, &s.path).await }) }
            })).await;
            Ok(RouteReply::json_as_kept::<200>(json!({ "repository": repo, "skills": described, "partial": partial })))
        }
    });
    routes.register::<get_kuvo_skill_library_owner_name_about::Spec, _, _>(Access::Any, move |call| {
        let (lib, o, n, path) = (library.clone(), call.param("owner").map(str::to_owned), call.param("name").map(str::to_owned), call.request.path.clone());
        async move {
            let bad = || Refusal::new(StatusCode::BAD_REQUEST, "That isn’t a path.");
            let (owner, name) = (decode_uri_component(o.as_deref().unwrap_or("")).ok_or_else(bad)?, decode_uri_component(n.as_deref().unwrap_or("")).ok_or_else(bad)?);
            let at = match path.split_once('?').and_then(|(_, q)| q.split('&').find_map(|p| p.strip_prefix("path="))) {
                Some(raw) => decode_uri_component(raw).ok_or_else(bad)?,
                None => String::new(),
            };
            let repo = lib.repository(&owner, &name).await.map_err(SkillFailure::into_refusal)?;
            let about = lib.about_skill(&repo, &at).await;
            Ok(RouteReply::json::<200>(get_kuvo_skill_library_owner_name_about::Status200 { name: about.name, description: about.description }))
        }
    });
}
