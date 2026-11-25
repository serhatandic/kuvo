//! Import Claude Code's Markdown agents without following a link into credentials.
use super::*;
use kuvo_store::files;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const MAX_FILE: u64 = 256 * 1024;
const MAX_TOTAL: usize = 4 * 1024 * 1024;
const MAX_ENTRIES: usize = 2000;
const REFUSED: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".config/gh",
    ".kuvo-code",
    "Library",
    ".local/share/keyrings",
    ".config/gcloud",
    ".azure",
    ".kube",
    ".docker/config.json",
    ".oci",
    ".config/doctl",
    ".terraform.d/credentials.tfrc.json",
    ".netrc",
    ".npmrc",
    ".yarnrc.yml",
    ".pypirc",
    ".cargo/credentials.toml",
    ".cargo/credentials",
    ".gem/credentials",
    ".git-credentials",
    ".password-store",
    ".config/op",
    ".config/hub",
    ".pgpass",
    ".my.cnf",
    ".android/adbkey",
    ".composer/auth.json",
    ".config/composer/auth.json",
    ".local/share/opencode/auth.json",
    ".config/stripe",
    ".wakatime.cfg",
    ".codex/auth.json",
    ".grok/auth.json",
    ".claude/.credentials.json",
    ".gemini/oauth_creds.json",
    ".config/google-chrome",
    ".config/chromium",
    ".config/BraveSoftware",
    ".mozilla",
];

