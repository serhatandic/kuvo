//! Visit journal and history suggestions, in the released browser's disk format.
use base64::{Engine, engine::general_purpose::STANDARD};
use http::StatusCode;
use kuvo_protocol::routes::{
    delete_kuvo_browser_history as clear, delete_kuvo_browser_history_host as forget,
    get_kuvo_browser_history as get, get_kuvo_browser_icons_host as icon,
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use kuvo_store::{files, locks::OwnedLock};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

const HISTORY_LIMIT: usize = 300;
const ICON_LIMIT: usize = 128 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Site {
    host: String,
    url: String,
    title: String,
    #[serde(serialize_with = "number")]
    visits: f64,
    #[serde(serialize_with = "number")]
    last_visit: f64,
}
#[derive(Serialize, Deserialize, Default)]
struct History {
    sites: Vec<Site>,
}
#[derive(Deserialize)]
struct Visit {
    url: String,
    title: Option<String>,
    at: Option<f64>,
    icon: Option<String>,
}
#[derive(Serialize)]
struct Suggestion {
    #[serde(flatten)]
    site: Site,
    icon: bool,
}

// JavaScript stores whole timestamps and counts without a decimal point.
fn number<S: serde::Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
    if value.fract() == 0.0 && *value >= i64::MIN as f64 && *value < i64::MAX as f64 {
        serializer.serialize_i64(*value as i64)
    } else {
        serializer.serialize_f64(*value)
    }
}

fn site_key(url: &url::Url) -> Option<String> {
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?;
    let mut host = host.strip_prefix("www.").unwrap_or(host).to_owned();
    if let Some(port) = url.port() {
        host.push_str(&format!(":{port}"));
    }
    Some(host)
}
fn icon_name(name: &str) -> bool {
    let (host, port) = name
        .split_once(':')
        .map_or((name, None), |(h, p)| (h, Some(p)));
    let valid = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    !host.is_empty()
        && host.len() <= 253
        && !host.contains("..")
        && valid(host.as_bytes()[0])
        && host.bytes().all(|b| valid(b) || b".-".contains(&b))
        && port
            .is_none_or(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()))
}
fn image_type(bytes: &[u8]) -> Option<&'static str> {
    Some(match bytes {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0, 0, 1, 0, ..] => "image/x-icon",
        [0xff, 0xd8, 0xff, ..] => "image/jpeg",
        [b'G', b'I', b'F', b'8', ..] => "image/gif",
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => "image/webp",
        _ => return None,
    })
}
fn now() -> f64 {
    jiff::Timestamp::now().as_millisecond() as f64
}
fn trim_to(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}
fn visit(history: &mut History, note: Visit, folder: &Path) {
    let Ok(url) = url::Url::parse(&note.url) else {
        return;
    };
    let Some(host) = site_key(&url) else {
        return;
    };
    if let Some(icon) = note.icon {
        if icon_name(&host)
            && icon.len() <= ICON_LIMIT * 2
            && let Ok(bytes) = STANDARD.decode(icon)
            && bytes.len() <= ICON_LIMIT
            && image_type(&bytes).is_some()
        {
            let _ = files::write_secret(&folder.join("icons").join(host), &bytes);
        }
        return;
    }
    let title = note.title.as_deref().unwrap_or_default().trim();
    let title = if title == note.url {
        String::new()
    } else {
        trim_to(title, 300)
    };
    let address = trim_to(&note.url, 2000);
    let at = note.at.unwrap_or_else(now);
    if let Some(site) = history.sites.iter_mut().find(|s| s.host == host) {
        site.visits += 1.0;
        site.last_visit = site.last_visit.max(at);
        if url.path().len() <= 1 || site.title.is_empty() {
            site.url = address;
            site.title = title;
        }
    } else {
        history.sites.push(Site {
            host,
            url: address,
            title,
            visits: 1.0,
            last_visit: at,
        });
        if history.sites.len() > HISTORY_LIMIT {
            history
                .sites
                .sort_by(|a, b| b.last_visit.total_cmp(&a.last_visit));
            history.sites.truncate(HISTORY_LIMIT);
        }
    }
}
fn save(folder: &Path, history: &History) -> io::Result<()> {
    files::write_secret(
        &folder.join("history.json"),
        &serde_json::to_vec(history).map_err(io::Error::other)?,
    )
}
fn journals(folder: &Path) -> Vec<PathBuf> {
    let mut files: Vec<_> = files::list_of(folder)
        .into_iter()
        .filter(|n| n.to_string_lossy().starts_with("visits.jsonl.folding-"))
        .map(|n| folder.join(n))
        .collect();
    files.sort();
    files
}
fn fold(folder: &Path) -> io::Result<History> {
    let mut history = History::default();
    if let Some(bytes) = files::read(&folder.join("history.json"))? {
        // Keep well-formed entries even if another entry wasn't readable.
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            history.sites = value["sites"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|site| serde_json::from_value(site.clone()).ok())
                .collect();
        }
    }
    let taking = folder.join(format!(
        "visits.jsonl.folding-{}-{}",
        std::process::id(),
        now() as i64
    ));
    let _ = fs::rename(folder.join("visits.jsonl"), taking);
    let taken = journals(folder);
    if taken.is_empty() {
        return Ok(history);
    }
    for file in &taken {
        if let Ok(text) = fs::read_to_string(file) {
            for line in text.lines() {
                if let Ok(note) = serde_json::from_str(line) {
                    visit(&mut history, note, folder);
                }
            }
        }
    }
    save(folder, &history)?;
    for file in taken {
        files::remove_file_synced(&file)?;
    }
    Ok(history)
}
#[derive(Default)]
enum Action {
    #[default]
    Read,
    Forget(String),
    Clear,
}
fn history(folder: &Path, action: Action) -> io::Result<Vec<Suggestion>> {
    if !folder.exists() {
        return Ok(vec![]);
    }
    let _lock = OwnedLock::acquire(&folder.join(".history.lock"), Duration::from_secs(3))?;
    if matches!(action, Action::Clear) {
        for file in journals(folder) {
            files::remove_file_synced(&file)?;
        }
        files::remove_file_synced(&folder.join("visits.jsonl"))?;
        files::remove_file_synced(&folder.join("history.json"))?;
        files::remove_tree_synced(&folder.join("icons"))?;
        return Ok(vec![]);
    }
    let mut history = fold(folder)?;
    if let Action::Forget(host) = action {
        history.sites.retain(|s| s.host != host);
        save(folder, &history)?;
        if icon_name(&host) {
            files::remove_file_synced(&folder.join("icons").join(host))?;
        }
    }
    let at = now();
    let score = |s: &Site| s.visits / (1.0 + (at - s.last_visit) / 86_400_000.0 / 14.0);
    history.sites.sort_by(|a, b| {
        score(b)
            .total_cmp(&score(a))
            .then_with(|| b.last_visit.total_cmp(&a.last_visit))
    });
    let icons = files::list_of(&folder.join("icons"));
    Ok(history
        .sites
        .into_iter()
        .map(|site| Suggestion {
            icon: icons.iter().any(|name| name == site.host.as_str()),
            site,
        })
        .collect())
}
fn failed(error: impl std::fmt::Display) -> Refusal {
    Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}
