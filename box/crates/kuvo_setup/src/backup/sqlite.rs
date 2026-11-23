//! Database snapshots and recovery run against private copies.
use std::{fs::File, io::Write, path::Path, time::Duration};

use kuvo_store::sqlite::{open_for_move, open_index};
use serde_json::Value;

use super::{Error, Result};

const BUSY: Duration = Duration::from_secs(5);

pub(super) fn snapshot(path: &Path, copy: &Path) -> Result<()> {
    let connection = open_index(path, BUSY)?;
    let copy = copy.to_str().ok_or(Error::Message(
        "Could not take a consistent database snapshot.",
    ))?;
    connection
        .execute("VACUUM INTO ?1", [copy])
        .map_err(|_| Error::Message("Could not take a consistent database snapshot."))?;
    Ok(())
}

pub(super) fn scrub_engine(path: &Path, restoring: bool, moved: &[String]) -> Result<()> {
    let scrub = || -> Result<()> {
        let mut db = open_for_move(path, BUSY)?;
        db.execute_batch("PRAGMA secure_delete=ON; PRAGMA foreign_keys=ON;")?;
        let tx = db.transaction()?;
        let tables = tables(&tx)?;
        for name in &tables {
            if name.split('_').any(|part| {
                matches!(
                    part.to_ascii_lowercase().as_str(),
                    "auth"
                        | "credential"
                        | "credentials"
                        | "oauth"
                        | "token"
                        | "tokens"
                        | "permission"
                        | "permissions"
                        | "grant"
                        | "grants"
                ) || (restoring && part.eq_ignore_ascii_case("inbox"))
            }) {
                tx.execute(&format!("DELETE FROM {}", quote(name)), [])?;
            }
        }
        if !moved.is_empty() {
            for name in ["session", "session_v2"] {
                if tables.iter().any(|found| found == name) {
                    let mut delete = tx.prepare(&format!("DELETE FROM {name} WHERE id = ?1"))?;
                    for id in moved {
                        delete.execute([id])?;
                    }
                }
            }
        }
        tx.commit()?;
        db.execute_batch("VACUUM;")?;
        Ok(())
    };
    scrub().map_err(|_| Error::Message("Could not reconcile the engine database."))
}

pub(super) fn transcript(path: &Path, copy: &Path) -> Result<()> {
    let db = open_index(path, Duration::from_secs(1))?;
    let mut query =
        db.prepare("SELECT body FROM message WHERE body IS NOT NULL ORDER BY position")?;
    let mut rows = query.query([])?;
    let mut output = File::create_new(copy)?;
    output.write_all(b"[")?;
    let mut first = true;
    while let Some(row) = rows.next()? {
        let row: String = row.get(0)?;
        let value: Value = serde_json::from_str(&row)?;
        if !first {
            output.write_all(b",")?;
        }
        first = false;
        serde_json::to_writer(&mut output, &value)?;
    }
    output.write_all(b"]")?;
    Ok(())
}

pub(super) fn prepare_bootstrap(path: &Path) -> bool {
    let check = || -> Result<bool> {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let mut side = path.as_os_str().to_owned();
            side.push(suffix);
            if super::inventory::stat(Path::new(&side))?.is_some_and(|meta| !meta.is_file()) {
                return Ok(false);
            }
        }
        let db = open_for_move(path, BUSY)?;
        let names = tables(&db)?;
        if !names
            .iter()
            .any(|table| matches!(table.as_str(), "session" | "session_v2"))
            || db.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))? != "ok"
        {
            return Ok(false);
        }
        for name in names {
            if matches!(
                name.as_str(),
                "migration" | "__drizzle_migrations" | "sqlite_sequence"
            ) {
                continue;
            }
            let query = format!("SELECT EXISTS (SELECT 1 FROM {} LIMIT 1)", quote(&name));
            if db.query_row(&query, [], |row| row.get::<_, bool>(0))? {
                return Ok(false);
            }
        }
        Ok(db.query_row("PRAGMA journal_mode=DELETE", [], |row| {
            row.get::<_, String>(0)
        })? == "delete")
    };
    check().unwrap_or(false)
}

