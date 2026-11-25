//! The maker's billed Go/Zen calls. Keep separate channel records so a
//! removed or replaced channel cannot keep an old total, and a copied call
//! is selected by the same path/table order as the reference box.
use super::{
    engine_index::Record,
    history::{Account, Slots},
};
use rusqlite::{Connection, params};
use std::collections::BTreeMap;
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const DAY: i64 = 86_400_000;
const SLOT: i64 = 900_000;
pub fn schema(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS engine_channel(source TEXT PRIMARY KEY,path TEXT NOT NULL,scanned INTEGER NOT NULL,go_place TEXT NOT NULL,zen_place TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS engine_hosted_call(source TEXT NOT NULL,version INTEGER NOT NULL,call TEXT NOT NULL,provider TEXT NOT NULL,at INTEGER NOT NULL,model TEXT NOT NULL,tokens INTEGER NOT NULL,cost REAL NOT NULL,PRIMARY KEY(source,version,call));
      CREATE INDEX IF NOT EXISTS engine_hosted_at ON engine_hosted_call(at,call);")
}
pub fn source(db: &Connection, key: &str, path: &str, now: i64, go: &str, zen: &str) -> Result<()> {
    db.execute(
        "INSERT OR REPLACE INTO engine_channel VALUES(?,?,?,?,?)",
        params![key, path, now, go, zen],
    )?;
    Ok(())
}
pub fn reset(db: &Connection, key: &str, version: i64) -> Result<()> {
    db.execute(
        "DELETE FROM engine_hosted_call WHERE source=? AND version=?",
        params![key, version],
    )?;
    db.execute(
        "DELETE FROM engine_codex_call WHERE source=? AND table_version=?",
        params![key, version],
    )?;
    Ok(())
}
/// Only directory metadata is enumerated, not message rows. An unreadable
/// directory is never passed here as an empty one.
pub fn retain(db: &Connection, keys: &[String]) -> Result<bool> {
    let keys = serde_json::to_string(keys)?;
    let mut changed = 0;
    for table in [
        "engine_channel",
        "engine_hosted_call",
        "engine_codex_source",
        "engine_codex_call",
    ] {
        changed += db.execute(
            &format!("DELETE FROM {table} WHERE source NOT IN (SELECT value FROM json_each(?))"),
            [&keys],
        )?;
    }
    changed+=db.execute("DELETE FROM state WHERE key LIKE 'scan/rust-engine-v1/%' AND NOT EXISTS(SELECT 1 FROM json_each(?) k WHERE state.key=k.value OR state.key=k.value||'/decoded')",[&keys])?;
    Ok(changed > 0)
}
pub fn observe(db: &Connection, key: &str, record: &Record) -> Result<()> {
    let provider = record.provider.as_deref().unwrap_or_default();
    let cost = record.cost.filter(|cost| cost.is_finite() && *cost >= 0.0);
    if !matches!(provider, "opencode-go" | "opencode")
        || !matches!(record.role.as_str(), "assistant" | "compaction")
        || cost.is_none()
    {
        db.execute(
            "DELETE FROM engine_hosted_call WHERE source=? AND version=? AND call=?",
            params![key, record.version, record.call],
        )?;
        return Ok(());
    }
    let n = |v: &serde_json::Value| {
        super::engine_readers::number(v)
            .map(|n| n.max(0.).min(1e15))
            .unwrap_or(0.)
    };
    let usage = record.usage.as_ref().unwrap_or(&serde_json::Value::Null);
    let tokens = super::engine_readers::number(&usage["total"])
        .unwrap_or_else(|| {
            [
                &usage["input"],
                &usage["output"],
                &usage["reasoning"],
                &usage["cache"]["read"],
                &usage["cache"]["write"],
            ]
            .into_iter()
            .map(n)
            .sum()
        })
        .clamp(0., 1e15) as u64;
    db.execute(
        "INSERT OR REPLACE INTO engine_hosted_call VALUES(?,?,?,?,?,?,?,?)",
        params![
            key,
            record.version,
            record.call,
            provider,
            record.created,
            record.model.as_deref().unwrap_or(""),
            tokens,
            cost
        ],
    )?;
    Ok(())
}
pub fn own(
    db: &Connection,
    now: i64,
    identities: &BTreeMap<String, String>,
) -> Result<Vec<Account>> {
    let sql = format!(
        "WITH ranked AS (SELECT h.*,c.scanned,CASE h.provider WHEN 'opencode-go' THEN c.go_place ELSE c.zen_place END AS place,ROW_NUMBER() OVER(PARTITION BY h.call ORDER BY c.path,h.version) AS rank FROM engine_hosted_call h JOIN engine_channel c ON h.source=c.source WHERE h.at>=?) SELECT provider,at/{SLOT}*{SLOT},model,SUM(tokens),SUM(cost),MAX(scanned),GROUP_CONCAT(DISTINCT place) FROM ranked WHERE rank=1 GROUP BY provider,at/{SLOT},model"
    );
    let mut q = db.prepare(&sql)?;
    let mut rows = q.query([now - 35 * DAY])?;
    let mut hosted = BTreeMap::<String, (i64, Slots, std::collections::BTreeSet<String>)>::new();
    while let Some(row) = rows.next()? {
        let provider: String = row.get(0)?;
        let (scanned, slots, places) = hosted.entry(provider).or_default();
        *scanned = (*scanned).max(row.get(5)?);
        slots.add(row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?);
        if let Some(given) = row.get::<_, Option<String>>(6)? {
            places.extend(
                given
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
            );
        }
    }
    Ok(hosted
        .into_iter()
        .map(|(provider, (scanned, slots, places))| Account {
            identity: identities.get(&provider).cloned(),
            provider,
            service: "opencode".into(),
            agent: None,
            account: None,
            label: None,
            scanned: Some(scanned),
            places: places.into_iter().collect(),
            billed: true,
            wide: false,
            cut: false,
            slots,
        })
        .collect())
}