async fn answer(folder: PathBuf, action: Action) -> Result<serde_json::Value, Refusal> {
    let sites = tokio::task::spawn_blocking(move || history(&folder, action))
        .await
        .map_err(failed)?
        .map_err(failed)?;
    Ok(serde_json::json!({"sites":sites}))
}
fn host(value: &str) -> Option<String> {
    let decoded = percent_encoding::percent_decode_str(value)
        .decode_utf8()
        .ok()?;
    (!decoded.is_empty()).then(|| decoded.into_owned())
}
pub fn register(routes: &mut Registry, home: &Path) {
    let folder = home.join("browser");
    let read = folder.clone();
    routes.register::<get::Spec, _, _>(Access::Any, move |_| {
        let read = read.clone();
        async move {
            Ok(RouteReply::json_as_kept::<200>(
                answer(read, Action::Read).await?,
            ))
        }
    });
    let cleared = folder.clone();
    routes.register::<clear::Spec, _, _>(Access::Any, move |_| {
        let cleared = cleared.clone();
        async move {
            Ok(RouteReply::json_as_kept::<200>(
                answer(cleared, Action::Clear).await?,
            ))
        }
    });
    let removed = folder.clone();
    routes.register::<forget::Spec, _, _>(Access::Any, move |call| {
        let removed = removed.clone();
        let host = host(call.param("host").unwrap_or_default());
        async move {
            let host = host.ok_or_else(|| {
                Refusal::new(
                    StatusCode::BAD_REQUEST,
                    "Name the site as its address has it.",
                )
            })?;
            Ok(RouteReply::json_as_kept::<200>(
                answer(removed, Action::Forget(host)).await?,
            ))
        }
    });
    routes.register::<icon::Spec, _, _>(Access::Any, move |call| {
        let folder = folder.clone();
        let host = host(call.param("host").unwrap_or_default());
        async move {
            let not_found =
                || Refusal::new(StatusCode::NOT_FOUND, "There's no icon for that site here.");
            let host = host.filter(|host| icon_name(host)).ok_or_else(not_found)?;
            let bytes = tokio::fs::read(folder.join("icons").join(host))
                .await
                .map_err(|_| not_found())?;
            let media = image_type(&bytes).ok_or_else(not_found)?;
            Ok(RouteReply::bytes::<200>(media, bytes))
        }
    });
}
