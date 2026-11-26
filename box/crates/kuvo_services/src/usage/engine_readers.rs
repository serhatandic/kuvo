//! Read the plan of the engine's active credential. The engine renews its
//! own sign-in. No credential value is kept in the index or sent to a peer.
use super::{
    limits::{Account, Provider},
    plan_facts::{Amount, Balance, Extra, Facts, Problem, ProblemKind, Unit},
    readings::{Source, Window},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use jiff::{Span, Timestamp, tz::TimeZone};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const MIN: u64 = 60_000;
const TEN_MIN: u64 = 10 * MIN;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Integration {
    Openai,
    Xai,
    Copilot,
    Openrouter,
    Zai,
    Go,
}
impl Integration {
    fn of(id: &str) -> Option<Self> {
        Some(match id {
            "openai" => Self::Openai,
            "xai" => Self::Xai,
            "github-copilot" => Self::Copilot,
            "openrouter" => Self::Openrouter,
            "zai" | "zai-coding-plan" => Self::Zai,
            "opencode-go" => Self::Go,
            _ => return None,
        })
    }
    fn provider(self) -> Provider {
        match self {
            Self::Openai => Provider::Codex,
            Self::Xai => Provider::Grok,
            Self::Copilot => Provider::Copilot,
            Self::Openrouter => Provider::Openrouter,
            Self::Zai => Provider::Zai,
            Self::Go => Provider::Opencode,
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Credential {
    #[serde(rename = "integrationID")]
    integration_id: String,
    active: bool,
    value: SignIn,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignIn {
    #[serde(rename = "type", default)]
    kind: String,
    access: Option<String>,
    refresh: Option<String>,
    key: Option<String>,
    expires: Option<u64>,
    #[serde(default)]
    metadata: Value,
}
impl SignIn {
    fn oauth(&self) -> bool {
        self.kind == "oauth"
    }
    fn secret(&self, integration: Integration) -> Option<&str> {
        if integration == Integration::Copilot {
            self.refresh
                .as_deref()
                .filter(|s| !s.is_empty())
                .or(self.key.as_deref())
        } else {
            self.access
                .as_deref()
                .filter(|s| !s.is_empty())
                .or(self.key.as_deref())
        }
    }
}
/// Same credential admission for a read and for withdrawing cached rows. A
/// historical model call is never a credential, even if it used a plan.
fn admitted(credentials: &Value) -> Option<Vec<(Account, Integration, SignIn)>> {
    let list = credentials
        .as_array()
        .or_else(|| credentials["data"].as_array())?;
    Some(
        list.iter()
            .filter_map(|c| serde_json::from_value::<Credential>(c.clone()).ok())
            .filter(|c| c.active)
            .filter_map(|credential| {
                let kind = Integration::of(&credential.integration_id)?;
                let sign_in = credential.value;
                if matches!(kind, Integration::Openai | Integration::Xai)
                    && !sign_in.oauth()
                    && !sign_in.access.as_deref().is_some_and(|s| !s.is_empty())
                {
                    return None;
                }
                let secret = sign_in.secret(kind)?.trim();
                if secret.is_empty() {
                    return None;
                }
                let account = Account {
                    provider: credential.integration_id,
                    service: kind.provider(),
                    account: None,
                    label: None,
                    identity: identity(kind, &sign_in).map(|raw| Account::identity_of(&raw)),
                    sign_in: fingerprint(&format!("{}:{}", secret, sign_in.expires.unwrap_or(0))),
                    notice_card: None,
                };
                Some((account, kind, sign_in))
            })
            .collect(),
    )
}
pub fn admitted_accounts(credentials: &Value) -> Option<Vec<Account>> {
    Some(
        admitted(credentials)?
            .into_iter()
            .map(|(account, _, _)| account)
            .collect(),
    )
}
fn reset_sign_in(credentials: Value, account: &Account, now: u64) -> Option<SignIn> {
    let list = credentials
        .as_array()
        .or_else(|| credentials["data"].as_array())?;
    let expected = account.identity.as_ref()?;
    list.iter()
        .filter_map(|c| serde_json::from_value::<Credential>(c.clone()).ok())
        .find(|c| {
            c.active
                && c.integration_id == "openai"
                && c.value.oauth()
                && c.value.expires.is_none_or(|at| at > now)
                && identity(Integration::Openai, &c.value)
                    .is_some_and(|id| Account::identity_of(&id) == *expected)
        })
        .map(|c| c.value)
}
fn reset_headers(sign: &SignIn) -> Option<Vec<(&'static str, String)>> {
    let token = sign.access.as_deref()?.trim();
    if token.is_empty() {
        return None;
    }
    let mut headers = vec![
        ("Authorization", format!("Bearer {token}")),
        ("Accept", "application/json".into()),
        ("Content-Type", "application/json".into()),
        ("OpenAI-Beta", "codex-1".into()),
        ("originator", "Codex Desktop".into()),
    ];
    if let Some(id) = sign.metadata["accountID"]
        .as_str()
        .filter(|s| !s.is_empty())
    {
        headers.push(("ChatGPT-Account-Id", id.into()));
    }
    Some(headers)
}
fn target_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}
fn reset_time(v: &Value) -> Option<u64> {
    v.as_f64()
        .filter(|n| n.is_finite() && *n > 0.)
        .map(|n| {
            if n < 1e12 {
                (n * 1000.) as u64
            } else {
                n as u64
            }
        })
        .or_else(|| date(v))
}
fn available_credit(c: &Value) -> bool {
    c.is_object() && (c["status"].is_null() || c["status"] == "available")
}
fn reset_extra(v: &Value, now: u64) -> Option<Extra> {
    let count = v["available_count"]
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.)?
        .floor() as u32;
    let mut expiries = v["credits"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| available_credit(c))
        .filter_map(|c| reset_time(&c["expires_at"]))
        .filter(|at| *at > now)
        .collect::<Vec<_>>();
    expiries.sort();
    expiries.truncate(50);
    Some(Extra::Resets {
        id: "resets".into(),
        name: "Rate Limit Resets".into(),
        count,
        expiries,
        refills: vec![],
        usable: count > 0,
    })
}
pub(super) fn fingerprint(secret: &str) -> String {
    hex::encode(&Sha256::digest(secret.as_bytes())[..8])
}
pub(super) fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase)
}
pub(super) fn claims(token: Option<&str>) -> Option<Value> {
    let part = token?.split('.').nth(1)?;
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part.trim_end_matches('=')).ok()?).ok()
}
fn identity(kind: Integration, sign_in: &SignIn) -> Option<String> {
    match kind {
        Integration::Openai => {
            let auth = claims(sign_in.access.as_deref()).unwrap_or(Value::Null);
            let auth = &auth["https://api.openai.com/auth"];
            let account = text(&auth["chatgpt_account_id"])
                .or_else(|| text(&sign_in.metadata["accountID"]))?;
            let user = text(&auth["chatgpt_user_id"]).or_else(|| text(&auth["user_id"]));
            Some(format!(
                "chatgpt:{account}{}",
                user.map_or(String::new(), |u| format!("/{u}"))
            ))
        }
        Integration::Xai => {
            text(&claims(sign_in.access.as_deref())?["sub"]).map(|id| format!("xai:{id}"))
        }
        Integration::Copilot => None,
        _ => sign_in
            .key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(|key| format!("key:{key}")),
    }
}
pub(super) fn number(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        .filter(|v| v.is_finite())
}
pub(super) fn date(v: &Value) -> Option<u64> {
    let s = v.as_str()?;
    if let Ok(ts) = s.parse::<Timestamp>() {
        return ts.as_millisecond().try_into().ok();
    }
    let date = s.parse::<jiff::civil::Date>().ok()?;
    date.at(0, 0, 0, 0)
        .to_zoned(TimeZone::UTC)
        .ok()?
        .timestamp()
        .as_millisecond()
        .try_into()
        .ok()
}
pub(super) fn title(v: &Value) -> Option<String> {
    let s = v.as_str()?.trim();
    if s.is_empty() {
        return None;
    }
    Some(
        s.split(|c: char| c == '_' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .map(|s| {
                let mut chars = s.chars();
                let first = chars.next().unwrap();
                format!("{}{}", first.to_uppercase(), chars.as_str().to_lowercase())
            })
            .collect::<Vec<_>>()
            .join(" "),
    )
}
pub(super) fn window(
    id: &str,
    used: f64,
    minutes: Option<u32>,
    resets_at: Option<u64>,
    name: Option<&str>,
    model: Option<&str>,
    amount: Option<Amount>,
    now: u64,
) -> Window {
    Window {
        id: id.into(),
        used,
        minutes,
        resets_at,
        name: name.map(str::to_owned),
        model: model.map(str::to_owned),
        amount,
        read: now,
        source: Source::Usage,
    }
}
pub struct ReadPlan {
    pub account: Account,
    pub windows: Vec<Window>,
    pub facts: Facts,
}
pub struct ReadBatch {
    pub providers: Vec<String>,
    pub plans: Vec<ReadPlan>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Endpoint {
    Chatgpt,
    ChatgptResets,
    ChatgptConsume,
    Grok,
    Copilot,
    GithubOrgs,
    GithubBilling,
    OpenrouterKey,
    OpenrouterCredits,
    Zai,
    Go,
}
impl Endpoint {
    fn url(self) -> &'static str {
        match self {
            Self::Chatgpt => "https://chatgpt.com/backend-api/wham/usage",
            Self::ChatgptResets => "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits",
            Self::ChatgptConsume => {
                "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits/consume"
            }
            Self::Grok => "https://cli-chat-proxy.grok.com/v1/billing?format=credits",
            Self::Copilot => "https://api.github.com/copilot_internal/user",
            Self::GithubOrgs => "https://api.github.com/user/orgs?per_page=100",
            Self::GithubBilling => "https://api.github.com/orgs",
            Self::OpenrouterKey => "https://openrouter.ai/api/v1/key",
            Self::OpenrouterCredits => "https://openrouter.ai/api/v1/credits",
            Self::Zai => "https://api.z.ai/api/monitor/usage/quota/limit",
            Self::Go => "https://opencode.ai/zen/go/v1/usage",
        }
    }
    fn maker(self) -> &'static str {
        match self {
            Self::Chatgpt | Self::ChatgptResets | Self::ChatgptConsume => "ChatGPT",
            Self::Grok => "xAI",
            Self::Copilot | Self::GithubOrgs | Self::GithubBilling => "GitHub",
            Self::OpenrouterKey | Self::OpenrouterCredits => "OpenRouter",
            Self::Zai => "Z.ai",
            Self::Go => "OpenCode",
        }
    }
    fn test_name(self) -> &'static str {
        match self {
            Self::Chatgpt => "chatgpt",
            Self::ChatgptResets => "chatgpt/resets",
            Self::ChatgptConsume => "chatgpt/resets/consume",
            Self::Grok => "grok",
            Self::Copilot => "copilot",
            Self::GithubOrgs => "githubOrgs",
            Self::GithubBilling => "githubBilling",
            Self::OpenrouterKey => "openrouterKey",
            Self::OpenrouterCredits => "openrouterCredits",
            Self::Zai => "zai",
            Self::Go => "opencodeGo",
        }
    }
}
pub async fn kept_credentials(home: std::path::PathBuf) -> Result<Option<Value>, String> {
    tokio::task::spawn_blocking(move || {
        let root = std::env::var_os("KUVO_ENGINE_DATA")
            .filter(|value| !value.is_empty())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| home.join("data/opencode"));
        super::engine_index::credentials(&root).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}
#[derive(Clone)]
pub struct Reader {
    client: reqwest::Client,
    test_base: Option<url::Url>,
    orgs: std::sync::Arc<tokio::sync::Mutex<BTreeMap<String, Organization>>>,
}
#[derive(Clone)]
struct Organization {
    org: Option<String>,
    checked: u64,
    extras: Vec<Extra>,
}
impl Reader {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(15))
                .build()?,
            test_base: None,
            orgs: Default::default(),
        })
    }
    /// An isolated provider fixture can stand in for every maker. There is no production
    /// environment-based redirect to arbitrary endpoints carrying sign-in credentials.
    pub fn fixture(base: url::Url) -> Result<Self, String> {
        if base.scheme() != "http" || !matches!(base.host_str(), Some("127.0.0.1" | "::1")) {
            return Err("Usage fixtures must be loopback HTTP.".into());
        }
        let mut reader = Self::new().map_err(|e| e.to_string())?;
        reader.test_base = Some(base);
        Ok(reader)
    }
    fn url(&self, endpoint: Endpoint) -> String {
        self.test_base
            .as_ref()
            .and_then(|base| base.join(endpoint.test_name()).ok())
            .map_or_else(|| endpoint.url().into(), |u| u.into())
    }
    async fn get(
        &self,
        endpoint: Endpoint,
        url: Option<String>,
        headers: Vec<(&str, String)>,
        now: u64,
    ) -> Result<Value, Problem> {
        let url = url.unwrap_or_else(|| self.url(endpoint));
        let maker = endpoint.maker();
        let mut request = self.client.get(url).header("Accept", "application/json");
        for (header, value) in headers {
            request = request.header(header, value);
        }
        let response = request
            .send()
            .await
            .map_err(|_| Problem::new(ProblemKind::Failed, format!("Couldn't reach {maker}.")))?;
        let status = response.status().as_u16();
        if status == 429
            || (status == 403
                && endpoint.maker() == "GitHub"
                && (response.headers().contains_key("retry-after")
                    || response
                        .headers()
                        .get("x-ratelimit-remaining")
                        .is_some_and(|v| v == "0")))
        {
            let until = response
                .headers()
                .get("retry-after")
                .and_then(|h| h.to_str().ok())
                .and_then(|s| super::plan_facts::retry_at(s, now));
            return Err(Problem {
                kind: ProblemKind::RateLimited,
                message: format!("{maker} is turning away reads of the limits for now."),
                until,
                unknown: false,
            });
        }
        if status == 401 || status == 403 {
            let problem = if endpoint == Endpoint::Go
                && status == 403
                && response
                    .text()
                    .await
                    .unwrap_or_default()
                    .contains("EntitlementError")
            {
                Problem::new(ProblemKind::None, "No OpenCode Go plan on this key.")
            } else {
                Problem::new(
                    ProblemKind::Expired,
                    format!(
                        "{maker} turned the sign-in away: sign in again in Settings › Accounts if it keeps saying so."
                    ),
                )
            };
            return Err(problem);
        }
        if !(200..300).contains(&status) {
            return Err(Problem::new(
                ProblemKind::Failed,
                format!("{maker} answered {status}."),
            ));
        }
        response.json().await.map_err(|_| {
            Problem::new(
                ProblemKind::Failed,
                format!("{maker}'s answer wasn't what Kuvo Code reads."),
            )
        })
    }
    pub async fn read(
        &self,
        credentials: Value,
        now: u64,
        held: &BTreeSet<String>,
        selected: Option<&str>,
    ) -> Option<ReadBatch> {
        let mut returned = vec![];
        let mut providers = vec![];
        for (mut account, kind, sign_in) in admitted(&credentials)? {
            if held.contains(&account.provider) || selected.is_some_and(|id| id != account.provider)
            {
                providers.push(account.provider);
                continue;
            }
            let Some(secret) = sign_in.secret(kind).map(str::trim) else {
                continue;
            };
            let result = if kind != Integration::Copilot
                && sign_in.oauth()
                && sign_in.expires.is_some_and(|at| at <= now)
            {
                Err(Problem::new(
                    ProblemKind::Expired,
                    "Its sign-in has lapsed: the engine renews it the next time a model of it runs here.",
                ))
            } else {
                self.fetch_plan(kind, secret, &sign_in, now).await
            };
            let (windows, mut facts) = match result {
                Ok((windows, mut facts, who)) => {
                    if let Some(who) = who {
                        account.identity = Some(Account::identity_of(&who));
                    }
                    facts.checked = Some(now);
                    (windows, facts)
                }
                Err(problem) if problem.kind == ProblemKind::None => continue,
                Err(mut problem) => {
                    if problem.kind == ProblemKind::Expired && !sign_in.oauth() {
                        problem.kind = ProblemKind::Failed;
                        problem.message = problem
                            .message
                            .replace("the sign-in away: sign in again", "the key away: check it");
                    }
                    (
                        vec![],
                        Facts {
                            problem: Some(problem),
                            checked: Some(now),
                            ..Default::default()
                        },
                    )
                }
            };
            facts.next = Some(
                facts
                    .problem
                    .as_ref()
                    .and_then(|p| p.until)
                    .unwrap_or(now + TEN_MIN),
            );
            providers.push(account.provider.clone());
            returned.push(ReadPlan {
                account,
                windows,
                facts,
            });
        }
        Some(ReadBatch {
            providers,
            plans: returned,
        })
    }
    async fn fetch_plan(
        &self,
        kind: Integration,
        secret: &str,
        sign_in: &SignIn,
        now: u64,
    ) -> Result<(Vec<Window>, Facts, Option<String>), Problem> {
        let auth = || vec![("Authorization", format!("Bearer {secret}"))];
        match kind {
            Integration::Openai => {
                let mut headers = auth();
                if let Some(id) = sign_in.metadata["accountID"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                {
                    headers.push(("ChatGPT-Account-Id", id.to_owned()));
                }
                let body = self.get(Endpoint::Chatgpt, None, headers, now).await?;
                let (windows, mut facts, who) = chatgpt(&body, now);
                if body["rate_limit_reset_credits"]["available_count"]
                    .as_f64()
                    .is_some_and(|n| n > 0.)
                {
                    if let Some(headers) = reset_headers(sign_in) {
                        if let Ok(list) =
                            self.get(Endpoint::ChatgptResets, None, headers, now).await
                        {
                            if let Some(extra) = reset_extra(&list, now) {
                                facts.extras.retain(|e| !matches!(e, Extra::Resets { .. }));
                                facts.extras.push(extra);
                            }
                        }
                    }
                }
                Ok((windows, facts, who))
            }
            Integration::Xai => {
                let mut headers = auth();
                headers.push(("X-XAI-Token-Auth", "xai-grok-cli".into()));
                let body = self.get(Endpoint::Grok, None, headers, now).await?;
                grok(&body, now)
            }
            Integration::Copilot => {
                let body = self
                    .get(
                        Endpoint::Copilot,
                        None,
                        vec![
                            ("Authorization", format!("token {secret}")),
                            ("Editor-Version", "vscode/1.96.2".into()),
                            ("Editor-Plugin-Version", "copilot-chat/0.26.7".into()),
                            ("User-Agent", "GitHubCopilotChat/0.26.7".into()),
                            ("X-Github-Api-Version", "2025-04-01".into()),
                        ],
                        now,
                    )
                    .await?;
                let (mut windows, mut facts, identity) = copilot(&body, now);
                if windows.is_empty() && body["token_based_billing"] == true {
                    facts.extras = self.organization(secret, now).await;
                }
                Ok((std::mem::take(&mut windows), facts, identity))
            }
            Integration::Openrouter => {
                let body = self.get(Endpoint::OpenrouterKey, None, auth(), now).await?;
                let (windows, mut facts, identity) = router(&body, now);
                match self
                    .get(Endpoint::OpenrouterCredits, None, auth(), now)
                    .await
                {
                    Ok(credits) => {
                        let data = &credits["data"];
                        if let Some((total, spent)) =
                            number(&data["total_credits"]).zip(number(&data["total_usage"]))
                        {
                            facts.balance = Some(Balance {
                                left: (total - spent).max(0.0),
                                limit: (total > 0.0).then_some(total),
                                unit: Unit::Usd,
                            });
                        }
                    }
                    Err(e) if windows.is_empty() => return Err(e),
                    Err(_) => {}
                }
                Ok((windows, facts, identity))
            }
            Integration::Zai => zai(&self.get(Endpoint::Zai, None, auth(), now).await?, now),
            Integration::Go => go(&self.get(Endpoint::Go, None, auth(), now).await?, now),
        }
    }
    pub async fn choose_reset(
        &self,
        credentials: Value,
        account: &Account,
        now: u64,
    ) -> Result<String, super::resets::Outcome> {
        use super::resets::Outcome;
        let sign = reset_sign_in(credentials, account, now).ok_or(Outcome::NoReset)?;
        let headers = reset_headers(&sign).ok_or(Outcome::NoReset)?;
        let list = self
            .get(Endpoint::ChatgptResets, None, headers, now)
            .await
            .map_err(|_| Outcome::Failed)?;
        let mut credits = list["credits"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| {
                available_credit(c) && reset_time(&c["expires_at"]).is_none_or(|at| at > now)
            })
            .filter_map(|c| {
                let id = c["id"].as_str()?.trim();
                (!id.is_empty()).then_some((reset_time(&c["expires_at"]).unwrap_or(u64::MAX), id))
            })
            .collect::<Vec<_>>();
        credits.sort_by_key(|(at, _)| *at);
        let (_, target) = credits.first().ok_or(Outcome::NoReset)?;
        if !target_id(target) {
            return Err(Outcome::Failed);
        }
        Ok((*target).to_owned())
    }
    pub async fn consume_reset(
        &self,
        credentials: Value,
        account: &Account,
        request: &str,
        target: &str,
        now: u64,
    ) -> super::resets::Completion {
        use super::resets::{Completion, Outcome};
        if !target_id(target)
            || request.is_empty()
            || request.len() > 64
            || !request
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            return Outcome::Failed.into();
        }
        let Some(sign) = reset_sign_in(credentials, account, now) else {
            return Outcome::NoReset.into();
        };
        let Some(headers) = reset_headers(&sign) else {
            return Outcome::NoReset.into();
        };
        let mut call = self
            .client
            .post(self.url(Endpoint::ChatgptConsume))
            .timeout(std::time::Duration::from_secs(25))
            .json(&json!({"redeem_request_id":request,"credit_id":target}));
        for (key, value) in headers {
            call = call.header(key, value);
        }
        let Ok(reply) = call.send().await else {
            return Completion::maker(Outcome::Unknown);
        };
        let status = reply.status().as_u16();
        if !(200..300).contains(&status) {
            return Completion::maker(if status >= 500 || matches!(status, 401 | 403 | 429) {
                Outcome::Unknown
            } else {
                Outcome::Failed
            });
        }
        match reply
            .json::<Value>()
            .await
            .ok()
            .and_then(|v| v["code"].as_str().map(str::to_owned))
            .as_deref()
        {
            Some("reset") => Completion::maker(Outcome::Reset),
            Some("already_redeemed") => Completion {
                already: true,
                ..Completion::maker(Outcome::Reset)
            },
            Some("nothing_to_reset") => Completion::maker(Outcome::NothingToReset),
            Some("no_credit") => Completion::maker(Outcome::NoReset),
            _ => Completion::maker(Outcome::Unknown),
        }
    }
    async fn organization(&self, secret: &str, now: u64) -> Vec<Extra> {
        let key = fingerprint(secret);
        let prior = self.orgs.lock().await.get(&key).cloned();
        let headers = || {
            vec![
                ("Authorization", format!("token {secret}")),
                ("X-GitHub-Api-Version", "2022-11-28".into()),
                ("User-Agent", "Kuvo-Code".into()),
            ]
        };
        let read = |org: String| async move {
            let url = format!(
                "{}/{}/settings/billing/usage/summary",
                self.url(Endpoint::GithubBilling),
                url::form_urlencoded::byte_serialize(org.as_bytes()).collect::<String>()
            );
            self.get(Endpoint::GithubBilling, Some(url), headers(), now)
                .await
        };
        if let Some(prior) = prior.as_ref() {
            if let Some(org) = prior.org.as_ref() {
                match read(org.clone()).await {
                    Ok(body) => {
                        let extras = billing(&body);
                        if !extras.is_empty() {
                            self.orgs.lock().await.insert(
                                key.clone(),
                                Organization {
                                    org: Some(org.clone()),
                                    checked: now,
                                    extras: extras.clone(),
                                },
                            );
                            return extras;
                        }
                    }
                    Err(e) if matches!(e.kind, ProblemKind::Failed | ProblemKind::RateLimited) => {
                        return prior.extras.clone();
                    }
                    Err(_) => {}
                }
            } else if now.saturating_sub(prior.checked) < 6 * 60 * MIN {
                return vec![];
            }
        }
        let Ok(orgs) = self.get(Endpoint::GithubOrgs, None, headers(), now).await else {
            return prior.map_or(vec![], |p| p.extras);
        };
        for org in orgs
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|o| o["login"].as_str().map(str::to_owned))
            .take(30)
        {
            match read(org.clone()).await {
                Ok(body) => {
                    let extras = billing(&body);
                    if !extras.is_empty() {
                        self.orgs.lock().await.insert(
                            key,
                            Organization {
                                org: Some(org),
                                checked: now,
                                extras: extras.clone(),
                            },
                        );
                        return extras;
                    }
                }
                Err(e) if matches!(e.kind, ProblemKind::Failed | ProblemKind::RateLimited) => {
                    return prior.map_or(vec![], |p| p.extras);
                }
                Err(_) => {}
            }
        }
        self.orgs.lock().await.insert(
            key,
            Organization {
                org: None,
                checked: now,
                extras: vec![],
            },
        );
        vec![]
    }
}
fn chatgpt(body: &Value, now: u64) -> (Vec<Window>, Facts, Option<String>) {
    let mut windows = vec![];
    let mut add = |limit: &Value, prefix: &str, model: Option<&str>| {
        for which in ["primary", "secondary"] {
            let w = &limit[format!("{which}_window")];
            let Some(used) = number(&w["used_percent"]) else {
                continue;
            };
            let minutes = number(&w["limit_window_seconds"])
                .filter(|n| *n > 0.0)
                .map(|n| (n / 60.0).round() as u32);
            let reset = number(&w["reset_at"])
                .map(|n| (n * 1000.0) as u64)
                .or_else(|| {
                    number(&w["reset_after_seconds"])
                        .map(|n| now.saturating_add((n * 1000.0) as u64))
                });
            windows.push(window(
                &format!("{prefix}{which}"),
                used / 100.0,
                minutes,
                reset,
                None,
                model,
                None,
                now,
            ));
        }
    };
    add(&body["rate_limit"], "", None);
    for extra in body["additional_rate_limits"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let name = extra["limit_name"]
            .as_str()
            .or_else(|| extra["metered_feature"].as_str());
        if let Some(id) = extra["limit_id"]
            .as_str()
            .or_else(|| extra["metered_feature"].as_str())
            .or(name)
        {
            add(&extra["rate_limit"], &format!("{id}."), name);
        }
    }
    let mut extras = vec![];
    let credits = &body["credits"];
    if credits.is_object() && credits["unlimited"] != true {
        if let Some(left) = number(&credits["balance"]).or_else(|| {
            (credits["has_credits"] == false || credits["hasCredits"] == false).then_some(0.)
        }) {
            extras.push(Extra::Left {
                id: "credits".into(),
                name: "Extra Usage".into(),
                amount: left.max(0.).floor(),
                unit: Unit::Credits,
                resets_at: None,
            });
        }
    }
    if let Some(extra) = reset_extra(&body["rate_limit_reset_credits"], now) {
        extras.push(extra);
    }
    (
        windows,
        Facts {
            plan: title(&body["plan_type"]),
            extras,
            ..Default::default()
        },
        None,
    )
}
pub(super) fn grok(
    body: &Value,
    now: u64,
) -> Result<(Vec<Window>, Facts, Option<String>), Problem> {
    let config = &body["config"];
    if config.as_object().is_some_and(|c| {
        !c.keys().any(|key| {
            ["period", "percent", "limit"]
                .iter()
                .any(|word| key.to_ascii_lowercase().contains(word))
        })
    }) {
        return Err(Problem::new(
            ProblemKind::Ended,
            "Its SuperGrok plan isn't active now.",
        ));
    }
    let start = date(&config["currentPeriod"]["start"]);
    let end = date(&config["currentPeriod"]["end"]);
    let Some((start, end)) = start.zip(end).filter(|(a, b)| b > a) else {
        return Err(Problem::new(
            ProblemKind::Failed,
            "xAI's answer wasn't what Kuvo Code reads.",
        ));
    };
    Ok((
        vec![window(
            "weekly",
            number(&config["creditUsagePercent"]).unwrap_or(0.0) / 100.0,
            Some(((end - start) as f64 / MIN as f64).round() as u32),
            Some(end),
            None,
            None,
            None,
            now,
        )],
        Facts::default(),
        None,
    ))
}
fn copilot(body: &Value, now: u64) -> (Vec<Window>, Facts, Option<String>) {
    let reset = date(&body["quota_reset_date"]).or_else(|| date(&body["limited_user_reset_date"]));
    let mut windows = vec![];
    for (id, name) in [
        ("premium_interactions", "Premium requests"),
        ("chat", "Chat"),
        ("completions", "Completions"),
    ] {
        let snapshot = &body["quota_snapshots"][id];
        if snapshot["unlimited"] == true {
            continue;
        }
        let limit = number(&snapshot["entitlement"])
            .filter(|n| *n > 0.0)
            .or_else(|| number(&body["monthly_quotas"][id]).filter(|n| *n > 0.0));
        let Some(limit) = limit else {
            continue;
        };
        let remaining =
            number(&snapshot["remaining"]).or_else(|| number(&body["limited_user_quotas"][id]));
        if remaining == Some(-1.0) {
            continue;
        }
        let Some(used) = number(&snapshot["percent_remaining"])
            .map(|n| 1.0 - n / 100.0)
            .or_else(|| remaining.map(|n| 1.0 - n / limit))
        else {
            continue;
        };
        windows.push(window(
            id,
            used,
            Some(43200),
            reset,
            Some(name),
            None,
            remaining.map(|n| Amount {
                used: (limit - n).max(0.0),
                limit,
                unit: Unit::Requests,
            }),
            now,
        ));
    }
    (
        windows,
        Facts {
            plan: title(&body["copilot_plan"]),
            ..Default::default()
        },
        text(&body["login"]).map(|name| format!("github:{name}")),
    )
}
fn billing(body: &Value) -> Vec<Extra> {
    let items: Vec<_> = body["usageItems"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| {
            text(&item["product"]).as_deref() == Some("copilot")
                && matches!(
                    text(&item["unitType"]).as_deref(),
                    Some("ai-credits" | "ai-units")
                )
        })
        .collect();
    if items.is_empty() {
        return vec![];
    }
    let sum = |name: &str| {
        items
            .iter()
            .map(|item| number(&item[name]).unwrap_or(0.0).max(0.0))
            .sum()
    };
    vec![
        Extra::Spent {
            id: "org_credits".into(),
            name: "Org Credits".into(),
            amount: sum("grossQuantity"),
            unit: Unit::Credits,
            resets_at: None,
        },
        Extra::Spent {
            id: "org_spend".into(),
            name: "Org Spend".into(),
            amount: sum("netAmount"),
            unit: Unit::Usd,
            resets_at: None,
        },
    ]
}
fn router(body: &Value, now: u64) -> (Vec<Window>, Facts, Option<String>) {
    let data = &body["data"];
    let mut windows = vec![];
    if let Some(limit) = number(&data["limit"]).filter(|n| *n > 0.0) {
        let left = number(&data["limit_remaining"])
            .unwrap_or_else(|| limit - number(&data["usage"]).unwrap_or(0.0))
            .max(0.0);
        windows.push(window(
            "key",
            1.0 - left / limit,
            None,
            None,
            Some("Key limit"),
            None,
            Some(Amount {
                used: limit - left,
                limit,
                unit: Unit::Usd,
            }),
            now,
        ));
    }
    let mut extras = vec![];
    let day = Timestamp::from_millisecond(now as i64)
        .ok()
        .map(|t| t.to_zoned(TimeZone::UTC).date());
    for (field, id, name) in [
        ("usage_daily", "today", "Today"),
        ("usage_weekly", "week", "This Week"),
        ("usage_monthly", "month", "This Month"),
    ] {
        let Some(amount) = number(&data[field]) else {
            continue;
        };
        let reset = day.and_then(|d| {
            let end = match id {
                "today" => d.checked_add(Span::new().days(1)).ok()?,
                "week" => d
                    .checked_add(Span::new().days(7 - d.weekday().to_monday_zero_offset()))
                    .ok()?,
                _ => d.first_of_month().checked_add(Span::new().months(1)).ok()?,
            };
            end.at(0, 0, 0, 0)
                .to_zoned(TimeZone::UTC)
                .ok()?
                .timestamp()
                .as_millisecond()
                .try_into()
                .ok()
        });
        extras.push(Extra::Spent {
            id: id.into(),
            name: name.into(),
            amount: amount.max(0.0),
            unit: Unit::Usd,
            resets_at: reset,
        });
    }
    (
        windows,
        Facts {
            plan: data["is_free_tier"].as_bool().map(|free| {
                if free {
                    "Free tier".into()
                } else {
                    "Pay as you go".into()
                }
            }),
            extras,
            ..Default::default()
        },
        None,
    )
}
fn zai(body: &Value, now: u64) -> Result<(Vec<Window>, Facts, Option<String>), Problem> {
    if body["success"] == false
        && body["msg"]
            .as_str()
            .is_some_and(|s| s.to_ascii_lowercase().contains("coding plan"))
    {
        return Err(Problem::new(
            ProblemKind::None,
            "No coding plan on this key.",
        ));
    }
    let Some(limits) = body["data"]["limits"]
        .as_array()
        .or_else(|| body["limits"].as_array())
    else {
        return Err(Problem::new(
            ProblemKind::Failed,
            "Z.ai's answer wasn't what Kuvo Code reads.",
        ));
    };
    let mut windows = vec![];
    for entry in limits {
        let reset = number(&entry["nextResetTime"]).map(|n| n as u64);
        match entry["type"].as_str().or_else(|| entry["name"].as_str()) {
            Some("CREDIT_LIMIT" | "TOKENS_LIMIT") => {
                let unit = match number(&entry["unit"]).map(|n| n as u32) {
                    Some(3) => 60,
                    Some(4) => 1440,
                    Some(5) => 43200,
                    Some(6) => 10080,
                    _ => continue,
                };
                let Some(count) = number(&entry["number"]).filter(|n| *n > 0.0) else {
                    continue;
                };
                let minutes = (unit as f64 * count) as u32;
                let Some(used) = number(&entry["percentage"]) else {
                    continue;
                };
                let id = if minutes < 1440 {
                    "session".into()
                } else {
                    format!("window_{minutes}")
                };
                windows.push(window(
                    &id,
                    used / 100.0,
                    Some(minutes),
                    reset,
                    None,
                    None,
                    None,
                    now,
                ));
            }
            Some("TIME_LIMIT") => {
                let Some((used, limit)) = number(&entry["currentValue"])
                    .zip(number(&entry["usage"]).filter(|n| *n > 0.0))
                else {
                    continue;
                };
                windows.push(window(
                    "web_search",
                    used / limit,
                    None,
                    reset,
                    Some("Web searches"),
                    None,
                    Some(Amount {
                        used,
                        limit,
                        unit: Unit::Requests,
                    }),
                    now,
                ));
            }
            _ => {}
        }
    }
    Ok((windows, Facts::default(), None))
}
fn go(body: &Value, now: u64) -> Result<(Vec<Window>, Facts, Option<String>), Problem> {
    let usage = &body["usage"];
    if !usage.is_object() {
        return Err(Problem::new(
            ProblemKind::None,
            "No OpenCode Go plan on this key.",
        ));
    }
    let mut windows = vec![];
    for (id, minutes) in [("rolling", 300), ("weekly", 10080), ("monthly", 43200)] {
        if let Some(percent) = number(&usage[id]["percent"]) {
            let reset =
                date(&usage[id]["resetsAt"]).filter(|at| id != "rolling" || *at < now + 299 * MIN);
            windows.push(window(
                id,
                percent / 100.0,
                Some(minutes),
                reset,
                None,
                None,
                None,
                now,
            ));
        }
    }
    Ok((
        windows,
        Facts {
            plan: Some("Go".into()),
            ..Default::default()
        },
        None,
    ))
}
