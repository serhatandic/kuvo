use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ed25519_dalek::{Signer, SigningKey};
use kuvo_wire::keys::encode;
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use crate::{
    Error,
    model::{Identity, Link, Role, SharedKind},
    storage::Files,
};

const RECORD: &str = "run/setup-relay-lease.json";
const DOMAIN: &[u8] = b"kuvo-link setup lease v1\0";
const MAX_TIME: u64 = 120_000;

pub(crate) fn wall() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) struct Timestamp {
    wall: u64,
    uptime: f64,
}

pub(crate) struct Clock {
    started: Instant,
    offset_file: Option<Files>,
}

impl Clock {
    fn now(&self) -> Timestamp {
        let offset = self
            .offset_file
            .as_ref()
            .and_then(|files| {
                files
                    .read::<serde_json::Value>("run/test-setup-clock.json")
                    .ok()
                    .flatten()
                    .and_then(|file| file.get("offset").and_then(serde_json::Value::as_i64))
            })
            .unwrap_or_default();
        Timestamp {
            wall: wall().saturating_add_signed(offset),
            uptime: self.started.elapsed().as_secs_f64() * 1000.0 + offset as f64,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Observation {
    pub setup: String,
    pub holder: String,
    pub sign: String,
    pub term: u64,
    pub expires: u64,
    pub high: u64,
    number_floor: Option<u64>,
    pub relay_host: bool,
    since: Timestamp,
    valid_after: Timestamp,
    duration: u64,
}

impl Observation {
    fn fresh(&self, clock: &Clock) -> bool {
        let now = clock.now();
        [self.since, self.valid_after].iter().all(|start| {
            now.wall
                .checked_sub(start.wall)
                .is_some_and(|age| age < self.duration)
                && (0.0..self.duration as f64).contains(&(now.uptime - start.uptime))
        })
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Action {
    Read,
    Claim,
    Renew,
    Transfer,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Request<'a> {
    action: Action,
    setup: &'a str,
    computer: &'a str,
    sign: &'a str,
    at: u64,
    term: u64,
    bootstrap: bool,
    always_on: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    high: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    roster: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Deserialize)]
struct Lease {
    holder: String,
    sign: String,
    term: u64,
    expires: u64,
    high: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reply {
    lease: Option<Lease>,
    #[serde(default)]
    remaining: u64,
    #[serde(default)]
    starts_in: u64,
    #[serde(default)]
    takeover_in: u64,
    #[serde(default)]
    relay_host: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TakeoverSettings {
    always_on: Option<bool>,
}

pub(crate) struct Client {
    http: reqwest::Client,
    clock: Clock,
    pub seen: Option<Observation>,
    observed_here: bool,
    always_on: bool,
}

impl Client {
    pub fn new(files: &Files, link: &Link, test_clock: bool) -> Result<Self, Error> {
        let settings = files
            .read::<TakeoverSettings>("setup-takeover.json")
            .ok()
            .flatten();
        let seen = files.read::<Observation>(RECORD)?.filter(|lease| {
            link.setup
                .as_ref()
                .is_some_and(|s| s.position.id == lease.setup)
        });
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(4))
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .build()?,
            clock: Clock {
                started: Instant::now(),
                offset_file: test_clock.then(|| files.clone()),
            },
            seen,
            observed_here: false,
            always_on: settings
                .and_then(|settings| settings.always_on)
                .unwrap_or(cfg!(target_os = "linux")),
        })
    }

    pub fn holds(&self, link: &Link) -> bool {
        self.observed_here
            && self.seen.as_ref().is_some_and(|lease| {
                link.setup().is_ok_and(|setup| {
                    setup.position.id == lease.setup
                        && setup.position.primary == lease.holder
                        && setup.sign == lease.sign
                        && lease.fresh(&self.clock)
                })
            })
    }

    pub fn can_promote(&self, identity: &Identity) -> bool {
        self.observed_here
            && self.seen.as_ref().is_some_and(|lease| {
                lease.holder == identity.public
                    && lease.sign == identity.sign
                    && lease.fresh(&self.clock)
            })
    }

    pub async fn request(
        &mut self,
        files: &Files,
        link: &Link,
        identity: &Identity,
        action: Action,
        term: u64,
        target: Option<&str>,
    ) -> Result<Option<(Observation, u64)>, Error> {
        let setup = link.setup()?;
        let relay = link
            .relay
            .as_deref()
            .ok_or(Error::Invalid("missing setup relay"))?;
        let mut url = reqwest::Url::parse(relay).map_err(|_| Error::Invalid("setup relay URL"))?;
        // Rebuild from the relay authority because the url crate does not allow replacing every scheme.
        url = match url.scheme() {
            "ws" => reqwest::Url::parse(&format!("http{}", &url.as_str()[2..])),
            "wss" => reqwest::Url::parse(&format!("https{}", &url.as_str()[3..])),
            _ => return Err(Error::Invalid("setup relay URL scheme")),
        }
        .map_err(|_| Error::Invalid("setup relay URL"))?;
        url.set_path("/setup-lease");
        url.set_query(None);
        url.set_fragment(None);
        let bootstrap = matches!(action, Action::Claim) && term == 0 && setup.role == Role::Primary;
        let roster = if matches!(action, Action::Renew | Action::Transfer) || bootstrap {
            Some(
                link.member_list()?
                    .boxes
                    .into_iter()
                    .filter_map(|peer| Some((peer.key, peer.sign?)))
                    .collect(),
            )
        } else {
            None
        };

        let next = link
            .value(SharedKind::QueueNumberAllocation)
            .and_then(|value| value.get("next"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
            .saturating_sub(1);
        let old = setup
            .extra
            .get("numberCeiling")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let (known, requested) = if matches!(action, Action::Read) {
            (0, 0)
        } else {
            number_high(files, &setup.position.id)?
        };
        let high = next.max(old).max(known);
        let outgoing = match action {
            Action::Read => None,
            Action::Renew => {
                let floor = self
                    .seen
                    .as_ref()
                    .map_or(0, |l| l.number_floor.unwrap_or(l.high));
                Some(
                    high.max(floor)
                        .saturating_add(200)
                        .max(requested)
                        .min(999_999),
                )
            }
            _ => Some(high),
        };
        let since = self.clock.now();
        let body = serde_json::to_vec(&Request {
            action,
            setup: &setup.position.id,
            computer: &identity.public,
            sign: &identity.sign,
            at: wall(),
            term,
            bootstrap,
            always_on: self.always_on,
            target,
            high: outgoing,
            roster,
        })
        .expect("lease request serializes");
        let signature = SigningKey::from_bytes(&identity.seed).sign(&[DOMAIN, &body].concat());
        let reply: Reply = self
            .http
            .post(url)
            .json(&serde_json::json!({
                "data": encode(&body), "signature": encode(&signature.to_bytes()),
            }))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let Some(lease) = reply.lease else {
            return Ok(None);
        };
        if lease.term == 0
            || kuvo_wire::keys::decode_exact::<32>(&lease.holder).is_none()
            || kuvo_wire::keys::decode_exact::<32>(&lease.sign).is_none()
        {
            return Err(Error::Invalid("relay lease reply"));
        }
        let mut valid_after = since;
        if reply.starts_in > 0 {
            let received = self.clock.now();
            valid_after = Timestamp {
                wall: received.wall.saturating_add(reply.starts_in),
                uptime: received.uptime + reply.starts_in as f64,
            };
        }
        let saved = self
            .seen
            .as_ref()
            .filter(|saved| saved.setup == setup.position.id);
        if saved.is_some_and(|saved| {
            saved.term > lease.term
                || saved.term == lease.term
                    && (saved.expires > lease.expires || saved.since.wall > since.wall)
        }) {
            return Err(Error::Conflict);
        }
        let number_floor = saved
            .filter(|saved| saved.term == lease.term)
            .map_or(lease.high, |saved| {
                saved.number_floor.unwrap_or(saved.high.max(high))
            });
        let observed = Observation {
            setup: setup.position.id.clone(),
            holder: lease.holder,
            sign: lease.sign,
            term: lease.term,
            expires: lease.expires,
            high: lease.high,
            number_floor: Some(number_floor),
            relay_host: reply.relay_host,
            since,
            valid_after,
            duration: reply.remaining.min(MAX_TIME),
        };
        files.write(RECORD, &observed)?;
        self.seen = Some(observed.clone());
        self.observed_here = true;
        Ok(Some((observed, reply.takeover_in)))
    }

    pub fn expired(observation: &Observation) -> bool {
        observation.duration == 0
    }
}

fn number_high(files: &Files, setup: &str) -> Result<(u64, u64), Error> {
    use serde_json::Value;
    let ledger = files
        .read::<Value>("run/work-queue/numbers.json")?
        .unwrap_or(Value::Null);
    if ledger
        .get("formatVersion")
        .is_some_and(|version| !matches!(version.as_f64(), Some(0.0 | 1.0)))
    {
        return Err(Error::Version("Work Queue ledger"));
    }
    let mut high = ledger["next"].as_u64().unwrap_or(1).saturating_sub(1);
    for field in ["leases", "used"] {
        if let Some(ranges) = ledger[field].as_array() {
            high = ranges
                .iter()
                .filter_map(|range| range.get(1).and_then(Value::as_u64))
                .fold(high, u64::max);
        }
    }
    high = high
        .max(
            ledger["quorum"]["votes"][setup]["to"]
                .as_u64()
                .unwrap_or_default(),
        )
        .max(
            ledger["quorum"]["observed"][setup]
                .as_u64()
                .unwrap_or_default(),
        );
    for folder in ["run/work-queue/queues", "run/work-queue/peers"] {
        for file in files.names(folder)? {
            if !file.ends_with(".json") {
                continue;
            }
            if let Some(row) = files.read::<Value>(&format!("{folder}/{file}"))? {
                high = high
                    .max(row["number"].as_u64().unwrap_or_default())
                    .max(row["top"].as_u64().unwrap_or_default());
                if let Some(items) = row["items"].as_array() {
                    high = items
                        .iter()
                        .filter_map(|item| item["number"].as_u64())
                        .fold(high, u64::max);
                }
            }
        }
    }
    Ok((
        high.min(999_999),
        ledger["reserveThrough"].as_u64().unwrap_or_default(),
    ))
}
