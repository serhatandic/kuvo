//! One engine channel's ChatGPT history. The maker's read-only database says
//! whether the active sign-in is OAuth; no access/refresh token leaves SQLite.
use super::{
    engine_index::Record,
    history::{Account, Slots},
    native_logs::Counts,
    pricing::Pricing,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const DAY: i64 = 86_400_000;
const SLOT: i64 = 900_000;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignIn {
    #[default]
    Out,
    OAuth {
        since: Option<i64>,
    },
}
impl SignIn {
    pub fn active(self) -> bool {
        matches!(self, Self::OAuth { .. })
    }
    pub fn since(self) -> Option<i64> {
        match self {
            Self::Out => None,
            Self::OAuth { since } => since,
        }
    }
}
pub fn sign_in(db: &Connection, folder: &Path) -> Result<SignIn> {
    let imported = db
        .query_row(
            "SELECT 1 FROM migration WHERE id='20260805200742_import_legacy_credentials' LIMIT 1",
            [],
            |r| r.get::<_, i64>(0),
        )
        .optional();
    let imported = match imported {
        Ok(row) => row.is_some(),
        Err(error) if error.to_string().contains("no such table") => false,
        Err(error) => return Err(error.into()),
    };
    if imported {
        let result=db.query_row("SELECT json_extract(doc,'$.type')='oauth' AND (LENGTH(TRIM(COALESCE(json_extract(doc,'$.access'),'')))>0 OR LENGTH(TRIM(COALESCE(json_extract(doc,'$.refresh'),'')))>0),time_created FROM (SELECT CASE WHEN json_type(value)='text' THEN json_extract(value,'$') ELSE value END AS doc,time_created,active,time_updated,id FROM credential WHERE integration_id='openai' AND (active IS NULL OR active=1)) ORDER BY active DESC,time_updated DESC,id DESC LIMIT 1",[],|r|Ok((r.get::<_,Option<bool>>(0)?.unwrap_or(false),r.get::<_,Option<i64>>(1)?))).optional();
        match result {
            Ok(Some((true, since))) => {
                if since.is_some_and(|at| !(0..=1_000_000_000_000_000).contains(&at)) {
                    return Err("Malformed engine OAuth sign-in time".into());
                }
                return Ok(SignIn::OAuth { since });
            }
            Ok(_) => return Ok(SignIn::Out),
            Err(error) if error.to_string().contains("no such table") => {}
            Err(error) => return Err(error.into()),
        }
    }
    let Some(file) = super::engine_index::owned(folder, &folder.join("auth.json"))? else {
        return Ok(SignIn::Out);
    };
    let Some(raw) = kuvo_store::files::read(&file)? else {
        return Ok(SignIn::Out);
    };
    let raw: Value = serde_json::from_slice(&raw)?;
    let sign = &raw["openai"];
    Ok(
        if sign["type"] == "oauth"
            && ["access", "refresh"]
                .iter()
                .any(|field| sign[*field].as_str().is_some_and(|s| !s.trim().is_empty()))
        {
            SignIn::OAuth { since: None }
        } else {
            SignIn::Out
        },
    )
}
pub fn schema(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS engine_codex_source(source TEXT PRIMARY KEY,active INTEGER NOT NULL,since INTEGER,scanned INTEGER NOT NULL,place TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS engine_codex_call(source TEXT NOT NULL,table_version INTEGER NOT NULL,call TEXT NOT NULL,at INTEGER NOT NULL,model TEXT NOT NULL,counts TEXT NOT NULL,tokens INTEGER NOT NULL,cost REAL,unknown INTEGER NOT NULL,
        PRIMARY KEY(source,table_version,call)); CREATE INDEX IF NOT EXISTS engine_codex_at ON engine_codex_call(at,call);")
}
pub fn source(
    db: &Connection,
    key: &str,
    sign: SignIn,
    scanned: i64,
    place: &str,
) -> rusqlite::Result<()> {
    db.execute(
        "INSERT OR REPLACE INTO engine_codex_source VALUES(?,?,?,?,?)",
        params![key, sign.active(), sign.since(), scanned, place],
    )?;
    Ok(())
}
fn counts(raw: Option<&Value>) -> Counts {
    let raw = raw.unwrap_or(&Value::Null);
    let n = |v: &Value| {
        super::engine_readers::number(v)
            .map(|x| x.clamp(0., 1e15) as u64)
            .unwrap_or(0)
    };
    let mut counts = Counts {
        input: n(&raw["input"]),
        output: n(&raw["output"]).saturating_add(n(&raw["reasoning"])),
        cache_read: n(&raw["cache"]["read"]),
        cache_write_5m: n(&raw["cache"]["write"]),
        ..Default::default()
    };
    counts.total = Some(if counts.processed() > 0 {
        counts.processed()
    } else {
        n(&raw["total"])
    });
    counts
}
pub fn observe(db: &Connection, source: &str, record: &Record, pricing: &Pricing) -> Result<()> {
    if record.provider.as_deref() != Some("openai") {
        return Ok(());
    }
    let at = record.completed.unwrap_or(record.created);
    if !record.codex_history {
        db.execute(
            "DELETE FROM engine_codex_call WHERE source=? AND table_version=? AND call=?",
            params![source, record.version, &record.call],
        )?;
        return Ok(());
    }
    let counts = counts(record.usage.as_ref());
    let model = record.model.as_deref().unwrap_or("").trim();
    let price = super::codex_pricing::price(pricing, model, at, &counts, None);
    db.execute(
        "INSERT OR REPLACE INTO engine_codex_call VALUES(?,?,?,?,?,?,?,?,?)",
        params![
            source,
            record.version,
            record.call,
            at,
            model,
            serde_json::to_string(&counts)?,
            counts.total.unwrap_or(0),
            price.cost,
            price.unknown
        ],
    )?;
    Ok(())
}
pub fn reprice(db: &Connection, pricing: &Pricing, now: i64) -> Result<()> {
    let mut q = db.prepare(
        "SELECT source,table_version,call,at,model,counts FROM engine_codex_call WHERE at>=?",
    )?;
    let mut rows = q.query([now - 35 * DAY])?;
    while let Some(row) = rows.next()? {
        let source: String = row.get(0)?;
        let version: i64 = row.get(1)?;
        let call: String = row.get(2)?;
        let at: i64 = row.get(3)?;
        let model: String = row.get(4)?;
        let counts: Counts = serde_json::from_str(&row.get::<_, String>(5)?)?;
        let price = super::codex_pricing::price(pricing, &model, at, &counts, None);
        db.execute("UPDATE engine_codex_call SET cost=?,unknown=? WHERE source=? AND table_version=? AND call=?",params![price.cost,price.unknown,source,version,call])?;
    }
    Ok(())
}
pub fn account(
    db: &Connection,
    now: i64,
    identities: &BTreeMap<String, String>,
) -> Result<Option<Account>> {
    let (scanned, places): (Option<i64>, Option<String>) = db.query_row(
        "SELECT MAX(scanned),GROUP_CONCAT(DISTINCT place) FROM engine_codex_source WHERE active=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if scanned.is_none() {
        return Ok(None);
    }
    let mut slots = Slots::default();
    let sql = format!(
        "WITH ranked AS (SELECT c.*, ROW_NUMBER() OVER(PARTITION BY call ORDER BY c.at DESC,c.tokens DESC,c.source,c.table_version) AS rank FROM engine_codex_call c JOIN engine_codex_source s ON c.source=s.source WHERE s.active=1 AND c.at>=? AND (c.table_version=1 OR s.since IS NULL OR c.at>=s.since)) SELECT at/{SLOT}*{SLOT},model,SUM(CASE WHEN cost IS NOT NULL THEN tokens ELSE 0 END),SUM(cost),MAX(unknown) FROM ranked WHERE rank=1 GROUP BY at/{SLOT},model"
    );
    let mut q = db.prepare(&sql)?;
    let mut rows = q.query([now - 35 * DAY])?;
    while let Some(row) = rows.next()? {
        let at: i64 = row.get(0)?;
        let model: String = row.get(1)?;
        let tokens: u64 = row.get(2)?;
        let cost: Option<f64> = row.get(3)?;
        if let Some(cost) = cost {
            slots.add(
                at,
                if model.is_empty() {
                    "Unattributed".into()
                } else {
                    model.clone()
                },
                tokens,
                cost,
            );
        }
        if row.get::<_, bool>(4)? && !model.is_empty() {
            slots.unknown.entry(at).or_default().insert(model);
        }
    }
    Ok(Some(Account {
        provider: "openai".into(),
        service: "codex".into(),
        agent: None,
        account: None,
        label: None,
        identity: identities.get("openai").cloned(),
        scanned,
        places: places
            .unwrap_or_default()
            .split(',')
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect(),
        billed: false,
        wide: false,
        cut: false,
        slots,
    }))
}