fn tables(db: &rusqlite::Connection) -> rusqlite::Result<Vec<String>> {
    db.prepare("SELECT name FROM sqlite_schema WHERE type = 'table'")?
        .query_map([], |row| row.get(0))?
        .collect()
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

pub(super) fn rewrite_attachments(
    path: &Path,
    addresses: &mut super::attachments::Addresses,
    unavailable: &mut Vec<super::recovery::Unavailable>,
) -> Result<()> {
    use super::attachments::Format;
    use rusqlite::{OptionalExtension, TransactionBehavior, params};

    fn columns(db: &rusqlite::Connection, table: &str) -> Result<Vec<String>> {
        Ok(db
            .prepare("SELECT name FROM pragma_table_info(?1)")?
            .query_map([table], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }
    fn has(columns: &[String], names: &[&str]) -> bool {
        names
            .iter()
            .all(|name| columns.iter().any(|column| column == name))
    }

    let mut db = open_for_move(path, BUSY)?;
    db.execute_batch("PRAGMA synchronous=FULL;")?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let message_columns = columns(&tx, "message")?;
    for table in ["part", "session_message"] {
        let fields = columns(&tx, table)?;
        if !has(&fields, &["id", "data"]) {
            continue;
        }
        let parent = table == "part"
            && has(&fields, &["message_id"])
            && has(&message_columns, &["id", "data"]);
        let parent_id = if parent { "message_id" } else { "NULL" };
        let mut read = tx.prepare(&format!(
            "SELECT id, data, {parent_id} FROM {table} WHERE id > ?1 ORDER BY id LIMIT 128"
        ))?;
        let mut update = tx.prepare(&format!("UPDATE {table} SET data = ?1 WHERE id = ?2"))?;
        let mut after = String::new();
        loop {
            let rows = read
                .query_map([&after], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if rows.is_empty() {
                break;
            }
            for (id, data, parent_id) in rows {
                after = id;
                let Ok(mut value) = serde_json::from_str::<Value>(&data) else {
                    continue;
                };
                let info = if let Some(parent_id) = &parent_id
                    && value.get("type").and_then(Value::as_str) == Some("text")
                {
                    tx.query_row(
                        "SELECT data FROM message WHERE id = ?1",
                        [parent_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?
                    .and_then(|data| serde_json::from_str::<Value>(&data).ok())
                    .filter(|info| info.get("role").and_then(Value::as_str) == Some("user"))
                } else {
                    None
                };
                if let (Some(info), Some(parent_id)) = (info, parent_id) {
                    let mut envelope = serde_json::json!({ "info": info, "parts": [value] });
                    if addresses.rewrite(&mut envelope, Format::Transcript, unavailable)? {
                        if envelope["info"] != info {
                            tx.execute(
                                "UPDATE message SET data = ?1 WHERE id = ?2",
                                params![serde_json::to_string(&envelope["info"])?, parent_id],
                            )?;
                        }
                        update.execute(params![
                            serde_json::to_string(&envelope["parts"][0])?,
                            &after
                        ])?;
                    }
                } else {
                    let format = if table == "part" {
                        Format::Part
                    } else {
                        Format::Transcript
                    };
                    if addresses.rewrite(&mut value, format, unavailable)? {
                        update.execute(params![serde_json::to_string(&value)?, &after])?;
                    }
                }
            }
        }
    }
    tx.commit()?;
    Ok(())
}

pub(super) fn merge_insights(
    target: Option<&Path>,
    archived: &Path,
    scope: &super::recovery::InsightOrigins,
) -> Result<()> {
    use rusqlite::{OptionalExtension, TransactionBehavior, params};
    let path = target.unwrap_or(archived);
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut side = path.as_os_str().to_owned();
        side.push(suffix);
        if super::inventory::stat(Path::new(&side))?.is_some_and(|info| !info.is_file()) {
            return Err(Error::Message("Insights index contains a link."));
        }
    }
    let mut db = open_for_move(path, BUSY)?;
    db.execute_batch("PRAGMA synchronous=FULL;")?;
    if target.is_some() {
        let archived = archived
            .to_str()
            .ok_or(Error::Message("Invalid Insights archive path."))?;
        db.execute("ATTACH DATABASE ?1 AS archived", [archived])?;
    }
    let schemas: &[&str] = if target.is_some() {
        &["main", "archived"]
    } else {
        &["main"]
    };
    for schema in schemas {
        let v = db.query_row(&format!("PRAGMA {schema}.user_version"), [], |row| {
            row.get::<_, u32>(0)
        })?;
        if !matches!(v, 0 | 1) {
            return Err(Error::Message("Unsupported Insights database"));
        }
    }
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let sequence: Option<String> = tx
        .query_row(
            "SELECT body FROM main.state WHERE key = 'sequence'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let mut seq: u64 = serde_json::from_str(sequence.as_deref().unwrap_or("0"))?;
    if seq > 9_007_199_254_740_991 {
        return Err(Error::Message("Invalid Insights sequence"));
    }
    let origins = serde_json::to_string(&scope.accepted)?;
    if target.is_none() {
        tx.execute(
            "DELETE FROM event WHERE computer NOT IN (SELECT value FROM json_each(?1))",
            [&origins],
        )?;
        tx.commit()?;
        return Ok(());
    }
    let before = seq;
    let recovered = tx.execute(r#"    WITH recovered AS MATERIALIZED (
      SELECT saved.*, ? + ROW_NUMBER() OVER (ORDER BY saved.at, saved.key) AS next,
        kept.computer = saved.computer AND json_extract(kept.body, '$.accountObserved') = 1 AS keep_account,
        kept.account AS prior_account
      FROM archived.event AS saved LEFT JOIN main.event AS kept ON kept.key = saved.key
      WHERE saved.computer IN (SELECT value FROM json_each(?))
        AND (kept.key IS NULL
          OR (saved.computer = kept.computer AND saved.partial < kept.partial)
          OR (saved.computer <> kept.computer AND (
            (kept.computer <> ? AND kept.computer NOT IN (SELECT value FROM json_each(?)))
            OR COALESCE(json_extract(saved.body, '$.attributed'), 0) > COALESCE(json_extract(kept.body, '$.attributed'), 0)
            OR (COALESCE(json_extract(saved.body, '$.attributed'), 0) = COALESCE(json_extract(kept.body, '$.attributed'), 0)
              AND (saved.received, saved.computer) < (kept.received, kept.computer))
          )))
    )
    INSERT OR REPLACE INTO main.event (
      key, seq, sender, received, at, session, computer, bot, bot_name, title,
      provider, account, model, agent, human, partial, cost, tokens, input, cache_read, cache_write, output,
      skills, body, supersedes, processed
    )
    SELECT key, next, sender, received, at, session, computer, bot, bot_name, title,
      provider, CASE WHEN keep_account THEN prior_account ELSE account END,
      model, agent, human, partial, cost, tokens, input, cache_read, cache_write, output, skills,
      CASE WHEN keep_account THEN json_set(body, '$.account', prior_account, '$.accountObserved', json('true')) ELSE body END,
      supersedes, processed FROM recovered
"#, params![seq, origins, scope.own, origins])?;
    seq += recovered as u64;
    let enriched = tx.execute(
        r#"    WITH accounts AS MATERIALIZED (
      SELECT kept.key, saved.account, ? + ROW_NUMBER() OVER (ORDER BY kept.at, kept.key) AS next
      FROM main.event AS kept JOIN archived.event AS saved USING (key)
      WHERE kept.computer = saved.computer AND saved.computer IN (SELECT value FROM json_each(?))
        AND saved.account IS NOT NULL AND json_extract(saved.body, '$.accountObserved') = 1
        AND COALESCE(json_extract(kept.body, '$.accountObserved'), 0) = 0
    )
    UPDATE main.event SET account = accounts.account, seq = accounts.next,
      body = json_set(event.body, '$.account', accounts.account, '$.accountObserved', json('true'))
    FROM accounts WHERE event.key = accounts.key
"#,
        params![seq, origins],
    )?;
    seq += enriched as u64;
    if recovered != 0 {
        tx.execute("DELETE FROM main.state WHERE key = 'prices-v1'", [])?;
    }
    if seq != before {
        tx.execute("INSERT INTO main.state VALUES ('sequence', ?1) ON CONFLICT(key) DO UPDATE SET body = excluded.body", [seq.to_string()])?;
    }
    tx.commit()?;
    Ok(())
}
