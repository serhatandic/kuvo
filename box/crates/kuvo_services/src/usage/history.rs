//! Account usage counts retained from local event observations, in UTC
//! quarter-hours. Only aggregate rows are decoded to answer a client's zone.
use super::{
    native_index,
    native_logs::{Kind, Observation},
    pricing::Pricing,
};
use jiff::{Span, Timestamp, tz::TimeZone};
use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const DAY: i64 = 86_400_000;
const SLOT: i64 = 900_000;
#[derive(Debug)]
pub struct Priced {
    model: Option<String>,
    tokens: u64,
    cost: Option<f64>,
    unknown: bool,
    fallback: Option<String>,
}
fn price(event: &Observation, kind: Kind, pricing: &Pricing, fallback: Option<&str>) -> Priced {
    let model = event
        .model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let tokens = if kind == Kind::Codex {
        event.counts.total.unwrap_or(event.counts.processed())
    } else {
        event.counts.processed()
    };
    if kind == Kind::Codex {
        let resolution = super::codex_pricing::price(
            pricing,
            if model == Some("codex-auto-review") {
                "codex-auto-review"
            } else {
                event.pricing_model.as_deref().or(model).unwrap_or("")
            },
            event.at,
            &event.counts,
            fallback,
        );
        return Priced {
            model: model.map(str::to_owned),
            tokens,
            cost: resolution.cost,
            unknown: resolution.unknown,
            fallback: resolution.fallback,
        };
    }
    let candidates = [model, event.pricing_model.as_deref()];
    let priced = candidates
        .into_iter()
        .flatten()
        .find_map(|name| pricing.resolve(name).map(|r| (name, r)));
    let cost = event.billed.or_else(|| {
        priced
            .as_ref()
            .map(|(_, rates)| rates.cost(&event.counts.price_tokens(), true))
    });
    let unknown = cost.is_none() && tokens > 0;
    let name = if kind == Kind::Antigravity {
        priced
            .as_ref()
            .map(|(name, _)| {
                pricing
                    .canonical_name(name)
                    .trim_end_matches("-preview")
                    .to_owned()
            })
            .or_else(|| model.map(str::to_owned))
            .or_else(|| Some("Unknown Antigravity Model".into()))
    } else {
        model
            .map(str::to_owned)
            .or_else(|| cost.map(|_| "Unattributed".into()))
    };
    Priced {
        model: name,
        tokens,
        cost,
        unknown,
        fallback: None,
    }
}
pub fn schema(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS native_price(source TEXT NOT NULL,position INTEGER NOT NULL,ordinal INTEGER NOT NULL,model TEXT,tokens INTEGER NOT NULL,cost REAL,unknown INTEGER NOT NULL,fallback TEXT,
          PRIMARY KEY(source,position,ordinal));
          CREATE TABLE IF NOT EXISTS hosted_call(call TEXT PRIMARY KEY, provider TEXT NOT NULL, at INTEGER NOT NULL, model TEXT NOT NULL, tokens INTEGER NOT NULL,cost REAL NOT NULL,scanned INTEGER NOT NULL);
          CREATE INDEX IF NOT EXISTS hosted_call_at ON hosted_call(at);")
}
pub fn put(
    db: &Connection,
    source: &str,
    position: u64,
    ordinal: u64,
    event: &Observation,
    kind: Kind,
    pricing: &Pricing,
    fallback: Option<&str>,
) -> Result<()> {
    let p = price(event, kind, pricing, fallback);
    db.execute(
        "INSERT OR REPLACE INTO native_price VALUES(?,?,?,?,?,?,?,?)",
        params![
            source,
            position,
            ordinal,
            p.model,
            p.tokens.min(i64::MAX as u64),
            p.cost,
            p.unknown,
            p.fallback
        ],
    )?;
    Ok(())
}
/// Used only on the initial migration, a new price catalog or a fallback change;
/// the raw observations are already present, so no maker file is read again.
pub fn reprice(
    db: &mut Connection,
    epoch: &str,
    pricing: &Pricing,
    fallback: Option<&str>,
    now: i64,
) -> Result<()> {
    let prior: Option<String> = db
        .query_row(
            "SELECT body FROM state WHERE key='native-price-v1'",
            [],
            |r| r.get(0),
        )
        .ok();
    if prior
        .as_deref()
        .and_then(|s| serde_json::from_str::<String>(s).ok())
        .as_deref()
        == Some(epoch)
    {
        return Ok(());
    }
    let tx = db.transaction()?;
    {
        let mut query =
            tx.prepare("SELECT source,position,ordinal,kind,body FROM native_call WHERE at>=?")?;
        let mut rows = query.query([now - 35 * DAY])?;
        while let Some(row) = rows.next()? {
            let source: String = row.get(0)?;
            let kind: String = row.get(3)?;
            let kind = match kind.as_str() {
                "claude" => Kind::Claude,
                "codex" => Kind::Codex,
                "grok" => Kind::Grok,
                "antigravity" => Kind::Antigravity,
                _ => continue,
            };
            let body: String = row.get(4)?;
            put(
                &tx,
                &source,
                row.get(1)?,
                row.get(2)?,
                &serde_json::from_str(&body)?,
                kind,
                pricing,
                fallback,
            )?;
        }
    }
    tx.execute("INSERT INTO state(key,body) VALUES('native-price-v1',?) ON CONFLICT(key) DO UPDATE SET body=excluded.body",[serde_json::to_string(epoch)?])?;
    tx.commit()?;
    Ok(())
}
#[derive(Default, Clone)]
struct Day {
    models: BTreeMap<String, (u64, f64)>,
    unknown: BTreeSet<String>,
    fallback: BTreeSet<String>,
}
type Days = BTreeMap<String, Day>;
#[derive(Serialize)]
struct Model<'a> {
    model: &'a str,
    tokens: u64,
    cost: f64,
}
#[derive(Serialize)]
struct WireDay<'a> {
    date: &'a str,
    tokens: u64,
    cost: f64,
    models: Vec<Model<'a>>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    unknown: &'a BTreeSet<String>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    fallback: &'a BTreeSet<String>,
}
fn days(days: &Days) -> Vec<WireDay<'_>> {
    days.iter()
        .rev()
        .map(|(date, day)| {
            let models = day
                .models
                .iter()
                .map(|(model, (tokens, cost))| Model {
                    model,
                    tokens: *tokens,
                    cost: *cost,
                })
                .collect::<Vec<_>>();
            WireDay {
                date,
                tokens: models.iter().map(|m| m.tokens).sum(),
                cost: models.iter().map(|m| m.cost).sum(),
                models,
                unknown: &day.unknown,
                fallback: &day.fallback,
            }
        })
        .collect()
}
#[derive(Clone, Default, Debug)]
pub struct Slots {
    pub used: BTreeMap<i64, BTreeMap<String, (u64, f64)>>,
    pub unknown: BTreeMap<i64, BTreeSet<String>>,
    pub fallback: BTreeMap<i64, BTreeSet<String>>,
}
impl Slots {
    pub fn add(&mut self, slot: i64, model: String, tokens: u64, cost: f64) {
        let entry = self.used.entry(slot).or_default().entry(model).or_default();
        entry.0 = entry.0.saturating_add(tokens);
        entry.1 += cost;
    }
    pub fn merge(&mut self, other: &Self) {
        for (slot, models) in &other.used {
            for (model, (tokens, cost)) in models {
                self.add(*slot, model.clone(), *tokens, *cost);
            }
        }
        for (slot, models) in &other.unknown {
            self.unknown
                .entry(*slot)
                .or_default()
                .extend(models.iter().cloned());
        }
        for (slot, models) in &other.fallback {
            self.fallback
                .entry(*slot)
                .or_default()
                .extend(models.iter().cloned());
        }
    }
    fn days(&self, now: i64, zone: &TimeZone) -> Result<Days> {
        let today = Timestamp::from_millisecond(now)?
            .to_zoned(zone.clone())
            .date();
        let earliest = today.checked_sub(Span::new().days(30))?;
        let date = |slot: i64| -> Option<String> {
            let day = Timestamp::from_millisecond(slot)
                .ok()?
                .to_zoned(zone.clone())
                .date();
            (day >= earliest && day <= today).then(|| day.to_string())
        };
        let mut days = Days::new();
        for (at, models) in &self.used {
            let Some(date) = date(*at) else {
                continue;
            };
            let day = days.entry(date).or_default();
            for (model, (tokens, cost)) in models {
                let row = day.models.entry(model.clone()).or_default();
                row.0 = row.0.saturating_add(*tokens);
                row.1 += cost;
            }
        }
        for (at, models) in &self.unknown {
            if let Some(date) = date(*at) {
                days.entry(date)
                    .or_default()
                    .unknown
                    .extend(models.iter().cloned());
            }
        }
        for (at, models) in &self.fallback {
            if let Some(date) = date(*at) {
                days.entry(date)
                    .or_default()
                    .fallback
                    .extend(models.iter().cloned());
            }
        }
        Ok(days)
    }
}
#[derive(Clone, Debug)]
pub struct Account {
    pub provider: String,
    pub service: String,
    pub agent: Option<String>,
    pub account: Option<String>,
    pub label: Option<String>,
    pub identity: Option<String>,
    pub scanned: Option<i64>,
    pub places: Vec<String>,
    pub billed: bool,
    pub wide: bool,
    pub cut: bool,
    pub slots: Slots,
}
const SERVICES: [(&str, &str); 6] = [
    ("claude", "Claude"),
    ("codex", "Codex"),
    ("grok", "Grok"),
    ("opencode", "OpenCode"),
    ("cursor", "Cursor"),
    ("antigravity", "Antigravity"),
];
pub fn own(
    db: &Connection,
    now: i64,
    identities: &BTreeMap<String, String>,
) -> Result<Vec<Account>> {
    let mut records = BTreeMap::<String, Slots>::new();
    let query = format!(
        "SELECT n.provider,n.at/{SLOT}*{SLOT},p.model,COALESCE(SUM(CASE WHEN p.cost IS NOT NULL THEN p.tokens ELSE 0 END),0),SUM(p.cost),MAX(p.unknown),p.fallback FROM native_call n JOIN native_price p ON p.source=n.source AND p.position=n.position AND p.ordinal=n.ordinal WHERE n.at>=? AND ({}) GROUP BY n.provider,n.at/{SLOT},p.model,p.fallback",
        native_index::CANONICAL
    );
    let mut statement = db.prepare(&query)?;
    let mut rows = statement.query([now - 35 * DAY])?;
    while let Some(row) = rows.next()? {
        let provider: String = row.get(0)?;
        let at: i64 = row.get(1)?;
        let model: Option<String> = row.get(2)?;
        let Some(model) = model else {
            continue;
        };
        let slots = records.entry(provider).or_default();
        let tokens: u64 = row.get(3)?;
        let cost: Option<f64> = row.get(4)?;
        if let Some(cost) = cost {
            slots.add(at, model.clone(), tokens, cost);
        }
        if row.get::<_, bool>(5)? {
            slots.unknown.entry(at).or_default().insert(model);
        }
        if let Some(fallback) = row.get::<_, Option<String>>(6)? {
            slots.fallback.entry(at).or_default().insert(fallback);
        }
    }
    let mut query=db.prepare("SELECT provider,kind,MAX(label),MAX(scanned),GROUP_CONCAT(place) FROM native_root GROUP BY provider,kind ORDER BY kind,provider")?;
    let mut sources = query.query([])?;
    let mut out = vec![];
    while let Some(row) = sources.next()? {
        let provider: String = row.get(0)?;
        let kind: String = row.get(1)?;
        let account = provider
            .split_once('@')
            .map(|(_, id)| id)
            .unwrap_or("default")
            .to_owned();
        let slots = records.remove(&provider).unwrap_or_default();
        let places: Option<String> = row.get(4)?;
        out.push(Account {
            identity: identities.get(&provider).cloned(),
            provider,
            service: kind.clone(),
            agent: Some(kind),
            account: Some(account),
            label: row.get(2)?,
            scanned: row.get(3)?,
            places: places
                .map(|s| s.split(',').map(str::to_owned).collect())
                .unwrap_or_default(),
            billed: false,
            wide: false,
            cut: false,
            slots,
        });
    }
    let mut hosted = BTreeMap::<String, (i64, Slots)>::new();
    let mut query=db.prepare(&format!("SELECT provider,at/{SLOT}*{SLOT},model,SUM(tokens),SUM(cost),MAX(scanned) FROM hosted_call WHERE at>=? GROUP BY provider,at/{SLOT},model"))?;
    let mut rows = query.query([now - 35 * DAY])?;
    while let Some(row) = rows.next()? {
        let provider: String = row.get(0)?;
        let (scanned, slots) = hosted.entry(provider).or_default();
        *scanned = (*scanned).max(row.get(5)?);
        slots.add(row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?);
    }
    for (provider, (scanned, slots)) in hosted {
        out.push(Account {
            identity: identities.get(&provider).cloned(),
            provider,
            service: "opencode".into(),
            agent: None,
            account: None,
            label: None,
            scanned: Some(scanned),
            places: vec![],
            billed: true,
            wide: false,
            cut: false,
            slots,
        });
    }
    // The wire order follows the bridge's source roster, not SQLite's
    // alphabetical kind order. Default sign-ins precede their added accounts.
    out.sort_by_key(|row| match row.service.as_str() {
        "claude" => 0,
        "codex" if row.agent.is_some() => 1,
        "grok" => 2,
        "codex" => 3,
        "opencode" => 4,
        "cursor" => 5,
        "antigravity" => 6,
        _ => 7,
    });
    Ok(out)
}
pub struct Computer {
    pub key: String,
    pub name: String,
    pub here: bool,
    pub away: bool,
    pub accounts: Vec<Account>,
}
pub fn view(
    computers: Vec<Computer>,
    now: i64,
    zone: TimeZone,
    pricing: &Pricing,
    fallback: Option<&str>,
    chosen: bool,
) -> Result<Value> {
    let mut groups = BTreeMap::<String, Vec<(&Computer, &Account, bool)>>::new();
    let mut order = vec![];
    let mut places = BTreeSet::new();
    let mut scanned = None;
    for computer in &computers {
        for account in &computer.accounts {
            if computer.here {
                scanned = scanned.max(account.scanned);
            }
            let same = account.places.iter().any(|p| places.contains(p));
            if !same {
                places.extend(account.places.iter().cloned());
            }
            let id = account
                .identity
                .as_ref()
                .map(|id| format!("{}#{id}", account.service))
                .unwrap_or_else(|| format!("{}/{}", computer.key, account.provider));
            if !groups.contains_key(&id) {
                order.push(id.clone());
            }
            groups
                .entry(id)
                .or_default()
                .push((computer, account, same));
        }
    }
    let mut totals = BTreeMap::<String, Slots>::new();
    let mut partial = BTreeMap::<String, (bool, bool)>::new();
    let mut rows = vec![];
    for id in order {
        let members = &groups[&id];
        let base = members
            .iter()
            .find(|(_, a, _)| a.agent.is_some())
            .unwrap_or(&members[0])
            .1;
        let label = members.iter().find_map(|(_, a, _)| a.label.as_ref());
        let scanned = members.iter().filter_map(|(_, a, _)| a.scanned).max();
        let wide = members.iter().any(|(_, a, _)| a.wide);
        let mut counted = members
            .iter()
            .filter(|(_, _, same)| !*same)
            .collect::<Vec<_>>();
        if wide {
            counted = counted
                .into_iter()
                .rev()
                .max_by_key(|(_, a, _)| a.scanned)
                .into_iter()
                .collect();
        }
        let mut slots = Slots::default();
        let mut away = false;
        let mut cut = false;
        for (computer, account, _) in counted {
            slots.merge(&account.slots);
            away |= computer.away;
            cut |= account.cut;
        }
        totals
            .entry(base.service.clone())
            .or_default()
            .merge(&slots);
        if away || cut {
            let flags = partial.entry(base.service.clone()).or_default();
            flags.0 |= away;
            flags.1 |= cut;
        }
        let boxes = members
            .iter()
            .map(|(c, a, same)| {
                let mut value = json!({"key":c.key,"name":c.name,"provider":a.provider});
                for (name, field) in [
                    ("agent", a.agent.as_ref()),
                    ("account", a.account.as_ref()),
                    ("label", a.label.as_ref()),
                ] {
                    if let Some(field) = field {
                        value[name] = json!(field);
                    }
                }
                if let Some(at) = a.scanned {
                    value["scanned"] = json!(at);
                }
                if c.here {
                    value["here"] = json!(true);
                }
                if c.away {
                    value["away"] = json!(true);
                }
                if *same {
                    value["same"] = json!(true);
                }
                value
            })
            .collect::<Vec<_>>();
        let day = slots.days(now, &zone)?;
        let mut row = json!({"id":id,"service":base.service,"provider":base.provider,"boxes":boxes,"days":days(&day)});
        for (name, field) in [
            ("agent", base.agent.as_ref()),
            ("account", base.account.as_ref()),
            ("label", label),
            ("identity", base.identity.as_ref()),
        ] {
            if let Some(field) = field {
                row[name] = json!(field);
            }
        }
        if let Some(at) = scanned {
            row["scanned"] = json!(at);
        }
        if cut {
            row["cut"] = json!(true);
        }
        if wide {
            row["accountWide"] = json!(true);
        }
        if members.iter().all(|(c, _, _)| c.away) {
            row["away"] = json!(true);
        }
        if members.iter().all(|(_, a, _)| a.billed) {
            row["estimated"] = json!(false);
        }
        rows.push(row);
    }
    let mut service_totals = BTreeMap::new();
    for (service, slots) in totals {
        service_totals.insert(service, json!(days(&slots.days(now, &zone)?)));
    }
    let mut data = json!({"zone":zone.iana_name().unwrap_or("UTC"),"services":SERVICES.map(|(id,_)|id),"serviceTitles":SERVICES.into_iter().collect::<BTreeMap<_,_>>(),
        "rows":rows,"totals":service_totals,"pricing":{"codex":{"fallback":fallback,"chosen":chosen,"choices":pricing.fallback_choices("codex")}}});
    if let Some(at) = scanned {
        data["scanned"] = json!(at);
    }
    if !partial.is_empty() {
        data["partial"] = json!(
            partial
                .into_iter()
                .map(|(service, (away, cut))| {
                    let mut v = json!({});
                    if away {
                        v["away"] = json!(true);
                    }
                    if cut {
                        v["cut"] = json!(true);
                    }
                    (service, v)
                })
                .collect::<BTreeMap<_, _>>()
        );
    }
    Ok(json!({"data":data}))
}