fn unquote(text: &str) -> String {
    if let Some(s) = text.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        s.replace("\\\"", "\"")
    } else if let Some(s) = text.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        s.replace("''", "'")
    } else {
        text.into()
    }
}
fn parse(text: &str) -> (std::collections::BTreeMap<String, String>, String) {
    let mut fields = std::collections::BTreeMap::new();
    let Some((front, body)) = text.strip_prefix("---").and_then(|s| s.split_once("\n---")) else {
        return (fields, text.trim().into());
    };
    let mut lines = front.lines().peekable();
    while let Some(line) = lines.next() {
        if line.starts_with([' ', '\t']) {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        let value = if matches!(value, "|" | ">" | "|-" | ">-") {
            let mut block = Vec::new();
            while lines
                .peek()
                .is_some_and(|s| s.starts_with(' ') || s.is_empty())
            {
                block.push(lines.next().unwrap().trim());
            }
            block
                .join(if value.starts_with('|') { "\n" } else { " " })
                .trim()
                .to_owned()
        } else {
            value.into()
        };
        fields.insert(key.trim().into(), unquote(&value));
    }
    (
        fields,
        body.split_once('\n')
            .map(|(_, s)| s)
            .unwrap_or(body)
            .trim()
            .into(),
    )
}
fn real(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}
fn within(path: &Path, root: &Path) -> bool {
    // These systems conventionally use case-insensitive home paths.
    if cfg!(any(target_os = "macos", windows)) {
        PathBuf::from(path.to_string_lossy().to_lowercase())
            .starts_with(PathBuf::from(root.to_string_lossy().to_lowercase()))
    } else {
        path.starts_with(root)
    }
}
struct Places {
    roots: Vec<PathBuf>,
    refused: Vec<PathBuf>,
}
impl Places {
    fn new(account: &Path, home: &Path, claude: &Path) -> Self {
        let mut roots = vec![
            account.to_owned(),
            real(account),
            PathBuf::from("/nix/store"),
            real(Path::new("/nix/store")),
        ];
        if let Ok(target) = fs::canonicalize(claude)
            && !roots.iter().any(|p| within(&target, p))
        {
            roots.push(target);
        }
        let mut refused = Vec::new();
        for p in REFUSED
            .iter()
            .map(|p| account.join(p))
            .chain([home.to_owned(), home.join("integrations")])
        {
            refused.push(real(&p));
            refused.push(p);
        }
        Self { roots, refused }
    }
    fn admits(&self, path: &Path) -> bool {
        self.roots.iter().any(|r| within(path, r)) && !self.refused.iter().any(|r| within(path, r))
    }
}
fn read_file(path: &Path, places: &Places, left: usize) -> Option<String> {
    let target = fs::canonicalize(path).ok()?;
    if target.extension().and_then(|s| s.to_str()) != Some("md") || !places.admits(&target) {
        return None;
    }
    let mut file = files::open_payload(&target).ok()?;
    let stamp = files::opened_file_stamp(&file).ok()?;
    let len = file.metadata().ok()?.len();
    if len > MAX_FILE || len > left as u64 {
        return None;
    }
    if fs::canonicalize(path).ok()? != target || files::file_stamp(&target).ok().flatten()? != stamp
    {
        return None;
    }
    let mut bytes = Vec::with_capacity(len as usize);
    file.by_ref().take(len).read_to_end(&mut bytes).ok()?;
    if fs::canonicalize(path).ok()? != target || files::file_stamp(&target).ok().flatten()? != stamp
    {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}
fn list(account: &Path, home: &Path, settings: &Value) -> Value {
    let claude = account.join(".claude");
    let places = Places::new(account, home, &claude);
    let folder = real(&claude.join("agents"));
    if !places.admits(&folder) {
        return json!({"agents":[],"commands":[]});
    }
    let mut entries = fs::read_dir(&folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
        .map(|e| e.path())
        .collect::<Vec<_>>();
    entries.sort();
    let mut left = MAX_TOTAL;
    let mut agents = Vec::new();
    for path in entries.into_iter().take(MAX_ENTRIES) {
        let Some(text) = read_file(&path, &places, left) else {
            continue;
        };
        left = left.saturating_sub(text.len());
        let (fields, system) = parse(&text);
        let name = fields
            .get("name")
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| {
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
        let mut id = name
            .to_lowercase()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || matches!(c, '_' | '-') {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>();
        if id.is_empty() {
            continue;
        }
        if [
            "build",
            "plan",
            "general",
            "explore",
            "compaction",
            "title",
            "summary",
        ]
        .contains(&id.as_str())
        {
            id.push_str("-claude");
        }
        let description = fields
            .get("description")
            .map(|d| d.replace("\\n", "\n"))
            .unwrap_or_default();
        agents.push(json!({"name":name,"description":description,"path":["agents",id],"settings":{"mode":"subagent","description":description,"system":system},"imported":settings["agents"].get(&id).is_some()}));
    }
    agents.sort_by_key(|a| a["name"].as_str().unwrap_or_default().to_lowercase());
    json!({"agents":agents,"commands":[]})
}

pub fn register(routes: &mut Registry, home: PathBuf, is_mac: bool) {
    routes.register::<get_kuvo_claude_code::Spec, _, _>(Access::Any, move |call| {
        let home = home.clone();
        async move {
            let query = form_urlencoded::parse(
                call.request
                    .path
                    .split_once('?')
                    .map(|(_, q)| q)
                    .unwrap_or_default()
                    .as_bytes(),
            );
            if query.into_iter().any(|(k, _)| k == "machine") {
                return Err(missing("This computer lists its own Claude Code only."));
            }
            let page = tokio::task::spawn_blocking(move || {
                let account = kuvo_os::platform::Account::current().and_then(|a| {
                    a.home_folder(
                        kuvo_os::platform::Platform::current(),
                        &std::env::vars_os().collect(),
                    )
                });
                let config = crate::config::ConfigPaths::for_home(&home, is_mac);
                let settings = match config.read_settings() {
                    crate::config::SettingsRead::Ok(v) => Value::Object(v),
                    _ => json!({}),
                };
                account.map(|account| list(&account, &home, &settings))
            })
            .await
            .map_err(|e| refused(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .map_err(|e| refused(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            Ok(typed(page))
        }
    });
}
