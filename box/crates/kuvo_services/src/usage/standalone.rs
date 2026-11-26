//! Plan readers for the agents whose full conversation adapters are separate
//! from this usage service. Each source is the account Kuvo owns, never a
//! browser or another desktop application's sign-in.
use super::{
    engine_readers::{claims, date, fingerprint, grok, number, title, window},
    grok_native,
    limits::{Account, Provider, Service, now},
    plan_facts::{Amount, Balance, Extra, Facts, Problem, ProblemKind, Unit},
    readings::Window,
};
use kuvo_agents::accounts::{Account as AgentAccount, Accounts, AgentKind};
use kuvo_os::platform::{Environment, Platform, env_value};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;

#[path = "antigravity_refusals.rs"]
mod antigravity_refusals;
#[path = "cursor_history_reader.rs"]
mod cursor_history;
use antigravity_refusals::{Kind as RefusalKind, Refusals};

type Plan = (Vec<Window>, Facts, Option<String>);
const MINUTE: u64 = 60_000;
const DAY: u64 = 24 * 60 * MINUTE;
fn str_at(v: &Value) -> Option<&str> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty())
}
fn identity(prefix: &str, id: &str) -> String {
    Account::identity_of(&format!("{prefix}:{}", id.to_ascii_lowercase()))
}
fn jwt_subject(token: &str) -> Option<String> {
    str_at(&claims(Some(token))?["sub"]).map(str::to_owned)
}
fn problem(kind: ProblemKind, words: impl Into<String>) -> Problem {
    Problem::new(kind, words)
}
fn positive(v: &Value) -> Option<f64> {
    number(v).filter(|n| *n > 0.0)
}
fn iso(v: &Value) -> Option<u64> {
    date(v).or_else(|| {
        let raw = v.as_str()?.trim();
        let civil = raw
            .strip_suffix(" UTC")
            .unwrap_or(raw)
            .parse::<jiff::civil::DateTime>()
            .ok()?;
        civil
            .to_zoned(jiff::tz::TimeZone::UTC)
            .ok()?
            .timestamp()
            .as_millisecond()
            .try_into()
            .ok()
    })
}
fn usd(n: f64) -> f64 {
    n.round() / 100.0
}
fn sign_mark(secret: &str) -> String {
    fingerprint(secret)
}

