//! Cursor supplies one account-wide history, not per-machine usage. The
//! complete UTC days and their raw counts are indexed once and repriced locally.
use super::{
    history::{Account, Slots},
    native_logs::Counts,
    pricing::Pricing,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub const DAY: i64 = 86_400_000;
const SLOT: i64 = 900_000;
pub const DAYS: i64 = 32;
const SAFE: u64 = 9_007_199_254_740_991;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Use {
    pub slot: i64,
    pub model: String,
    pub counts: Counts,
}
#[derive(Clone, Debug)]
pub struct Day {
    pub start: i64,
    pub complete: bool,
    pub uses: Vec<Use>,
}
#[derive(Debug, Default)]
pub struct Page {
    pub total: u64,
    pub received: u64,
    pub events: Vec<Use>,
}
fn whole(v: &Value) -> Option<u64> {
    v.as_u64().filter(|n| *n <= SAFE)
}
fn number(v: Option<&Value>) -> Option<u64> {
    v.map_or(Some(0), whole)
}
pub fn parse(body: &Value) -> Option<Page> {
    let total = whole(&body["totalUsageEventsCount"])?;
    let events = body["usageEventsDisplay"].as_array()?;
    let mut page = Page {
        total,
        received: events.len() as u64,
        events: vec![],
    };
    for event in events {
        let read = || -> Option<Use> {
            let at = event["timestamp"]
                .as_str()
                .and_then(|s| s.parse::<u64>().ok())
                .filter(|n| *n <= SAFE)
                .or_else(|| whole(&event["timestamp"]))?;
            let at = i64::try_from(at).ok()?;
            let model = event["model"].as_str()?.trim().to_owned();
            let mut counts = Counts::default();
            if let Some(value) = event.get("tokenUsage") {
                let t = value.as_object()?;
                counts.input = number(t.get("inputTokens"))?;
                counts.output = number(t.get("outputTokens"))?;
                counts.cache_write_5m = number(t.get("cacheWriteTokens"))?;
                counts.cache_read = number(t.get("cacheReadTokens"))?;
            }
            Some(Use {
                slot: at,
                model,
                counts,
            })
        };
        if let Some(event) = read() {
            page.events.push(event);
        }
    }
    Some(page)
}
pub fn merge_page(
    uses: &mut BTreeMap<(i64, String), Counts>,
    start: i64,
    page: Page,
) -> Option<(u64, u64)> {
    for event in page.events {
        if event.slot < start || event.slot >= start + DAY {
            return None;
        }
        let slot = event.slot.div_euclid(SLOT) * SLOT;
        let counts = uses.entry((slot, event.model)).or_default();
        counts.input = counts.input.saturating_add(event.counts.input);
        counts.output = counts.output.saturating_add(event.counts.output);
        counts.cache_read = counts.cache_read.saturating_add(event.counts.cache_read);
        counts.cache_write_5m = counts
            .cache_write_5m
            .saturating_add(event.counts.cache_write_5m);
    }
    Some((page.total, page.received))
}
pub fn schema(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS cursor_account(provider TEXT PRIMARY KEY,account TEXT NOT NULL,label TEXT,identity TEXT NOT NULL,user_key TEXT NOT NULL,scanned INTEGER,sign_mark TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS cursor_day(user_key TEXT NOT NULL,start INTEGER NOT NULL,complete INTEGER NOT NULL,PRIMARY KEY(user_key,start));
      CREATE TABLE IF NOT EXISTS cursor_use(user_key TEXT NOT NULL,day INTEGER NOT NULL,slot INTEGER NOT NULL,model TEXT NOT NULL,counts TEXT NOT NULL,tokens INTEGER NOT NULL,cost REAL,unknown INTEGER NOT NULL,PRIMARY KEY(user_key,day,slot,model));
      CREATE INDEX IF NOT EXISTS cursor_day_start ON cursor_day(start);
      CREATE INDEX IF NOT EXISTS cursor_use_day ON cursor_use(day);")
}
pub fn kept_days(db: &Connection, user: &str, now: i64) -> Result<BTreeMap<i64, bool>> {
    let mut q =
        db.prepare("SELECT start,complete FROM cursor_day WHERE user_key=? AND start>=?")?;
    let rows = q.query_map(
        params![user, now.div_euclid(DAY) * DAY - (DAYS - 1) * DAY],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}
fn priced(model: &str, counts: &Counts, pricing: &Pricing) -> (Option<f64>, bool) {
    let cost = pricing
        .resolve(model)
        .map(|rate| rate.cost(&counts.price_tokens(), false));
    (
        cost,
        cost.is_none() && !model.is_empty() && counts.processed() > 0,
    )
}
pub fn save_day(
    db: &mut Connection,
    user: &str,
    day: Day,
    pricing: &Pricing,
    now: i64,
) -> Result<()> {
    let tx = db.transaction()?;
    tx.execute(
        "DELETE FROM cursor_use WHERE user_key=? AND day=?",
        params![user, day.start],
    )?;
    for row in day.uses {
        let (cost, unknown) = priced(&row.model, &row.counts, pricing);
        tx.execute(
            "INSERT INTO cursor_use VALUES(?,?,?,?,?,?,?,?)",
            params![
                user,
                day.start,
                row.slot,
                row.model,
                serde_json::to_string(&row.counts)?,
                row.counts.processed().min(SAFE),
                cost,
                unknown
            ],
        )?;
    }
    tx.execute(
        "INSERT OR REPLACE INTO cursor_day VALUES(?,?,?)",
        params![user, day.start, day.complete],
    )?;
    tx.execute("DELETE FROM cursor_day WHERE start<?", [now - 35 * DAY])?;
    tx.execute("DELETE FROM cursor_use WHERE day<?", [now - 35 * DAY])?;
    tx.commit()?;
    Ok(())
}
pub fn reprice(db: &Connection, pricing: &Pricing, now: i64) -> Result<()> {
    let mut q = db.prepare("SELECT user_key,day,slot,model,counts FROM cursor_use WHERE day>=?")?;
    let mut rows = q.query([now - 35 * DAY])?;
    while let Some(row) = rows.next()? {
        let user: String = row.get(0)?;
        let day: i64 = row.get(1)?;
        let at: i64 = row.get(2)?;
        let model: String = row.get(3)?;
        let counts: Counts = serde_json::from_str(&row.get::<_, String>(4)?)?;
        let (cost, unknown) = priced(&model, &counts, pricing);
        db.execute("UPDATE cursor_use SET cost=?,unknown=? WHERE user_key=? AND day=? AND slot=? AND model=?",params![cost,unknown,user,day,at,model])?;
    }
    Ok(())
}
pub fn known(db: &Connection, provider: &str, user: &str) -> Result<Option<i64>> {
    Ok(db
        .query_row(
            "SELECT scanned FROM cursor_account WHERE provider=? AND user_key=?",
            params![provider, user],
            |r| r.get(0),
        )
        .optional()?
        .flatten())
}
pub fn account(
    db: &Connection,
    provider: &str,
    account: &str,
    label: Option<&str>,
    identity: &str,
    user: &str,
    scanned: Option<i64>,
    mark: &str,
) -> Result<()> {
    db.execute("INSERT INTO cursor_account VALUES(?,?,?,?,?,?,?) ON CONFLICT(provider) DO UPDATE SET account=excluded.account,label=excluded.label,identity=excluded.identity,user_key=excluded.user_key,sign_mark=excluded.sign_mark,scanned=CASE WHEN excluded.user_key=cursor_account.user_key THEN COALESCE(excluded.scanned,cursor_account.scanned) ELSE excluded.scanned END",params![provider,account,label,identity,user,scanned,mark])?;
    Ok(())
}
pub fn forget_changed(db: &Connection, provider: &str, mark: &str) -> Result<bool> {
    Ok(db.execute(
        "DELETE FROM cursor_account WHERE provider=? AND sign_mark<>?",
        params![provider, mark],
    )? > 0)
}
pub fn retain(db: &Connection, providers: &BTreeSet<String>) -> Result<bool> {
    Ok(db.execute(
        "DELETE FROM cursor_account WHERE provider NOT IN (SELECT value FROM json_each(?))",
        [serde_json::to_string(providers)?],
    )? > 0)
}
pub fn own(db: &Connection, now: i64) -> Result<Vec<Account>> {
    let mut q=db.prepare("SELECT provider,account,label,identity,user_key,scanned FROM cursor_account WHERE scanned IS NOT NULL ORDER BY provider")?;
    let mut rows = q.query([])?;
    let mut out = vec![];
    while let Some(row) = rows.next()? {
        let provider: String = row.get(0)?;
        let user: String = row.get(4)?;
        let mut slots = Slots::default();
        let mut q=db.prepare_cached("SELECT slot,model,tokens,cost,unknown FROM cursor_use WHERE user_key=? AND day>=? ORDER BY slot,model")?;
        let mut uses = q.query(params![user, now - 35 * DAY])?;
        while let Some(row) = uses.next()? {
            let at: i64 = row.get(0)?;
            let model: String = row.get(1)?;
            if let Some(cost) = row.get::<_, Option<f64>>(3)? {
                slots.add(
                    at,
                    if model.is_empty() {
                        "Unattributed".into()
                    } else {
                        model.clone()
                    },
                    row.get(2)?,
                    cost,
                );
            }
            if row.get::<_, bool>(4)? && !model.is_empty() {
                slots.unknown.entry(at).or_default().insert(model);
            }
        }
        out.push(Account {
            provider,
            service: "cursor".into(),
            agent: Some("cursor".into()),
            account: Some(row.get(1)?),
            label: row.get(2)?,
            identity: Some(row.get(3)?),
            scanned: row.get(5)?,
            places: vec![],
            billed: false,
            wide: true,
            cut: false,
            slots,
        });
    }
    Ok(out)
}
