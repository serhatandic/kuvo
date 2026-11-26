//! Public price lists are fetched off the request path. A bad or oversized
//! response leaves the last good compact catalog in place.
use super::pricing::Rates;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};
const HOUR: u64 = 3_600_000;
const RETRY: u64 = HOUR / 2;
const MAX: usize = 64 * 1024 * 1024;
#[derive(Clone, Copy, Debug, Ord, PartialOrd, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Litellm,
    ModelsDev,
    Supplement,
}
impl Source {
    const ALL: [Self; 3] = [Self::Litellm, Self::ModelsDev, Self::Supplement];
    fn name(self) -> &'static str {
        match self {
            Self::Litellm => "litellm",
            Self::ModelsDev => "models_dev",
            Self::Supplement => "supplement",
        }
    }
    fn url(self) -> &'static str {
        match self {
            Self::Litellm => {
                "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json"
            }
            Self::ModelsDev => "https://models.dev/api.json",
            Self::Supplement => "https://robinebers.github.io/openusage/pricing_supplement.json",
        }
    }
}
fn money(v: &Value) -> Option<f64> {
    v.as_f64().filter(|n| n.is_finite() && *n >= 0.)
}
fn rates(input: f64, output: f64, cw: Option<f64>, cr: Option<f64>) -> Rates {
    Rates {
        input,
        output,
        cache_write: cw.unwrap_or(input),
        cache_read: cr.unwrap_or(input * 0.1),
        input_above: None,
        output_above: None,
        cache_write_above: None,
        cache_read_above: None,
        cache_read_explicit: cr.is_some(),
        fast: 1.,
    }
}
fn compact(source: Source, body: &Value) -> Result<Value, String> {
    let root = body.as_object().ok_or("Price feed is not an object")?;
    if source == Source::Supplement {
        super::pricing::validate_supplement(body)?;
        return Ok(body.clone());
    }
    let mut models = BTreeMap::new();
    if source == Source::Litellm {
        for (id, item) in root {
            let rate = |name: &str| {
                money(&item[name])
                    .map(|v| v * 1e6)
                    .filter(|v| v.is_finite())
            };
            let (Some(input), Some(output)) =
                (rate("input_cost_per_token"), rate("output_cost_per_token"))
            else {
                continue;
            };
            let mut r = rates(
                input,
                output,
                rate("cache_creation_input_token_cost"),
                rate("cache_read_input_token_cost"),
            );
            r.input_above = rate("input_cost_per_token_above_200k_tokens");
            r.output_above = rate("output_cost_per_token_above_200k_tokens");
            r.cache_write_above = rate("cache_creation_input_token_cost_above_200k_tokens");
            r.cache_read_above = rate("cache_read_input_token_cost_above_200k_tokens");
            r.fast = money(&item["provider_specific_entry"]["fast"]).unwrap_or(1.);
            models.insert(id.clone(), r);
        }
    } else {
        let mut providers = root.iter().collect::<Vec<_>>();
        providers.sort_by(|(a, _), (b, _)| a.cmp(b));
        for (_, provider) in providers {
            let Some(items) = provider["models"].as_object() else {
                continue;
            };
            for (id, item) in items {
                let cost = &item["cost"];
                let (Some(input), Some(output)) = (money(&cost["input"]), money(&cost["output"]))
                else {
                    continue;
                };
                models.entry(id.clone()).or_insert_with(|| {
                    rates(
                        input,
                        output,
                        money(&cost["cache_write"]),
                        money(&cost["cache_read"]),
                    )
                });
            }
        }
    }
    if models.is_empty() {
        return Err("The price feed contains no usable model entries".into());
    }
    Ok(json!({"models":models}))
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct State {
    #[serde(skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fetched_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failed_at: Option<u64>,
}
impl State {
    fn next(&self) -> u64 {
        self.failed_at
            .map(|at| at.saturating_add(RETRY))
            .filter(|at| *at > self.fetched_at.unwrap_or(0))
            .unwrap_or_else(|| self.fetched_at.map_or(0, |at| at.saturating_add(HOUR)))
    }
}
pub struct Feeds {
    folder: PathBuf,
    client: reqwest::Client,
    states: BTreeMap<Source, State>,
    fixture: Option<url::Url>,
}
impl Feeds {
    pub fn for_home(home: &Path, personal: &Path) -> Option<Self> {
        // No real URL is used in a scratch home. Tests can explicitly supply
        // a loopback price server rather than changing the real feed addresses.
        let fixture = std::env::var("KUVO_TEST_PRICE_BASE")
            .ok()
            .and_then(|v| v.parse::<url::Url>().ok())
            .filter(|u| u.scheme() == "http" && matches!(u.host_str(), Some("127.0.0.1" | "::1")));
        let live = home == personal.join(".kuvo-code")
            && std::env::var("KUVO_TEST_PRICE_FEEDS").as_deref() != Ok("off")
            && std::env::var("KUVO_TEST_PATH_ONLY").as_deref() != Ok("1");
        if !live && fixture.is_none() {
            return None;
        }
        Self::new(home.join("agents/usage/pricing"), fixture).ok()
    }
    pub fn new(folder: PathBuf, fixture: Option<url::Url>) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()?;
        let states = kuvo_store::files::read(&folder.join("state.json"))
            .ok()
            .flatten()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Ok(Self {
            folder,
            client,
            states,
            fixture,
        })
    }
    pub async fn refresh_due(&mut self, now: u64) -> bool {
        let mut changed = false;
        let mut checked = false;
        for source in Source::ALL {
            let state = self.states.get(&source).cloned().unwrap_or_default();
            if state.next() > now {
                continue;
            }
            checked = true;
            match self.fetch(source, &state, now).await {
                Ok((next, updated)) => {
                    self.states.insert(source, next);
                    changed |= updated;
                }
                Err(_) => {
                    self.states.insert(
                        source,
                        State {
                            failed_at: Some(now),
                            ..state
                        },
                    );
                }
            }
        }
        if checked {
            if let Ok(bytes) = serde_json::to_vec(&self.states) {
                let _ = kuvo_store::files::write_secret(&self.folder.join("state.json"), &bytes);
            }
        }
        changed
    }
    pub fn next(&self) -> u64 {
        Source::ALL
            .into_iter()
            .map(|s| self.states.get(&s).map_or(0, State::next))
            .min()
            .unwrap_or(0)
    }
    async fn fetch(
        &self,
        source: Source,
        state: &State,
        now: u64,
    ) -> Result<(State, bool), Box<dyn std::error::Error + Send + Sync>> {
        let path = self.folder.join(format!("{}.json", source.name()));
        let endpoint = self
            .fixture
            .as_ref()
            .map(|u| u.join(source.name()))
            .transpose()?
            .unwrap_or_else(|| source.url().parse().expect("feed URL"));
        let mut request = self.client.get(endpoint);
        if let Some(tag) = state.etag.as_ref().filter(|_| path.is_file()) {
            request = request.header(reqwest::header::IF_NONE_MATCH, tag);
        }
        let mut response = request.send().await?;
        if response.status() == reqwest::StatusCode::NOT_MODIFIED && path.is_file() {
            return Ok((
                State {
                    fetched_at: Some(now),
                    failed_at: None,
                    ..state.clone()
                },
                false,
            ));
        }
        response = response.error_for_status()?;
        if response.status() != reqwest::StatusCode::OK
            || response.content_length().is_some_and(|n| n > MAX as u64)
        {
            return Err("Invalid price feed response".into());
        }
        let tag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let mut bytes = vec![];
        while let Some(chunk) = response.chunk().await? {
            if bytes.len().saturating_add(chunk.len()) > MAX {
                return Err("Price feed too large".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        tokio::task::spawn_blocking(
            move || -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
                let body: Value = serde_json::from_slice(&bytes)?;
                let catalog = compact(source, &body).map_err(std::io::Error::other)?;
                kuvo_store::files::write_secret(&path, &serde_json::to_vec(&catalog)?)?;
                Ok(())
            },
        )
        .await??;
        Ok((
            State {
                etag: tag,
                fetched_at: Some(now),
                failed_at: None,
            },
            true,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test]
    async fn hourly_feeds_reuse_their_saved_etags_and_do_not_fetch_early() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let received = requests.clone();
        let server = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut bytes = Vec::new();
                let mut chunk = [0; 1024];
                while !bytes.ends_with(b"\r\n\r\n") {
                    let n = stream.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                }
                let text = String::from_utf8_lossy(&bytes);
                let path = text
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .to_owned();
                let cached = text
                    .to_ascii_lowercase()
                    .lines()
                    .any(|line| line == "if-none-match: fixture-price");
                received.lock().unwrap().push((path.clone(), cached));
                let body=match path.as_str() {
                    "/litellm"=>json!({"fixture":{"input_cost_per_token":0.000001,"output_cost_per_token":0.000002}}),
                    "/models_dev"=>json!({"fixture":{"models":{"fixture":{"cost":{"input":1,"output":2}}}}}),
                    "/supplement"=>json!({"updated_at":"2099-01-01","pricing":{},"alias_rules":[]}),
                    _=>unreachable!(),
                }.to_string();
                let response = if cached {
                    "HTTP/1.1 304 Not Modified\r\nConnection: close\r\n\r\n".into()
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nETag: fixture-price\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                };
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let home = tempfile::tempdir().unwrap();
        let fixture = Some(url::Url::parse(&format!("http://{address}/")).unwrap());
        let at = 10 * HOUR;
        let mut feeds = Feeds::new(home.path().into(), fixture.clone()).unwrap();
        assert!(feeds.refresh_due(at).await);
        assert_eq!(requests.lock().unwrap().len(), 3);
        assert_eq!(feeds.next(), at + HOUR);
        assert!(!feeds.refresh_due(at + HOUR - 1).await);
        assert_eq!(requests.lock().unwrap().len(), 3);
        drop(feeds);
        let mut reopened = Feeds::new(home.path().into(), fixture).unwrap();
        assert_eq!(reopened.next(), at + HOUR);
        assert!(!reopened.refresh_due(at + HOUR).await);
        let seen = requests.lock().unwrap();
        assert_eq!(seen.len(), 6);
        assert!(seen[3..].iter().all(|(_, etag)| *etag));
        drop(seen);
        assert_eq!(reopened.next(), at + 2 * HOUR);
        server.abort();
    }
}
