//! Site metadata and explicit site removal. Values of cookies never enter an
//! answer: only their domain, name, expiry and storage kinds are needed here.
use super::{Browser, Living};
use http::StatusCode;
use kuvo_protocol::routes::{
    delete_kuvo_browser_sites as all, delete_kuvo_browser_sites_site as one,
    get_kuvo_browser_sites as get,
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use kuvo_store::{files, locks::OwnedLock};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

const STORAGE: &[&str] = &[
    "Local Storage",
    "Session Storage",
    "IndexedDB",
    "Service Worker",
    "Cache",
    "Code Cache",
    "WebStorage",
    "File System",
    "databases",
    "Shared Storage",
];
struct Cookie {
    domain: String,
    name: String,
    path: String,
    expires: Option<f64>,
    persistent: bool,
    partition: Option<Value>,
}
fn site_of(host: &str) -> String {
    let name = host.strip_prefix('.').unwrap_or(host).to_lowercase();
    if !name.contains('.')
        || name.contains(':')
        || name.bytes().all(|b| b.is_ascii_digit() || b == b'.')
    {
        return name;
    }
    let labels: Vec<_> = name.split('.').collect();
    let second = labels[labels.len() - 2];
    let n = if labels.len() >= 3
        && labels.last().is_some_and(|t| t.len() == 2)
        && [
            "co", "com", "net", "org", "gov", "edu", "ac", "or", "ne", "go", "gob", "mil", "nic",
        ]
        .contains(&second)
    {
        3
    } else {
        2
    };
    labels[labels.len() - n..].join(".")
}
fn on_site(host: &str, site: &str) -> bool {
    let host = host.strip_prefix('.').unwrap_or(host).to_lowercase();
    host == site
        || host
            .strip_suffix(site)
            .is_some_and(|prefix| prefix.ends_with('.'))
}
fn cookie_file(folder: &Path) -> Option<PathBuf> {
    [
        folder.join("profile/Default/Network/Cookies"),
        folder.join("profile/Default/Cookies"),
    ]
    .into_iter()
    .filter_map(|file| Some((fs::metadata(&file).ok()?.modified().ok()?, file)))
    .max_by_key(|(at, _)| *at)
    .map(|(_, file)| file)
}
fn database(path: &Path, read_only: bool) -> rusqlite::Result<Connection> {
    let db = Connection::open_with_flags(
        path,
        if read_only {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        },
    )?;
    db.busy_timeout(Duration::from_millis(500))?;
    Ok(db)
}
fn file_cookies(folder: &Path) -> Option<Vec<Cookie>> {
    let file = cookie_file(folder)?;
    let db = database(&file, true).ok()?;
    let mut statement = db
        .prepare(
            "SELECT host_key, name, path, expires_utc, has_expires, is_persistent FROM cookies",
        )
        .ok()?;
    let rows = statement
        .query_map([], |r| {
            let expires = r.get::<_, i64>(3)?;
            Ok(Cookie {
                domain: r.get(0)?,
                name: r.get(1)?,
                path: r.get(2)?,
                expires: (r.get::<_, i64>(4)? != 0 && expires > 0)
                    .then_some(expires as f64 / 1000.0 - 11_644_473_600_000.0),
                persistent: r.get::<_, i64>(5)? == 1,
                partition: None,
            })
        })
        .ok()?;
    rows.collect::<Result<Vec<_>, _>>().ok()
}
async fn live_cookies(live: &Living) -> Option<Vec<Cookie>> {
    let value = live
        .pipe
        .call("Storage.getCookies", json!({}), None)
        .await
        .ok()?;
    // Take only metadata (never put cookies' values into a returned record).
    Some(
        value["cookies"]
            .as_array()?
            .iter()
            .filter_map(|v| {
                let persistent = v["session"] != true;
                let expires = v["expires"]
                    .as_f64()
                    .filter(|n| persistent && *n > 0.)
                    .map(|n| n * 1000.0);
                Some(Cookie {
                    domain: v["domain"].as_str()?.into(),
                    name: v["name"].as_str()?.into(),
                    path: v["path"].as_str().unwrap_or("/").into(),
                    expires,
                    persistent,
                    partition: v.get("partitionKey").cloned(),
                })
            })
            .collect(),
    )
}
fn storage_hosts(folder: &Path) -> BTreeMap<String, Vec<PathBuf>> {
    let mut found = BTreeMap::<String, Vec<PathBuf>>::new();
    let indexed = folder.join("profile/Default/IndexedDB");
    for entry in fs::read_dir(&indexed).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let host = name
            .strip_prefix("https_")
            .or_else(|| name.strip_prefix("http_"))
            .and_then(|n| {
                n.strip_suffix(".indexeddb.leveldb")
                    .or_else(|| n.strip_suffix(".indexeddb.blob"))
            })
            .and_then(|n| n.rsplit_once('_'))
            .filter(|(_, port)| port.bytes().all(|b| b.is_ascii_digit()));
        if let Some((host, _)) = host {
            found.entry(host.into()).or_default().push(entry.path());
        }
    }
    found
}
#[derive(Clone)]
pub struct Sites {
    home: PathBuf,
    browser: Browser,
}
impl Sites {
    pub fn new(home: PathBuf, browser: Browser) -> Self {
        Self { home, browser }
    }
    async fn cookies(&self) -> (bool, Option<Vec<Cookie>>) {
        if let Some(lease) = self.browser.existing().await
            && let Some(cookies) = live_cookies(&lease.live).await
        {
            return (true, Some(cookies));
        }
        let folder = self.home.join("browser");
        (
            false,
            tokio::task::spawn_blocking(move || file_cookies(&folder))
                .await
                .ok()
                .flatten(),
        )
    }
    async fn listed(&self) -> Value {
        let (_, cookies) = self.cookies().await;
        let now = jiff::Timestamp::now().as_millisecond() as f64;
        let mut sites = BTreeMap::<String, (bool, bool)>::new();
        for cookie in cookies.into_iter().flatten() {
            if cookie.expires.is_none_or(|at| at > now) {
                let site = site_of(&cookie.domain);
                if !site.is_empty() {
                    sites.entry(site).or_default().0 = true;
                }
            }
        }
        let folder = self.home.join("browser");
        for host in tokio::task::spawn_blocking(move || storage_hosts(&folder))
            .await
            .unwrap_or_default()
            .keys()
        {
            let site = site_of(host);
            if !site.is_empty() {
                sites.entry(site).or_default().1 = true;
            }
        }
        json!({"sites":sites.into_iter().map(|(site,(cookies,storage))|{let mut kinds=Vec::new();if cookies{kinds.push("cookies");}if storage{kinds.push("storage");}json!({"site":site,"kinds":kinds})}).collect::<Vec<_>>()})
    }
    pub async fn signed_in(
        &self,
        entries: &[crate::integrations::upgrade::CatalogIntegration],
    ) -> Option<serde_json::Map<String, Value>> {
        let named: Vec<_> = entries
            .iter()
            .filter_map(|e| {
                let cookie = e.raw.get("signInCookie")?;
                Some((
                    &e.id,
                    cookie["domain"].as_str()?.to_lowercase(),
                    cookie["name"].as_str()?,
                ))
            })
            .collect();
        if named.is_empty() {
            return None;
        }
        let (live, cookies) = self.cookies().await;
        let folder = self.home.join("browser");
        let cookies = match cookies {
            Some(c) => c,
            None if folder.join("profile").exists() && cookie_file(&folder).is_none() => vec![],
            None => return None,
        };
        let now = jiff::Timestamp::now().as_millisecond() as f64;
        Some(
            named
                .into_iter()
                .map(|(id, domain, name)| {
                    (
                        id.clone(),
                        json!(cookies.iter().any(|c| on_site(
                            &c.domain,
                            domain.trim_start_matches('.')
                        ) && c.name == name
                            && (live || c.persistent)
                            && c.expires.map_or(live, |at| at > now))),
                    )
                })
                .collect(),
        )
    }
    async fn remove(&self, site: Option<String>) -> io::Result<Value> {
        let folder = self.home.join("browser");
        if let Some(lease) = self.browser.existing().await {
            remove_live(&lease.live, &folder, site.as_deref()).await?;
        } else {
            let deleting = site.clone();
            tokio::task::spawn_blocking(move || remove_stopped(&folder, deleting.as_deref()))
                .await
                .map_err(io::Error::other)??;
        }
        let mut listed = self.listed().await;
        if let Some(sites) = listed["sites"].as_array_mut() {
            if let Some(site) = site {
                sites.retain(|item| item["site"] != site);
            } else {
                sites.clear();
            }
        }
        Ok(listed)
    }
    pub fn register(&self, routes: &mut Registry) {
        let read = self.clone();
        routes.register::<get::Spec, _, _>(Access::Any, move |_| {
            let read = read.clone();
            async move { Ok(RouteReply::json_as_kept::<200>(read.listed().await)) }
        });
        let clear = self.clone();
        routes.register::<all::Spec, _, _>(Access::Any, move |_| {
            let clear = clear.clone();
            async move {
                Ok(RouteReply::json_as_kept::<200>(
                    clear.remove(None).await.map_err(busy)?,
                ))
            }
        });
        let forget = self.clone();
        routes.register::<one::Spec, _, _>(Access::Any, move |call| {
            let forget = forget.clone();
            let site = call
                .param("site")
                .and_then(|s| percent_encoding::percent_decode_str(s).decode_utf8().ok())
                .map(|s| s.to_lowercase());
            async move {
                let site = site
                    .filter(|s| site_of(s) == *s && (s.contains('.') || s == "localhost"))
                    .ok_or_else(|| {
                        Refusal::new(
                            StatusCode::BAD_REQUEST,
                            "Name a site as the list of sites names it.",
                        )
                    })?;
                Ok(RouteReply::json_as_kept::<200>(
                    forget.remove(Some(site)).await.map_err(busy)?,
                ))
            }
        });
    }
}
fn busy(_: impl std::fmt::Display) -> Refusal {
    Refusal::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "The browser is busy starting. Try again in a moment.",
    )
}
fn remove_stopped(folder: &Path, site: Option<&str>) -> io::Result<()> {
    if !folder.join("profile").exists() {
        return Ok(());
    }
    let _lock = OwnedLock::acquire(&folder.join(".profile-editing"), Duration::from_secs(3))?;
    let wanted = |host: &str| site.is_none_or(|site| on_site(host, site));
    let mut hosts = BTreeSet::new();
    if let Some(file) = cookie_file(folder) {
        let mut db = database(&file, false).map_err(io::Error::other)?;
        let tx = db.transaction().map_err(io::Error::other)?;
        let domains = {
            let mut query = tx
                .prepare("SELECT DISTINCT host_key FROM cookies")
                .map_err(io::Error::other)?;
            query
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(io::Error::other)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(io::Error::other)?
        };
        for host in domains.into_iter().filter(|h| wanted(h)) {
            tx.execute("DELETE FROM cookies WHERE host_key = ?", [&host])
                .map_err(io::Error::other)?;
            hosts.insert(host.trim_start_matches('.').into());
        }
        tx.commit().map_err(io::Error::other)?;
    }
    if let Some(site) = site {
        for (host, entries) in storage_hosts(folder) {
            if wanted(&host) {
                for entry in entries {
                    files::remove_tree_synced(&entry)?;
                }
                hosts.insert(host);
            }
        }
        let pending = folder.join("clear-sites.json");
        let mut kept: Vec<String> = files::text_of(&pending)
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default();
        hosts.insert(site.into());
        for host in hosts {
            if !kept.contains(&host) {
                kept.push(host);
            }
        }
        files::write_secret(&pending, json!(kept).to_string().as_bytes())?;
    } else {
        for part in STORAGE {
            files::remove_tree_synced(&folder.join("profile/Default").join(part))?;
        }
    }
    Ok(())
}
async fn remove_live(live: &Living, folder: &Path, site: Option<&str>) -> io::Result<()> {
    let cookies = live_cookies(live)
        .await
        .ok_or_else(|| io::Error::other("Cookies couldn't be read."))?;
    let wanted = |host: &str| site.is_none_or(|site| on_site(host, site));
    let mut hosts: BTreeSet<_> = cookies
        .iter()
        .filter(|c| wanted(&c.domain))
        .map(|c| c.domain.trim_start_matches('.').to_owned())
        .collect();
    hosts.extend(
        storage_hosts(folder)
            .into_keys()
            .filter(|host| wanted(host)),
    );
    if hosts.is_empty() && site.is_some() {
        return Ok(());
    }
    let targets = live.refresh_targets().await?;
    let opened = targets.is_empty();
    let page = match targets.first() {
        Some(target) => live.attach(&target.target_id).await?,
        None => live.create(None).await?,
    };
    let result = async {
        if site.is_none() {
            live.pipe
                .call(
                    "Network.clearBrowserCookies",
                    json!({}),
                    Some(&page.session),
                )
                .await?;
            let _ = live
                .pipe
                .call("Network.clearBrowserCache", json!({}), Some(&page.session))
                .await;
        } else {
            for c in cookies.iter().filter(|c| wanted(&c.domain)) {
                let mut params = json!({"name":c.name,"domain":c.domain,"path":c.path});
                if let Some(partition) = &c.partition {
                    params["partitionKey"] = partition.clone();
                }
                live.pipe
                    .call("Network.deleteCookies", params, Some(&page.session))
                    .await?;
            }
        }
        for host in hosts {
            for scheme in ["http", "https"] {
                let _ = live
                    .pipe
                    .call(
                        "Storage.clearDataForOrigin",
                        json!({"origin":format!("{scheme}://{host}"),"storageTypes":"all"}),
                        Some(&page.session),
                    )
                    .await;
            }
        }
        Ok(())
    }
    .await;
    if opened {
        let _ = live
            .pipe
            .call("Target.closeTarget", json!({"targetId":page.target}), None)
            .await;
    }
    result
}
