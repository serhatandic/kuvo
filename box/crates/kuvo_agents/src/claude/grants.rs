//! One-off reset grants are available only where Claude Code keeps its own
//! OAuth credentials in a file. We never read its Mac Keychain or renew a token.
use super::program::Program;
use crate::session::{ResetInventory, ResetOutcome, ResetTarget};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{io, time::Duration};
use tokio::sync::Mutex;
const HOUR: u64 = 3_600_000;
struct SignIn {
    access: String,
    mark: String,
    version: Option<String>,
}
#[derive(Clone)]
struct Grant {
    id: String,
    left: u64,
    ends: Option<u64>,
    clears: Vec<String>,
}
#[derive(Clone)]
struct Grants {
    eligible: bool,
    grants: Vec<Grant>,
    next: Option<String>,
}
#[derive(Default)]
struct Cached {
    mark: String,
    tried: u64,
    status: Option<Grants>,
}
#[derive(Default)]
pub struct Reader {
    cache: Mutex<Cached>,
}
fn now() -> u64 {
    jiff::Timestamp::now().as_millisecond().max(0) as u64
}
fn n(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse().ok())
        .filter(|n| n.is_finite())
}
fn date(value: &Value) -> Option<u64> {
    value
        .as_str()
        .and_then(|v| v.parse::<jiff::Timestamp>().ok())
        .and_then(|t| t.as_millisecond().try_into().ok())
        .or_else(|| {
            n(value).filter(|n| *n > 0.).map(|n| {
                if n < 1e12 {
                    (n * 1000.) as u64
                } else {
                    n as u64
                }
            })
        })
}
fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
fn parse(value: &Value) -> Option<Grants> {
    if !value.is_object() {
        return None;
    }
    let mut grants = vec![];
    for grant in value["grants"].as_array().into_iter().flatten() {
        let id = text(&grant["id"])?;
        let left = n(&grant["resets_left"])?.max(0.).floor() as u64;
        let ends = date(&grant["ends_at"]);
        let clears = grant["clears"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        grants.push(Grant {
            id,
            left,
            ends,
            clears,
        });
    }
    Some(Grants {
        eligible: value["eligible"] == true,
        grants,
        next: text(&value["next_grant_id"]),
    })
}
impl Grants {
    fn next(&self, at: u64) -> Option<&Grant> {
        if !self.eligible {
            return None;
        }
        let next = self.next.as_deref()?;
        self.grants
            .iter()
            .find(|g| g.id == next && g.left > 0 && g.ends.is_none_or(|end| end > at))
    }
    fn view(&self, at: u64) -> ResetInventory {
        let mut count = 0u64;
        let mut expiries = vec![];
        if self.eligible {
            for grant in self
                .grants
                .iter()
                .filter(|g| g.left > 0 && g.ends.is_none_or(|end| end > at))
            {
                count = count.saturating_add(grant.left);
                if let Some(end) = grant.ends {
                    expiries.extend(std::iter::repeat_n(end, grant.left.min(50) as usize));
                }
            }
        }
        expiries.sort();
        expiries.truncate(50);
        let next = self.next(at);
        ResetInventory {
            count: count.min(u32::MAX as u64) as u32,
            targets: next
                .into_iter()
                .map(|g| ResetTarget {
                    id: g.id.clone(),
                    expires_at: g.ends,
                })
                .collect(),
            refills: next.map(|g| g.clears.clone()).unwrap_or_default(),
            display_expiries: Some(expiries),
        }
    }
}
async fn sign_in(program: &Program) -> Option<SignIn> {
    // Production Mac accounts are Keychain-owned; fixture mode only uses
    // explicitly isolated temporary folders with a loopback maker.
    if cfg!(target_os = "macos") && fixture().is_none() {
        return None;
    }
    if std::env::var_os("KUVO_TEST_PATH_ONLY").is_some() && fixture().is_none() {
        return None;
    }
    let folder = program.config_home(program.home.root())?;
    let raw: Value = serde_json::from_slice(
        &tokio::fs::read(folder.join(".credentials.json"))
            .await
            .ok()?,
    )
    .ok()?;
    let oauth = &raw["claudeAiOauth"];
    let access = text(&oauth["accessToken"])?;
    let mark = hex::encode(&Sha256::digest(&access)[..8]);
    Some(SignIn {
        access,
        mark,
        version: program.kept_version(),
    })
}
fn fixture() -> Option<url::Url> {
    if std::env::var_os("KUVO_TEST_PATH_ONLY").is_none() {
        return None;
    }
    let url = url::Url::parse(&std::env::var("KUVO_TEST_USAGE_BASE").ok()?).ok()?;
    (url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")))
        .then_some(url)
}
fn endpoint(path: &str) -> String {
    fixture()
        .and_then(|base| base.join(&format!("claude/{path}")).ok())
        .map(|url| url.to_string())
        .unwrap_or_else(|| format!("https://api.anthropic.com/api/{path}"))
}
fn client() -> io::Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(io::Error::other)
}
fn headers(request: reqwest::RequestBuilder, sign: &SignIn) -> reqwest::RequestBuilder {
    let request = request
        .bearer_auth(&sign.access)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("anthropic-beta", "oauth-2025-04-20");
    if let Some(version) = &sign.version {
        request.header(
            "User-Agent",
            format!("claude-cli/{version} (external, cli)"),
        )
    } else {
        request
    }
}
fn same_account(a: &str, b: &str) -> bool {
    let split = |s: &str| {
        s.strip_prefix("anthropic:")
            .unwrap_or(s)
            .split_once('|')
            .map(|(a, b)| (a.to_owned(), b.to_owned()))
            .unwrap_or((s.into(), String::new()))
    };
    let (a, org_a) = split(a);
    let (b, org_b) = split(b);
    !a.is_empty() && a == b && (org_a.is_empty() || org_b.is_empty() || org_a == org_b)
}
fn organization(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}
impl Reader {
    async fn get(&self, program: &Program, force: bool) -> Option<(SignIn, Grants)> {
        let sign = sign_in(program).await?;
        let at = now();
        let mut cache = self.cache.lock().await;
        if cache.mark != sign.mark {
            *cache = Cached {
                mark: sign.mark.clone(),
                ..Default::default()
            };
        }
        if force || cache.tried == 0 || at.saturating_sub(cache.tried) >= HOUR {
            cache.tried = at;
            let response = async {
                let client = client().ok()?;
                let reply = headers(client.get(endpoint("oauth/usage")), &sign)
                    .query(&[("cedar_ember", "1"), ("skip_spend", "1")])
                    .send()
                    .await
                    .ok()?;
                if !reply.status().is_success() {
                    return None;
                }
                Some(parse(&reply.json::<Value>().await.ok()?["cedar_ember"]))
            }
            .await;
            if let Some(status) = response {
                cache.status = status;
            }
        }
        cache.status.clone().map(|g| (sign, g))
    }
    pub async fn limits(&self, program: &Program) -> Option<ResetInventory> {
        self.get(program, false).await.map(|(_, g)| g.view(now()))
    }
    pub async fn consume(
        &self,
        program: &Program,
        identity: &str,
        request: &str,
        target: &str,
    ) -> ResetOutcome {
        if target.is_empty()
            || target.len() > 40
            || !target
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'))
            || request.is_empty()
            || request.len() > 64
            || !request
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            return ResetOutcome::Failed;
        }
        let Some(sign) = sign_in(program).await else {
            return ResetOutcome::NoReset;
        };
        let Ok(client) = client() else {
            return ResetOutcome::NotNow;
        };
        let profile = async {
            let reply = headers(client.get(endpoint("oauth/profile")), &sign)
                .send()
                .await
                .ok()?;
            if !reply.status().is_success() {
                return None;
            }
            reply.json::<Value>().await.ok()
        }
        .await;
        let Some(profile) = profile else {
            return ResetOutcome::NotNow;
        };
        let Some(account) = text(&profile["account"]["uuid"]).map(|s| s.to_lowercase()) else {
            return ResetOutcome::NotNow;
        };
        let org = text(&profile["organization"]["uuid"])
            .unwrap_or_default()
            .to_lowercase();
        if !same_account(identity, &format!("anthropic:{account}|{org}")) {
            return ResetOutcome::NoReset;
        }
        let current = program.plan_sign_in().await.map(|(_, who)| who);
        if !current
            .as_deref()
            .is_some_and(|who| same_account(who, identity))
        {
            return ResetOutcome::NoReset;
        }
        if !organization(&org) {
            return ResetOutcome::Failed;
        }
        // The target and request are already selected by the resident journal.
        // A replay does not choose another grant, even if this one's now gone.
        let result = headers(
            client.post(endpoint(&format!("organizations/{org}/reset_rate_limits"))),
            &sign,
        )
        .timeout(Duration::from_secs(25))
        .json(&json!({"program":"cedar_ember","grant_id":target,"request_id":request}))
        .send()
        .await;
        let result = match result {
            Err(_) => ResetOutcome::Unknown,
            Ok(reply) => {
                let status = reply.status().as_u16();
                if !(200..300).contains(&status) {
                    if status >= 500 || matches!(status, 401 | 403 | 429) {
                        ResetOutcome::Unknown
                    } else {
                        ResetOutcome::MakerFailed
                    }
                } else {
                    let body = reply.json::<Value>().await.unwrap_or_default();
                    let code = if body["result"] == "unavailable" {
                        body["reason"].as_str()
                    } else {
                        body["result"].as_str()
                    };
                    match code {
                        Some("reset") => ResetOutcome::Reset,
                        Some("already_used") => ResetOutcome::AlreadyRedeemed,
                        Some("not_limited") => ResetOutcome::NothingToReset,
                        Some("ineligible" | "no_grant" | "unknown_grant" | "not_next_grant") => {
                            ResetOutcome::MakerNoReset
                        }
                        Some("reset_unconfirmed" | "stamp_indeterminate") => {
                            ResetOutcome::Unconfirmed
                        }
                        Some("cooldown" | "paused" | "low_priority_active") => {
                            ResetOutcome::MakerNotNow(date(&body["cooldown_until"]))
                        }
                        _ => ResetOutcome::Unknown,
                    }
                }
            }
        };
        if matches!(result, ResetOutcome::Reset | ResetOutcome::AlreadyRedeemed) {
            self.cache.lock().await.tried = 0;
        }
        result
    }
}