#[derive(Clone)]
pub struct Readers {
    root: PathBuf,
    personal: PathBuf,
    accounts: Accounts,
    env: Environment,
    client: reqwest::Client,
    fixture: Option<url::Url>,
    isolated: bool,
    cursor: Arc<Mutex<BTreeMap<String, Arc<Mutex<CursorSignIn>>>>>,
    antigravity: Arc<Mutex<BTreeMap<String, Arc<Mutex<GoogleSignIn>>>>>,
}
#[derive(Clone, Default)]
struct CursorSignIn {
    mark: String,
    access: Option<String>,
    until: u64,
    refused: bool,
    held: u64,
}
#[derive(Default)]
struct GoogleSignIn {
    mark: String,
    access: Option<String>,
    until: u64,
    refusals: Refusals,
    held: BTreeMap<String, u64>,
}
impl GoogleSignIn {
    fn hold(&mut self, url: &str, until: u64) {
        self.held.insert(url.into(), until);
    }
    fn waiting(&mut self, url: &str, at: u64) -> Option<u64> {
        self.held.retain(|_, until| *until > at);
        self.held.get(url).copied()
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorKey {
    api_key: String,
    api_key_expires_at_ms: Option<u64>,
}
#[derive(Clone)]
struct Google {
    access: Option<String>,
    refresh: Option<String>,
    expires: Option<u64>,
    identity: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
}
#[derive(Clone)]
enum Credential {
    Grok {
        home: PathBuf,
        api_key: bool,
    },
    Cursor {
        key: String,
    },
    Antigravity {
        credential: Option<Google>,
        profile: PathBuf,
    },
}
struct Candidate {
    record: AgentAccount,
    account: Account,
    credential: Credential,
}

fn read_json(path: &Path) -> std::io::Result<Option<Value>> {
    kuvo_store::files::read(path)?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(std::io::Error::other))
        .transpose()
}
fn cursor_key(
    folder: &Path,
    own: bool,
    env: &Environment,
    at: u64,
) -> std::io::Result<Option<String>> {
    if own
        && let Some(value) = env_value(Platform::current(), env, "CURSOR_API_KEY")
            .and_then(OsStr::to_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    {
        return Ok(Some(value.into()));
    }
    let Some(bytes) = kuvo_store::files::read(&folder.join("auth.json"))? else {
        return Ok(None);
    };
    let saved: CursorKey = serde_json::from_slice(&bytes)?;
    Ok((saved.api_key_expires_at_ms.is_none_or(|end| end >= at)
        && !saved.api_key.trim().is_empty())
    .then(|| saved.api_key.trim().to_owned()))
}
fn google(folder: &Path) -> std::io::Result<Option<Google>> {
    let Some(raw) = read_json(&folder.join("profile/antigravity-acp/acp_token.json"))? else {
        return Ok(None);
    };
    let v = raw.get("token").filter(|v| v.is_object()).unwrap_or(&raw);
    let access = str_at(&v["access_token"])
        .or_else(|| str_at(&v["accessToken"]))
        .map(str::to_owned);
    let refresh = str_at(&v["refresh_token"])
        .or_else(|| str_at(&v["refreshToken"]))
        .map(str::to_owned);
    if access.is_none() && refresh.is_none() {
        return Ok(None);
    }
    let identity = str_at(&v["id_token"])
        .or_else(|| str_at(&v["idToken"]))
        .and_then(jwt_subject)
        .or_else(|| access.as_deref().and_then(jwt_subject))
        .map(|id| identity("google", &id));
    let expires = number(&v["expires_at"])
        .map(|n| (n * 1000.0) as u64)
        .or_else(|| iso(&v["expiry"]))
        .or_else(|| iso(&v["expiresAt"]));
    Ok(Some(Google {
        access,
        refresh,
        expires,
        identity,
        client_id: str_at(&v["client_id"]).map(str::to_owned),
        client_secret: str_at(&v["client_secret"]).map(str::to_owned),
    }))
}
async fn watch_credential(
    path: PathBuf,
    hub: kuvo_os::watch::WatchHub,
    changed: tokio::sync::watch::Sender<u64>,
) {
    use kuvo_os::watch::{Notice, Options};
    loop {
        // Missing provider folders are never created merely to watch a login.
        let Some(folder) = path
            .ancestors()
            .skip(1)
            .find(|p| p.is_dir())
            .map(Path::to_owned)
        else {
            return;
        };
        let Some(next) = path
            .strip_prefix(&folder)
            .ok()
            .and_then(|p| p.iter().next())
            .map(ToOwned::to_owned)
        else {
            return;
        };
        let Ok(mut watch) = hub.subscribe(&folder, Options::default()).await else {
            return;
        };
        loop {
            let Some(notice) = watch.recv().await else {
                return;
            };
            let relevant = match notice {
                Notice::Changed { paths } => paths
                    .iter()
                    .any(|p| p.as_os_str().is_empty() || p.iter().next() == Some(next.as_os_str())),
                Notice::Rescan => true,
                Notice::Failed(_) => {
                    changed.send_modify(|n| *n = n.wrapping_add(1));
                    if !folder.is_dir() {
                        break;
                    }
                    return;
                }
            };
            if !relevant {
                continue;
            }
            changed.send_modify(|n| *n = n.wrapping_add(1));
            // Move down as the provider creates its folder, or back up on removal.
            if !folder.is_dir()
                || path.ancestors().skip(1).find(|p| p.is_dir()) != Some(folder.as_path())
            {
                break;
            }
        }
    }
}

impl Readers {
    pub fn for_home(home: &Path, personal: &Path) -> Option<Self> {
        let isolated = std::env::var("KUVO_TEST_PATH_ONLY").as_deref() == Ok("1");
        let fixture = std::env::var("KUVO_TEST_USAGE_BASE")
            .ok()
            .and_then(|v| v.parse::<url::Url>().ok())
            .filter(|u| u.scheme() == "http" && matches!(u.host_str(), Some("127.0.0.1" | "::1")));
        // An isolated test can launch its admitted CLI stand-in; HTTP is only
        // enabled if its explicit endpoint fixture is loopback.
        if !isolated
            && (home != personal.join(".kuvo-code")
                || std::env::var("KUVO_TEST_PRICE_FEEDS").as_deref() == Ok("off"))
        {
            return None;
        }
        let store = kuvo_store::home::Home::new(home.to_owned()).ok()?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .ok()?;
        Some(Self {
            root: home.join("agents"),
            personal: personal.to_owned(),
            accounts: Accounts::for_home(&store),
            env: std::env::vars_os().collect(),
            client,
            fixture: if isolated { fixture } else { None },
            isolated,
            cursor: Default::default(),
            antigravity: Default::default(),
        })
    }
    fn candidate(&self, record: AgentAccount, at: u64) -> std::io::Result<Option<Candidate>> {
        let managed = self.accounts.home(&record);
        let provider = record.provider().to_string();
        let (service, sign_in, who, credential) = match record.kind {
            AgentKind::Grok => {
                use kuvo_agents::acp::{GrokSecret, grok_installed, grok_secret};
                // Exactly the credential and installed check Accounts uses. The
                // transcript index does not participate in sign-in admission.
                let Some(secret) = grok_secret(managed.as_deref()) else {
                    return Ok(None);
                };
                if !grok_installed() {
                    return Ok(None);
                }
                let mark = secret.credential();
                let (who, api_key) = match secret {
                    GrokSecret::ApiKey(_) => (None, true),
                    GrokSecret::SignedIn { key, id_token } => (
                        jwt_subject(&key)
                            .or_else(|| id_token.as_deref().and_then(jwt_subject))
                            .map(|id| identity("xai", &id)),
                        false,
                    ),
                };
                let home = grok_native::folder(managed, &self.env, &self.personal);
                (
                    Provider::Grok,
                    mark,
                    who,
                    Credential::Grok { home, api_key },
                )
            }
            AgentKind::Cursor if !self.isolated || self.fixture.is_some() => {
                let own = managed.is_none();
                let folder = managed.unwrap_or_else(|| self.root.join("cursor"));
                let Some(key) = cursor_key(&folder, own, &self.env, at)? else {
                    return Ok(None);
                };
                (
                    Provider::Cursor,
                    sign_mark(&key),
                    None,
                    Credential::Cursor { key },
                )
            }
            AgentKind::Antigravity if !self.isolated || self.fixture.is_some() => {
                let folder = managed.unwrap_or_else(|| self.root.join("antigravity"));
                let profile = folder.join("profile");
                let credential = google(&folder)?;
                let (mark, who) = if let Some(cred) = &credential {
                    (
                        sign_mark(&serde_json::to_string(&[
                            cred.access.as_deref(),
                            cred.refresh.as_deref(),
                        ])?),
                        cred.identity.clone(),
                    )
                } else {
                    let Some(raw) = read_json(&profile.join("antigravity-acp/kuvo-sign-in.json"))?
                    else {
                        return Ok(None);
                    };
                    let Some(id) = str_at(&raw["id"]) else {
                        return Ok(None);
                    };
                    (sign_mark(id), None)
                };
                (
                    Provider::Antigravity,
                    mark,
                    who,
                    Credential::Antigravity {
                        credential,
                        profile,
                    },
                )
            }
            _ => return Ok(None),
        };
        Ok(Some(Candidate {
            record: record.clone(),
            account: Account {
                provider,
                service,
                account: Some(record.id),
                label: record.label,
                identity: who,
                sign_in,
            },
            credential,
        }))
    }
    async fn candidates(&self, at: u64) -> std::io::Result<(Vec<Candidate>, BTreeSet<String>)> {
        let list = self.accounts.all().await?;
        let readers = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut found = vec![];
            let mut unreadable = BTreeSet::new();
            for record in list {
                let id = record.provider().to_string();
                match readers.candidate(record, at) {
                    Ok(Some(c)) => found.push(c),
                    Ok(None) => {}
                    Err(error) => {
                        unreadable.insert(id.clone());
                        eprintln!("Could not read {id}'s plan sign-in: {error}");
                    }
                }
            }
            Ok((found, unreadable))
        })
        .await?
    }
    async fn prune_google(&self, found: &[Candidate], unreadable: &BTreeSet<String>) {
        if self.isolated && self.fixture.is_none() {
            return;
        }
        let ids = found
            .iter()
            .filter(|c| c.account.service == Provider::Antigravity)
            .map(|c| c.account.account.clone().unwrap_or("default".into()))
            .chain(unreadable.iter().filter_map(|id| {
                if id == "antigravity" {
                    Some("default".into())
                } else {
                    id.strip_prefix("antigravity@").map(str::to_owned)
                }
            }))
            .collect::<BTreeSet<_>>();
        self.antigravity.lock().await.retain(|provider, _| {
            provider
                .strip_prefix("antigravity@")
                .map_or_else(|| ids.contains("default"), |id| ids.contains(id))
        });
        let root = self.root.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let folder = root.join("usage/antigravity");
            let Ok(entries) = std::fs::read_dir(folder) else {
                return;
            };
            for entry in entries.flatten() {
                let Ok(id) = entry.file_name().into_string() else {
                    continue;
                };
                if !ids.contains(&id) {
                    if let Err(error) = Refusals::default().clear(&root, &id) {
                        eprintln!("Couldn't prune Antigravity's refusal record: {error}");
                    }
                }
            }
        })
        .await;
    }
    /// Watch credentials, not transcripts. A CLI sign-out on an otherwise idle
    /// computer must withdraw its peer row even before a local client opens Usage.
    pub fn follow_sign_ins(
        &self,
        hub: kuvo_os::watch::WatchHub,
        service: Service,
    ) -> tokio::task::JoinHandle<()> {
        use kuvo_os::watch::{Notice, Options};
        let readers = self.clone();
        tokio::spawn(async move {
            let Ok(mut roster) = hub
                .subscribe(
                    &readers.root,
                    Options {
                        recursive: false,
                        create: true,
                    },
                )
                .await
            else {
                return;
            };
            let (changed, mut changes) = tokio::sync::watch::channel(0u64);
            struct Watches(BTreeMap<PathBuf, tokio::task::JoinHandle<()>>);
            impl Drop for Watches {
                fn drop(&mut self) {
                    for task in self.0.values() {
                        task.abort();
                    }
                }
            }
            let mut watches = Watches(BTreeMap::new());
            loop {
                tokio::select! {
                    notice=roster.recv()=>{
                        let Some(notice)=notice else {return;};
                        if matches!(&notice,Notice::Changed{paths} if !paths.iter().any(|p|p==Path::new("accounts.json"))) {continue;}
                        let Ok(records)=readers.accounts.all().await else{continue;};
                        let mut wanted=BTreeSet::new();
                        for record in records {
                            let managed=readers.accounts.home(&record);
                            match record.kind {
                                AgentKind::Grok=>{wanted.insert(grok_native::folder(managed,&readers.env,&readers.personal).join("auth.json"));},
                                AgentKind::Cursor if !readers.isolated||readers.fixture.is_some()=>{wanted.insert(managed.unwrap_or_else(||readers.root.join("cursor")).join("auth.json"));},
                                AgentKind::Antigravity if !readers.isolated||readers.fixture.is_some()=>{
                                    let folder=managed.unwrap_or_else(||readers.root.join("antigravity")).join("profile/antigravity-acp");
                                    for name in ["acp_token.json","kuvo-sign-in.json"] {wanted.insert(folder.join(name));}
                                },
                                _=>{},
                            }
                        }
                        watches.0.retain(|path,task|{let keep=wanted.contains(path)&&!task.is_finished();if !keep{task.abort();}keep});
                        for path in wanted {
                            watches.0.entry(path.clone()).or_insert_with(||tokio::spawn(watch_credential(path,hub.clone(),changed.clone())));
                        }
                        readers.confirm(&service).await;
                    },
                    result=changes.changed()=>{
                        if result.is_err(){return;}
                        readers.confirm(&service).await;
                    },
                }
            }
        })
    }

    pub async fn confirm(&self, service: &Service) {
        if let Ok((found, unreadable)) = self.candidates(now()).await {
            self.prune_google(&found, &unreadable).await;
            service.confirm_standalone_accounts(
                &found.into_iter().map(|c| c.account).collect::<Vec<_>>(),
                &unreadable,
                now(),
            );
        }
    }
    pub async fn refresh(&self, service: Service, selected: Option<&str>, explicit: bool) -> bool {
        let at = now();
        let (found, unreadable) = match self.candidates(at).await {
            Ok(c) => c,
            Err(error) => {
                eprintln!("Could not read standalone plan accounts: {error}");
                return true;
            }
        };
        let mut failed = !unreadable.is_empty();
        self.prune_google(&found, &unreadable).await;
        // Preserve held/unreadable rows, remove only ones known signed out.
        service.confirm_standalone_accounts(
            &found.iter().map(|c| c.account.clone()).collect::<Vec<_>>(),
            &unreadable,
            at,
        );
        for mut source in found {
            if selected.is_some_and(|id| id != source.account.provider) {
                continue;
            }
            if service.held_cursor(&source.account, at) {
                continue;
            }
            if !explicit
                && selected.is_none()
                && service.defer_passive(&source.account, at, 10 * MINUTE)
            {
                continue;
            }
            let result = match &source.credential {
                Credential::Grok { home, api_key } => {
                    if !explicit {
                        continue;
                    }
                    match grok_native::read(home.clone(), self.isolated, *api_key).await {
                        Ok(grok_native::Answer::Billing(body)) => grok(&body, at),
                        Ok(grok_native::Answer::Expired) => Err(problem(
                            ProblemKind::Expired,
                            "Grok isn't signed in here now. Sign in again.",
                        )),
                        _ => Err(problem(
                            ProblemKind::Failed,
                            "Grok didn't answer about its plan.",
                        )),
                    }
                }
                Credential::Cursor { key } => self.cursor_read(&source.account, key, at).await,
                Credential::Antigravity {
                    credential,
                    profile,
                } => {
                    self.google_read(&source.account, credential.as_ref(), profile, at)
                        .await
                }
            };
            let (windows, mut facts) = match result {
                Ok((windows, facts, who)) => {
                    if let Some(who) = who {
                        source.account.identity = Some(who);
                    }
                    (windows, facts)
                }
                Err(problem) => {
                    failed |= problem.kind == ProblemKind::Failed;
                    (
                        vec![],
                        Facts {
                            problem: Some(problem),
                            ..Default::default()
                        },
                    )
                }
            };
            facts.checked = Some(at);
            facts.next = Some(
                facts
                    .problem
                    .as_ref()
                    .and_then(|p| p.until)
                    .unwrap_or(at + 10 * MINUTE),
            );
            // Recheck the admitted sign-in after the maker answered; old responses
            // must never become the new account's reading.
            let admitted = self.accounts.all().await.ok().is_some_and(|list| {
                list.iter()
                    .any(|r| r.provider() == source.record.provider())
            });
            if !admitted {
                continue;
            }
            let readers = self.clone();
            let record = source.record.clone();
            let live = tokio::task::spawn_blocking(move || readers.candidate(record, now()))
                .await
                .ok()
                .and_then(Result::ok)
                .flatten();
            let Some(live) = live else {
                continue;
            };
            if live.account.sign_in != source.account.sign_in {
                if live.account.identity.is_none()
                    || live.account.identity != source.account.identity
                {
                    continue;
                }
                source.account.sign_in = live.account.sign_in;
            }
            let service = service.clone();
            let kept = tokio::task::spawn_blocking(move || {
                service.observe_confirmed_facts(source.account, windows, facts, at)
            })
            .await;
            if !matches!(kept, Ok(Ok(()))) {
                failed = true;
                eprintln!("Couldn't keep standalone plan: {kept:?}");
            }
        }
        failed
    }
    fn endpoint(&self, fixture: &str, official: &str) -> String {
        self.fixture
            .as_ref()
            .and_then(|b| b.join(fixture).ok())
            .map_or_else(|| official.into(), |u| u.into())
    }
    async fn send(
        &self,
        request: reqwest::RequestBuilder,
        maker: &str,
        at: u64,
    ) -> Result<(u16, Value, Option<u64>), Problem> {
        let response = request
            .send()
            .await
            .map_err(|_| problem(ProblemKind::Failed, format!("Couldn't reach {maker}.")))?;
        let status = response.status().as_u16();
        let retry = response
            .headers()
            .get("retry-after")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| super::plan_facts::retry_at(s, at));
        let body = response.json().await.unwrap_or(Value::Null);
        Ok((status, body, retry))
    }
    async fn cursor_call(
        &self,
        method: &str,
        access: &str,
        at: u64,
    ) -> Result<(u16, Value, Option<u64>), Problem> {
        let official = format!("https://api2.cursor.sh/aiserver.v1.DashboardService/{method}");
        let fixture = format!("cursor/{method}");
        self.send(
            self.client
                .post(self.endpoint(&fixture, &official))
                .bearer_auth(access)
                .header("Connect-Protocol-Version", "1")
                .json(&json!({})),
            "Cursor",
            at,
        )
        .await
    }
    fn cursor_hold(state: &mut CursorSignIn, retry: Option<u64>, at: u64) -> Problem {
        state.held = state.held.max(retry.unwrap_or(at + 15 * MINUTE));
        Problem {
            kind: ProblemKind::RateLimited,
            message: "Cursor is turning reads away for now.".into(),
            until: Some(state.held),
            unknown: false,
        }
    }
    async fn cursor_access(
        &self,
        state: &mut CursorSignIn,
        key: &str,
        at: u64,
        fresh: bool,
    ) -> Result<String, Problem> {
        if state.refused {
            return Err(problem(
                ProblemKind::Expired,
                "Cursor turned its sign-in away: sign in to Cursor again in Settings › Accounts.",
            ));
        }
        if state.held > at {
            return Err(Self::cursor_hold(state, Some(state.held), at));
        }
        if !fresh
            && state.until > at
            && let Some(token) = &state.access
        {
            return Ok(token.clone());
        }
        let (status, body, retry) = self
            .send(
                self.client
                    .post(self.endpoint(
                        "cursor/exchange",
                        "https://api2.cursor.sh/auth/exchange_user_api_key",
                    ))
                    .bearer_auth(key)
                    .json(&json!({})),
                "Cursor",
                at,
            )
            .await?;
        if matches!(status, 400 | 401) {
            state.refused = true;
            return Err(problem(
                ProblemKind::Expired,
                "Cursor turned its sign-in away: sign in to Cursor again in Settings › Accounts.",
            ));
        }
        if status == 429 {
            return Err(Self::cursor_hold(state, retry, at));
        }
        if !(200..300).contains(&status) {
            return Err(problem(
                ProblemKind::Failed,
                format!("Cursor answered {status}."),
            ));
        }
        let access = str_at(&body["accessToken"])
            .ok_or_else(|| problem(ProblemKind::Failed, "Cursor's answer wasn't one it gives."))?
            .to_owned();
        state.until = claims(Some(&access))
            .and_then(|v| number(&v["exp"]))
            .map(|n| (n * 1000.0) as u64)
            .unwrap_or(0)
            .saturating_sub(5 * MINUTE);
        state.access = Some(access.clone());
        Ok(access)
    }
    async fn cursor_optional(
        &self,
        state: &mut CursorSignIn,
        method: &str,
        access: &str,
        at: u64,
    ) -> Result<Value, Problem> {
        match self.cursor_call(method, access, at).await {
            Ok((429, _, retry)) => Err(Self::cursor_hold(state, retry, at)),
            Ok((status, body, _)) if (200..300).contains(&status) => Ok(body),
            _ => Ok(Value::Null),
        }
    }
    async fn cursor_website(
        &self,
        state: &mut CursorSignIn,
        path: &str,
        user: &str,
        access: &str,
        at: u64,
        query: bool,
    ) -> Result<Value, Problem> {
        let official = format!("https://cursor.com{path}");
        let fixture = format!("cursor{}", path.strip_prefix("/api").unwrap_or(path));
        let mut request = self.client.get(self.endpoint(&fixture, &official)).header(
            "Cookie",
            format!("WorkosCursorSessionToken={user}%3A%3A{access}"),
        );
        if query {
            request = request.query(&[("user", user)]);
        }
        match self.send(request, "Cursor", at).await {
            Ok((429, _, retry)) => Err(Self::cursor_hold(state, retry, at)),
            Ok((status, body, _)) if (200..300).contains(&status) => Ok(body),
            _ => Ok(Value::Null),
        }
    }
    async fn cursor_read(&self, account: &Account, key: &str, at: u64) -> Result<Plan, Problem> {
        let sign = self.cursor_sign_in_state(account).await;
        let mut state = sign.lock().await;
        let mut access = self.cursor_access(&mut state, key, at, false).await?;
        let (mut status, mut usage, mut retry) = self
            .cursor_call("GetCurrentPeriodUsage", &access, at)
            .await?;
        if matches!(status, 401 | 403) {
            access = self.cursor_access(&mut state, key, at, true).await?;
            (status, usage, retry) = self
                .cursor_call("GetCurrentPeriodUsage", &access, at)
                .await?;
            if matches!(status, 401 | 403) {
                return Err(problem(
                    ProblemKind::Expired,
                    "Cursor turned its sign-in away: sign in to Cursor again in Settings › Accounts.",
                ));
            }
        }
        if status == 429 {
            return Err(Self::cursor_hold(&mut state, retry, at));
        }
        if !(200..300).contains(&status) {
            return Err(problem(
                ProblemKind::Failed,
                format!("Cursor answered {status}."),
            ));
        }
        if !usage.is_object() {
            return Err(problem(
                ProblemKind::Failed,
                "Cursor's answer wasn't one it gives.",
            ));
        }
        let user = jwt_subject(&access).map(|s| s.split('|').nth(1).unwrap_or(&s).to_owned());
        let who = user.as_deref().map(|s| identity("cursor", s));
        let info = self
            .cursor_optional(&mut state, "GetPlanInfo", &access, at)
            .await?;
        let name = str_at(&info["planInfo"]["planName"]);
        if usage["enabled"] == false {
            return Err(problem(
                ProblemKind::Ended,
                "Its Cursor plan isn't active now.",
            ));
        }
        let mut facts = Facts {
            plan: name.map(|s| title(&json!(s)).unwrap_or_else(|| s.into())),
            ..Default::default()
        };
        let p = &usage["planUsage"];
        let limit = number(&p["limit"]);
        let total = number(&p["totalPercentUsed"]);
        let auto = number(&p["autoPercentUsed"]);
        let api = number(&p["apiPercentUsed"]);
        let spent = number(&p["totalSpend"]).unwrap_or(
            limit.unwrap_or(0.0) - number(&p["remaining"]).unwrap_or(limit.unwrap_or(0.0)),
        );
        let pools = auto
            .zip(api)
            .is_some_and(|(a, b)| a >= 0.0 && b >= 0.0 && (a > 0.0 || b > 0.0 || spent == 0.0));
        let extra = &usage["spendLimitUsage"];
        let team = name.is_some_and(|s| s.eq_ignore_ascii_case("team"))
            || extra["limitType"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case("team"))
            || positive(&extra["pooledLimit"]).is_some();
        let (minutes, reset) = cycle(
            number(&usage["billingCycleStart"]).map(|n| n as u64),
            number(&usage["billingCycleEnd"]).map(|n| n as u64),
            30 * 1440,
        );
        let mut windows = vec![];
        let fallback = !p.is_object() || (limit.is_none() && total.is_none() && !pools);
        if fallback {
            if let Some(user) = user.as_deref() {
                let summary = self
                    .cursor_website(&mut state, "/api/usage-summary", user, &access, at, false)
                    .await?;
                let requests = self
                    .cursor_website(&mut state, "/api/usage", user, &access, at, true)
                    .await?;
                windows = cursor_summary(&summary, &requests, at, &mut facts)?;
            } else {
                return Err(problem(
                    ProblemKind::Failed,
                    "Cursor request-based usage data unavailable. Try again later.",
                ));
            }
        } else {
            if team && pools {
                if let Some(used) = total {
                    windows.push(percent("total", "Total usage", used, minutes, reset, at));
                }
            } else if team && let Some(cap) = limit.filter(|n| *n > 0.0) {
                windows.push(meter(
                    "total",
                    "Total usage",
                    usd(spent),
                    usd(cap),
                    Unit::Usd,
                    minutes,
                    reset,
                    at,
                ));
            } else {
                windows.push(percent(
                    "total",
                    "Total usage",
                    total.unwrap_or_else(|| {
                        if limit.is_some_and(|n| n > 0.0) {
                            spent / limit.unwrap_or(1.0) * 100.0
                        } else {
                            0.0
                        }
                    }),
                    minutes,
                    reset,
                    at,
                ));
            }
            for (id, label, value) in [
                ("auto", "Cursor Models", auto),
                ("api", "Other Models", api),
            ] {
                if let Some(value) = value {
                    windows.push(percent(id, label, value, minutes, reset, at));
                }
            }
            let cap = number(&extra["individualLimit"])
                .or_else(|| number(&extra["pooledLimit"]))
                .unwrap_or(0.0);
            let left = number(&extra["individualRemaining"])
                .or_else(|| number(&extra["pooledRemaining"]))
                .unwrap_or(0.0);
            let spent = [
                number(&extra["individualUsed"]),
                number(&extra["pooledUsed"]),
                number(&extra["totalSpend"]),
            ]
            .into_iter()
            .flatten()
            .find(|n| *n > 0.0)
            .unwrap_or((cap - left).max(0.0));
            if extra.is_object() && cap > 0.0 {
                windows.push(meter(
                    "on_demand",
                    "On-demand",
                    usd(spent),
                    usd(cap),
                    Unit::Usd,
                    None,
                    None,
                    at,
                ));
            } else if spent > 0.0 {
                facts.extras.push(Extra::Spent {
                    id: "on_demand".into(),
                    name: "On-demand".into(),
                    amount: usd(spent),
                    unit: Unit::Usd,
                    resets_at: reset,
                });
            }
            let grants = self
                .cursor_optional(&mut state, "GetCreditGrantsBalance", &access, at)
                .await?;
            let mut credit = if grants["hasCreditGrants"] == true {
                positive(&grants["totalCents"])
                    .zip(number(&grants["usedCents"]).filter(|n| *n >= 0.0))
                    .map_or(0.0, |(t, u)| (t - u).max(0.0))
            } else {
                0.0
            };
            if let Some(user) = user.as_deref() {
                let stripe = self
                    .cursor_website(&mut state, "/api/auth/stripe", user, &access, at, false)
                    .await?;
                credit += number(&stripe["customerBalance"])
                    .filter(|n| *n < 0.0)
                    .map_or(0.0, f64::abs);
                if team && !windows.iter().any(|w| w.id == "on_demand") && facts.extras.is_empty() {
                    let summary = self
                        .cursor_website(&mut state, "/api/usage-summary", user, &access, at, false)
                        .await?;
                    summary_on_demand(&summary, &mut windows, &mut facts, minutes, reset, at);
                    if facts.plan.is_none() {
                        facts.plan = title(&summary["membershipType"]);
                    }
                }
            }
            if credit > 0.0 {
                facts.balance = Some(Balance {
                    left: usd(credit),
                    limit: None,
                    unit: Unit::Usd,
                });
            }
        }
        let bot = self
            .cursor_optional(&mut state, "GetSandUsageStatus", &access, at)
            .await?;
        if bot["usesPooledEnterpriseAllowance"] != true
            && bot["hasNonZeroIncludedLimit"] != false
            && bot["includedLimitZero"] != true
            && let Some(pct) = number(&bot["usagePercent"]).filter(|n| *n >= 0.0)
        {
            let (minutes, reset) = cycle(
                iso(&bot["currentPeriodStart"]),
                iso(&bot["nextResetTimestampUtc"]),
                10080,
            );
            windows.push(percent(
                "grok_bot",
                "Grok Bot usage",
                pct,
                minutes,
                reset,
                at,
            ));
        }
        Ok((windows, facts, who))
    }
    async fn google_call(
        &self,
        base: &str,
        path: &str,
        access: &str,
        body: Value,
        at: u64,
    ) -> Result<(u16, Value, Option<u64>), Problem> {
        let official = format!("{base}/{path}");
        let fixture = format!(
            "antigravity/{}/{}",
            if base.contains("daily-") {
                "daily"
            } else {
                "main"
            },
            path
        );
        self.send(
            self.client
                .post(self.endpoint(&fixture, &official))
                .bearer_auth(access)
                .header("User-Agent", "antigravity/2.24.0")
                .json(&body),
            "Antigravity",
            at,
        )
        .await
    }
    async fn google_held_call(
        &self,
        state: &mut GoogleSignIn,
        base: &str,
        path: &str,
        access: &str,
        body: Value,
        at: u64,
    ) -> Result<(u16, Value, Option<u64>), Problem> {
        let url = format!("{base}/{path}");
        if let Some(until) = state.waiting(&url, at) {
            return Ok((429, Value::Null, Some(until)));
        }
        let result = self.google_call(base, path, access, body, at).await?;
        if result.0 == 429 {
            state.hold(&url, result.2.unwrap_or(at + 60 * MINUTE));
        }
        Ok(result)
    }
    async fn google_access(
        &self,
        state: &mut GoogleSignIn,
        credential: &Google,
        account: &str,
        at: u64,
        fresh: bool,
    ) -> Result<String, Problem> {
        if !fresh
            && state.until > at + MINUTE
            && let Some(token) = &state.access
            && !state.refusals.denied(RefusalKind::Access, token)
        {
            return Ok(token.clone());
        }
        if !fresh
            && credential.expires.is_none_or(|a| a > at + MINUTE)
            && let Some(access) = &credential.access
            && !state.refusals.denied(RefusalKind::Access, access)
        {
            return Ok(access.clone());
        }
        let Some(refresh) = &credential.refresh else {
            return Err(problem(
                ProblemKind::Expired,
                "Antigravity's sign-in has lapsed. Sign in again.",
            ));
        };
        if state.refusals.denied(RefusalKind::Refresh, refresh) {
            return Err(problem(
                ProblemKind::Expired,
                "Antigravity already refused this refresh token. Sign in again.",
            ));
        }
        // Consumer client defaults published by the maker, matching the reference reader.
        let id = credential
            .client_id
            .as_deref()
            .unwrap_or("1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com");
        let secret = credential
            .client_secret
            .as_deref()
            .unwrap_or("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");
        if let Some(until) = state.waiting("oauth", at) {
            return Err(Problem {
                kind: ProblemKind::RateLimited,
                message: "Antigravity asked this computer to wait.".into(),
                until: Some(until),
                unknown: false,
            });
        }
        let (status, value, retry) = self
            .send(
                self.client
                    .post(self.endpoint("antigravity/token", "https://oauth2.googleapis.com/token"))
                    .form(&[
                        ("grant_type", "refresh_token"),
                        ("refresh_token", refresh.as_str()),
                        ("client_id", id),
                        ("client_secret", secret),
                    ]),
                "Antigravity",
                at,
            )
            .await?;
        if status == 429 {
            let until = retry.unwrap_or(at + 60 * MINUTE);
            state.hold("oauth", until);
            return Err(Problem {
                kind: ProblemKind::RateLimited,
                message: "Antigravity asked this computer to wait.".into(),
                until: Some(until),
                unknown: false,
            });
        }
        if !(200..300).contains(&status) {
            let denied = matches!(status, 400 | 401 | 403);
            if denied {
                state
                    .refusals
                    .refuse(&self.root, account, RefusalKind::Refresh, refresh)?;
            }
            return Err(problem(
                if denied {
                    ProblemKind::Expired
                } else {
                    ProblemKind::Failed
                },
                "Antigravity couldn't renew its sign-in.",
            ));
        }
        let access = str_at(&value["access_token"])
            .ok_or_else(|| {
                problem(
                    ProblemKind::Failed,
                    "Antigravity token refresh returned no access token.",
                )
            })?
            .to_owned();
        if state.refusals.denied(RefusalKind::Access, &access) {
            return Err(problem(
                ProblemKind::Expired,
                "Antigravity returned its refused access token; no credential was sent again.",
            ));
        }
        state.until =
            at.saturating_add((positive(&value["expires_in"]).unwrap_or(3600.0) * 1000.0) as u64);
        state.access = Some(access.clone());
        Ok(access)
    }
    async fn google_local_call(
        &self,
        state: &mut GoogleSignIn,
        client: &reqwest::Client,
        endpoint: &kuvo_os::usage_server::Endpoint,
        method: &str,
        at: u64,
    ) -> Result<(u16, Value, Option<u64>), Problem> {
        // The certificate exception belongs solely to this literal local URL;
        // this client is never used for the cloud and never follows a redirect.
        let scheme = if endpoint.tls { "https" } else { "http" };
        let url = format!(
            "{scheme}://127.0.0.1:{}/exa.language_server_pb.LanguageServerService/{method}",
            endpoint.port
        );
        if let Some(until) = state.waiting(&url, at) {
            return Ok((429, Value::Null, Some(until)));
        }
        let result=self.send(client.post(&url).header("Connect-Protocol-Version","1").header("x-codeium-csrf-token",&endpoint.csrf).timeout(std::time::Duration::from_secs(3)).json(&json!({"metadata":{"ideName":"antigravity","extensionName":"antigravity","ideVersion":"unknown","locale":"en"}})),"Antigravity",at).await?;
        if result.0 == 429 {
            state.hold(&url, result.2.unwrap_or(at + 60 * MINUTE));
        }
        Ok(result)
    }
    async fn google_local(
        &self,
        state: &mut GoogleSignIn,
        profile: &Path,
        account: &str,
        who: Option<String>,
        at: u64,
    ) -> Result<Option<Plan>, Problem> {
        let fixture = if self.isolated {
            let fixture_key = if cfg!(windows) {
                "KUVO_TEST_AG_WINDOWS_ROOT"
            } else {
                "KUVO_TEST_AG_PROC_ROOT"
            };
            let Some(root) = self
                .env
                .get(OsStr::new(fixture_key))
                .map(PathBuf::from)
                .and_then(|p| p.canonicalize().ok())
            else {
                return Ok(None);
            };
            if !self
                .personal
                .canonicalize()
                .is_ok_and(|home| root.starts_with(home))
            {
                return Ok(None);
            }
            Some(root)
        } else {
            None
        };
        let endpoints = kuvo_os::usage_server::discover(profile.to_owned(), fixture).await;
        if endpoints.is_empty() {
            return Ok(None);
        }
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(2))
            .build()
            .map_err(|_| {
                problem(
                    ProblemKind::Failed,
                    "Could not open Antigravity's local quota reader.",
                )
            })?;
        let hold = |until: Option<u64>| Problem {
            kind: ProblemKind::RateLimited,
            message: "Antigravity asked this computer to wait.".into(),
            until: until.or(Some(at + 60 * MINUTE)),
            unknown: false,
        };
        for endpoint in endpoints {
            if !endpoint.csrf.is_empty() && state.refusals.denied(RefusalKind::Csrf, &endpoint.csrf)
            {
                continue;
            }
            let summary = self
                .google_local_call(state, &client, &endpoint, "RetrieveUserQuotaSummary", at)
                .await;
            let summary = match summary {
                Ok((429, _, until)) => return Err(hold(until)),
                Ok((401 | 403, _, _)) => {
                    if !endpoint.csrf.is_empty() {
                        state.refusals.refuse(
                            &self.root,
                            account,
                            RefusalKind::Csrf,
                            &endpoint.csrf,
                        )?;
                    }
                    continue;
                }
                Ok((status, body, _)) if (200..300).contains(&status) => body,
                _ => continue,
            };
            if let Some(windows) = antigravity_summary(&[summary], at) {
                if windows.is_empty() {
                    return Err(problem(
                        ProblemKind::Failed,
                        "Antigravity's quota summary has no readable buckets.",
                    ));
                }
                let (plan, waiting) = match self
                    .google_local_call(state, &client, &endpoint, "GetUserStatus", at)
                    .await
                {
                    Ok((status, body, _)) if (200..300).contains(&status) => {
                        (local_google_plan(&body), false)
                    }
                    Ok((429, _, _)) => (None, true),
                    _ => (None, false),
                };
                let partial =
                    waiting || windows.iter().filter(|w| w.id != "argon-weekly").count() < 4;
                return Ok(Some((
                    windows,
                    Facts {
                        plan,
                        partial,
                        ..Default::default()
                    },
                    who,
                )));
            }
            let status = self
                .google_local_call(state, &client, &endpoint, "GetUserStatus", at)
                .await;
            let status = match status {
                Ok((status, body, _)) if (200..300).contains(&status) => body,
                Ok((401 | 403, _, _)) => {
                    if !endpoint.csrf.is_empty() {
                        state.refusals.refuse(
                            &self.root,
                            account,
                            RefusalKind::Csrf,
                            &endpoint.csrf,
                        )?;
                    }
                    continue;
                }
                _ => continue,
            };
            let mut windows = antigravity_legacy(
                status["userStatus"]["cascadeModelConfigData"]["clientModelConfigs"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
                at,
            );
            if windows.is_empty() {
                if let Ok((code, body, _)) = self
                    .google_local_call(state, &client, &endpoint, "GetCommandModelConfigs", at)
                    .await
                    && (200..300).contains(&code)
                {
                    windows = antigravity_legacy(
                        body["clientModelConfigs"]
                            .as_array()
                            .map(Vec::as_slice)
                            .unwrap_or_default(),
                        at,
                    );
                }
            }
            if !windows.is_empty() {
                return Ok(Some((
                    windows,
                    Facts {
                        plan: local_google_plan(&status),
                        partial: true,
                        ..Default::default()
                    },
                    who,
                )));
            }
        }
        Ok(None)
    }
    async fn google_read(
        &self,
        account: &Account,
        credential: Option<&Google>,
        profile: &Path,
        at: u64,
    ) -> Result<Plan, Problem> {
        let slot = {
            let mut slots = self.antigravity.lock().await;
            slots
                .entry(account.provider.clone())
                .or_insert_with(|| Arc::new(Mutex::new(GoogleSignIn::default())))
                .clone()
        };
        let mut guard = slot.lock().await;
        let state = &mut *guard;
        if state.mark != account.sign_in {
            state.mark = account.sign_in.clone();
            state.access = None;
            state.until = 0;
        }
        let id = account.account.as_deref().unwrap_or("default");
        state.refusals.prepare(&self.root, id)?;
        if state
            .refusals
            .denied_hash(RefusalKind::Generation, &account.sign_in)
        {
            return Err(problem(
                ProblemKind::Expired,
                "Antigravity already refused this sign-in. Sign in again.",
            ));
        }
        if let Some(plan) = self
            .google_local(
                state,
                profile,
                id,
                credential.and_then(|c| c.identity.clone()),
                at,
            )
            .await?
        {
            return Ok(plan);
        }
        let Some(credential) = credential else {
            return Err(problem(
                ProblemKind::Failed,
                "Antigravity says its usage while it runs on this computer. Start a conversation with it to see it.",
            ));
        };
        let mut access = self.google_access(state, credential, id, at, false).await?;
        let main = "https://cloudcode-pa.googleapis.com";
        let daily = "https://daily-cloudcode-pa.googleapis.com";
        let path = "v1internal:retrieveUserQuotaSummary";
        let mut bodies = vec![];
        let mut refused = false;
        let mut held = None;
        for base in [main, daily] {
            match self
                .google_held_call(state, base, path, &access, json!({}), at)
                .await
            {
                Ok((status, body, _)) if (200..300).contains(&status) => bodies.push(body),
                Ok((401 | 403, _, _)) if base == main => refused = true,
                Ok((429, _, retry)) => {
                    let until = retry.unwrap_or(at + 60 * MINUTE);
                    held = Some(held.map_or(until, |old: u64| old.min(until)));
                }
                _ => {}
            }
            if antigravity_summary(&bodies, at)
                .is_some_and(|w| w.iter().filter(|w| w.id != "argon-weekly").count() == 4)
            {
                break;
            }
        }
        if bodies.is_empty() && refused {
            state
                .refusals
                .refuse(&self.root, id, RefusalKind::Access, &access)?;
            access = self.google_access(state, credential, id, at, true).await?;
            match self
                .google_held_call(state, main, path, &access, json!({}), at)
                .await?
            {
                (status, body, _) if (200..300).contains(&status) => bodies.push(body),
                (401 | 403, _, _) => {
                    state
                        .refusals
                        .refuse(&self.root, id, RefusalKind::Access, &access)?;
                    state.refusals.refuse_hash(
                        &self.root,
                        id,
                        RefusalKind::Generation,
                        &account.sign_in,
                    )?;
                    return Err(problem(
                        ProblemKind::Expired,
                        "Antigravity turned its sign-in away. Sign in again.",
                    ));
                }
                (429, _, retry) => {
                    let until = retry.unwrap_or(at + 60 * MINUTE);
                    held = Some(held.map_or(until, |old| old.min(until)));
                }
                _ => {}
            }
        }
        if bodies.is_empty()
            && let Some(until) = held
        {
            return Err(Problem {
                kind: ProblemKind::RateLimited,
                message: "Antigravity asked this computer to wait.".into(),
                until: Some(until),
                unknown: false,
            });
        }
        let mut windows = antigravity_summary(&bodies, at);
        if windows.is_none() {
            let mut configurations = vec![];
            for base in [main, daily] {
                if let Ok((status, value, _)) = self
                    .google_held_call(
                        state,
                        base,
                        "v1internal:fetchAvailableModels",
                        &access,
                        json!({}),
                        at,
                    )
                    .await
                    && (200..300).contains(&status)
                {
                    if let Some(models) = value["models"].as_object() {
                        for (id, v) in models {
                            let mut v = v.clone();
                            if v.is_object() {
                                v["modelID"] = json!(v["model"].as_str().unwrap_or(id));
                                configurations.push(v);
                            }
                        }
                    }
                    break;
                }
            }
            windows = Some(antigravity_legacy(&configurations, at));
            if windows.as_ref().is_none_or(Vec::is_empty) {
                if let Ok((status, value, _)) = self
                    .google_held_call(
                        state,
                        main,
                        "v1internal:retrieveUserQuota",
                        &access,
                        json!({}),
                        at,
                    )
                    .await
                    && (200..300).contains(&status)
                {
                    let configs = value["buckets"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| {
                            str_at(&v["modelId"])
                                .map(|id| json!({"label":id,"modelID":id,"quotaInfo":v}))
                        })
                        .collect::<Vec<_>>();
                    windows = Some(antigravity_legacy(&configs, at));
                }
            }
        }
        let windows = windows.filter(|w| !w.is_empty()).ok_or_else(|| {
            problem(
                ProblemKind::Failed,
                "Antigravity's quota response has no readable buckets; the last reading is kept.",
            )
        })?;
        let mut plan = None;
        let mut partial =
            held.is_some() || windows.iter().filter(|w| w.id != "argon-weekly").count() < 4;
        if let Ok((status,value,_))=self.google_held_call(state,main,"v1internal:loadCodeAssist",&access,json!({"metadata":{"ideType":"ANTIGRAVITY","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}}),at).await {
            partial |= status == 429;
            if (200..300).contains(&status) {
            plan=match value["paidTier"]["id"].as_str(){
                Some("g1-pro-tier")=>Some("Pro".into()),Some("g1-ultra-tier")=>Some("Ultra".into()),Some("g1-ultra-lite-tier")=>Some("Ultra Lite".into()),
                _=>str_at(&value["paidTier"]["name"]).or_else(||str_at(&value["currentTier"]["name"])).map(google_plan)
            }
            }
        }
        Ok((
            windows,
            Facts {
                plan,
                partial,
                ..Default::default()
            },
            credential.identity.clone(),
        ))
    }
}
fn local_google_plan(body: &Value) -> Option<String> {
    str_at(&body["userStatus"]["userTier"]["name"])
        .or_else(|| str_at(&body["userStatus"]["planStatus"]["planInfo"]["planName"]))
        .map(google_plan)
}
fn cycle(start: Option<u64>, end: Option<u64>, fallback: u32) -> (Option<u32>, Option<u64>) {
    (
        Some(
            start
                .zip(end)
                .filter(|(s, e)| e > s)
                .map_or(fallback, |(s, e)| {
                    ((e - s) as f64 / MINUTE as f64).round() as u32
                }),
        ),
        end,
    )
}
fn percent(
    id: &str,
    label: &str,
    used: f64,
    minutes: Option<u32>,
    reset: Option<u64>,
    at: u64,
) -> Window {
    window(
        id,
        used / 100.0,
        minutes,
        reset,
        Some(label),
        None,
        None,
        at,
    )
}
fn meter(
    id: &str,
    label: &str,
    used: f64,
    limit: f64,
    unit: Unit,
    minutes: Option<u32>,
    reset: Option<u64>,
    at: u64,
) -> Window {
    window(
        id,
        if limit > 0.0 { used / limit } else { 0.0 },
        minutes,
        reset,
        Some(label),
        None,
        Some(Amount { used, limit, unit }),
        at,
    )
}
fn dollar_meter(value: &Value) -> Option<(f64, f64)> {
    if value["enabled"] == false {
        return None;
    }
    let limit = positive(&value["limit"])?;
    let used = positive(&value["used"])
        .unwrap_or((limit - number(&value["remaining"]).unwrap_or(limit)).max(0.0));
    Some((usd(used), usd(limit)))
}
fn summary_on_demand(
    summary: &Value,
    windows: &mut Vec<Window>,
    facts: &mut Facts,
    minutes: Option<u32>,
    reset: Option<u64>,
    at: u64,
) {
    for bucket in [
        &summary["individualUsage"]["onDemand"],
        &summary["teamUsage"]["onDemand"],
    ] {
        if bucket["enabled"] == false {
            continue;
        }
        if let Some((used, limit)) = dollar_meter(bucket) {
            windows.push(meter(
                "on_demand",
                "On-demand",
                used,
                limit,
                Unit::Usd,
                minutes,
                reset,
                at,
            ));
            break;
        }
        if let Some(used) = positive(&bucket["used"]) {
            facts.extras.push(Extra::Spent {
                id: "on_demand".into(),
                name: "On-demand".into(),
                amount: usd(used),
                unit: Unit::Usd,
                resets_at: reset,
            });
            break;
        }
    }
}
fn cursor_summary(
    summary: &Value,
    requests: &Value,
    at: u64,
    facts: &mut Facts,
) -> Result<Vec<Window>, Problem> {
    let start = iso(&summary["billingCycleStart"]);
    let end = iso(&summary["billingCycleEnd"]);
    let fallback = iso(&requests["startOfMonth"]);
    let (minutes, reset) = if start.zip(end).is_some_and(|(s, e)| e > s) {
        cycle(start, end, 30 * 1440)
    } else {
        cycle(fallback, fallback.map(|s| s + 30 * DAY), 30 * 1440)
    };
    let mut windows = vec![];
    let p = &summary["individualUsage"]["plan"];
    if let Some(limit) = positive(&requests["gpt-4"]["maxRequestUsage"]) {
        let used = number(&requests["gpt-4"]["numRequests"])
            .or_else(|| number(&requests["gpt-4"]["numRequestsTotal"]))
            .unwrap_or(0.0)
            .max(0.0);
        windows.push(meter(
            "total",
            "Total usage",
            used,
            limit,
            Unit::Requests,
            minutes,
            reset,
            at,
        ));
    } else {
        let pooled = dollar_meter(&summary["teamUsage"]["pooled"]);
        let overall = dollar_meter(&summary["individualUsage"]["overall"]);
        if summary["limitType"] == "team"
            && let Some((u, l)) = pooled
        {
            windows.push(meter(
                "total",
                "Total usage",
                u,
                l,
                Unit::Usd,
                minutes,
                reset,
                at,
            ));
        } else if let Some(u) = number(&p["totalPercentUsed"]) {
            windows.push(percent("total", "Total usage", u, minutes, reset, at));
        } else if let Some((u, l)) = overall.or(pooled) {
            windows.push(meter(
                "total",
                "Total usage",
                u,
                l,
                Unit::Usd,
                minutes,
                reset,
                at,
            ));
        }
    }
    for (key, id, name) in [
        ("autoPercentUsed", "auto", "Cursor Models"),
        ("apiPercentUsed", "api", "Other Models"),
    ] {
        if let Some(n) = number(&p[key]) {
            windows.push(percent(id, name, n, minutes, reset, at));
        }
    }
    summary_on_demand(summary, &mut windows, facts, minutes, reset, at);
    if facts.plan.is_none() {
        facts.plan = title(&summary["membershipType"]);
    }
    if windows.is_empty() && facts.extras.is_empty() {
        return Err(problem(
            ProblemKind::Failed,
            "Cursor request-based usage data unavailable. Try again later.",
        ));
    }
    Ok(windows)
}
const BUCKETS: [(&str, &str, u32); 4] = [
    ("gemini-5h", "Gemini · 5-hour", 300),
    ("gemini-weekly", "Gemini · Weekly", 10080),
    ("3p-5h", "Claude and GPT · 5-hour", 300),
    ("3p-weekly", "Claude and GPT · Weekly", 10080),
];
fn quota_window(id: &str, name: &str, minutes: u32, b: &Value, at: u64) -> Option<Window> {
    let raw = 1.0 - b["remainingFraction"].as_f64()?.clamp(0.0, 1.0);
    let rounded = (raw * 100.0).round() / 100.0;
    Some(window(
        id,
        if rounded == 0.0 { raw } else { rounded },
        Some(minutes),
        iso(&b["resetTime"]),
        Some(name),
        None,
        None,
        at,
    ))
}
pub fn antigravity_summary(bodies: &[Value], at: u64) -> Option<Vec<Window>> {
    let lists = bodies
        .iter()
        .filter_map(|b| {
            b["response"]["groups"]
                .as_array()
                .or_else(|| b["groups"].as_array())
        })
        .collect::<Vec<_>>();
    if lists.is_empty() {
        return None;
    }
    let mut windows = vec![];
    for (id, name, minutes) in BUCKETS {
        let found = lists
            .iter()
            .flat_map(|g| g.iter())
            .filter(|g| g["displayName"] != "Gemini 4 Argon")
            .flat_map(|g| g["buckets"].as_array().into_iter().flatten())
            .find(|b| b["bucketId"] == id && b["remainingFraction"].is_number());
        if let Some(w) = found.and_then(|b| quota_window(id, name, minutes, b, at)) {
            windows.push(w);
        }
    }
    for groups in lists {
        let weekly = groups
            .iter()
            .filter(|g| g["displayName"] == "Gemini 4 Argon")
            .flat_map(|g| g["buckets"].as_array().into_iter().flatten())
            .filter(|b| b["window"] == "weekly")
            .collect::<Vec<_>>();
        let Some(b) = weekly.first().filter(|_| weekly.len() == 1) else {
            continue;
        };
        let Some(id) = str_at(&b["bucketId"]) else {
            continue;
        };
        if BUCKETS.iter().any(|(known, _, _)| *known == id)
            || b["disabled"] == true
            || (!b["disabled"].is_null() && !b["disabled"].is_boolean())
        {
            continue;
        }
        if let Some(w) = quota_window("argon-weekly", "Gemini 4 Argon · Weekly", 10080, b, at) {
            let i = windows
                .iter()
                .position(|w| w.id.starts_with("3p"))
                .unwrap_or(windows.len());
            windows.insert(i, w);
            break;
        }
    }
    Some(windows)
}
pub fn antigravity_legacy(configurations: &[Value], at: u64) -> Vec<Window> {
    let mut pools: BTreeMap<&str, (f64, Value)> = BTreeMap::new();
    let blocked = [
        "MODEL_CHAT_20706",
        "MODEL_CHAT_23310",
        "MODEL_GOOGLE_GEMINI_2_5_FLASH",
        "MODEL_GOOGLE_GEMINI_2_5_FLASH_THINKING",
        "MODEL_GOOGLE_GEMINI_2_5_FLASH_LITE",
        "MODEL_GOOGLE_GEMINI_2_5_PRO",
        "MODEL_PLACEHOLDER_M19",
        "MODEL_PLACEHOLDER_M9",
        "MODEL_PLACEHOLDER_M12",
        "MODEL_PLACEHOLDER_M421",
    ];
    for c in configurations {
        let Some(label) = str_at(&c["label"]).or_else(|| str_at(&c["displayName"])) else {
            continue;
        };
        let id = str_at(&c["modelOrAlias"]["model"])
            .or_else(|| str_at(&c["model"]))
            .or_else(|| str_at(&c["modelID"]))
            .unwrap_or("");
        if c["isInternal"] == true
            || blocked.contains(&id)
            || [label, id].iter().any(|v| {
                v.to_ascii_lowercase()
                    .replace(' ', "-")
                    .starts_with("gemini-4-argon")
            })
        {
            continue;
        }
        let pool = if label.to_ascii_lowercase().contains("gemini") {
            "gemini-5h"
        } else {
            "3p-5h"
        };
        let mut b = c["quotaInfo"].clone();
        if !b.is_object() {
            b = json!({});
        }
        let fraction = b["remainingFraction"].as_f64().unwrap_or(0.0);
        b["remainingFraction"] = json!(fraction);
        if pools.get(pool).is_none_or(|(old, _)| fraction < *old) {
            pools.insert(pool, (fraction, b));
        }
    }
    BUCKETS
        .into_iter()
        .filter_map(|(id, name, minutes)| {
            pools
                .get(id)
                .and_then(|(_, b)| quota_window(id, name, minutes, b, at))
        })
        .collect()
}
fn google_plan(raw: &str) -> String {
    if let Some(name) = raw.strip_prefix("Google AI ") {
        return title(&json!(name)).unwrap_or_else(|| name.into());
    }
    for name in ["Ultra", "Pro", "Free"] {
        if raw
            .to_ascii_lowercase()
            .contains(&name.to_ascii_lowercase())
        {
            return name.into();
        }
    }
    title(&json!(raw)).unwrap_or_else(|| raw.into())
}
#[cfg(test)]
mod parity_corpus {
    use super::*;
    use serde_json::{Value, json};
    fn labels(name: &str) -> &str {
        match name {
            "Session" => "Gemini · 5-hour",
            "Weekly" => "Gemini · Weekly",
            "Claude" => "Claude and GPT · 5-hour",
            "Claude Weekly" => "Claude and GPT · Weekly",
            "Argon Weekly" => "Gemini 4 Argon · Weekly",
            n => n,
        }
    }
    fn shaped(windows: &[Window]) -> Value {
        json!(windows.iter().map(|w|json!({"label":w.name,"used":(w.used*100.).round() as u64,"periodMs":w.minutes.map(|m|m as u64*60_000)})).collect::<Vec<_>>())
    }
    fn expected(raw: &Value) -> Value {
        let Some(items) = raw.as_array() else {
            return Value::Null;
        };
        json!(
            items
                .iter()
                .map(|item| {
                    let mut item = item.clone();
                    item["label"] = json!(labels(item["label"].as_str().unwrap_or_default()));
                    item
                })
                .collect::<Vec<_>>()
        )
    }
    #[test]
    fn shared_antigravity_summary_corpus() {
        let cases: Value = serde_json::from_str(include_str!(
            "../../../../test/fixtures/usage/antigravity-plan-cases.json"
        ))
        .unwrap();
        for case in cases.as_array().unwrap() {
            let got = antigravity_summary(case["bodies"].as_array().unwrap(), 1_794_916_800_000)
                .as_deref()
                .map(shaped)
                .unwrap_or(Value::Null);
            assert_eq!(got, expected(&case["expected"]), "{}", case["name"]);
        }
    }
    #[test]
    fn shared_legacy_quota_and_plan_name_corpus() {
        let cases: Value = serde_json::from_str(include_str!(
            "../../../../test/fixtures/usage/antigravity-legacy-plan-cases.json"
        ))
        .unwrap();
        for case in cases["legacy"].as_array().unwrap() {
            assert_eq!(
                shaped(&antigravity_legacy(
                    case["configs"].as_array().unwrap(),
                    1_794_916_800_000
                )),
                expected(&case["expected"])
            );
        }
        for case in cases["plans"].as_array().unwrap() {
            assert_eq!(
                json!(str_at(&case["input"]).map(google_plan)),
                case["expected"]
            );
        }
    }
}
