//! Peer synchronization of the indexed metadata: durable cursors in SQLite,
//! and peer usage slots in the existing small history cache.
use super::*;
use crate::usage::{
    history_peers,
    insight_mail::{self, Cursor, Mail, Record, Scope},
};

fn valid_cursor(n: u64) -> bool {
    n <= 9_007_199_254_740_991
}
fn small_epoch(s: &str) -> bool {
    !s.is_empty() && s.len() <= 512
}
fn string(db: &Connection, key: &str) -> Result<Option<String>> {
    Ok(get(db, key)?.and_then(|v| v.as_str().map(str::to_owned)))
}
fn integer(db: &Connection, key: &str) -> Result<u64> {
    Ok(get(db, key)?.and_then(|v| v.as_u64()).unwrap_or(0))
}
fn history_scope(scope: &Scope) -> Option<String> {
    (!scope.historical.is_empty()).then(|| {
        hash(&json!([
            "historical Insights v1",
            scope.setup,
            scope.historical.keys().collect::<Vec<_>>()
        ]))
    })
}
fn valid_sender(scope: &Scope, sender: &str) -> bool {
    scope.setup.is_some() && scope.peers.contains_key(sender)
}

impl Service {
    pub fn peer_reads_whole(&self, service: &str, identity: &str, now: i64) -> bool {
        if self.scope().setup.is_none() {
            return false;
        }
        self.history_peers
            .lock()
            .expect("usage peer book")
            .reads_whole(self.own_key(), service, identity, now)
    }
    pub fn cursor_days(
        &self,
        provider: &str,
        user: &str,
        now: i64,
    ) -> Result<(BTreeMap<i64, bool>, Option<i64>)> {
        let _gate = self.gate.lock().expect("insights gate");
        let db = self.open()?;
        Ok((
            super::super::cursor_history_store::kept_days(&db, user, now)?,
            super::super::cursor_history_store::known(&db, provider, user)?,
        ))
    }
    pub fn save_cursor_day(
        &self,
        user: &str,
        day: super::super::cursor_history_store::Day,
        now: i64,
    ) -> Result<()> {
        let _gate = self.gate.lock().expect("insights gate");
        let mut db = self.open()?;
        let pricing = self.pricing()?;
        super::super::cursor_history_store::save_day(&mut db, user, day, &pricing, now)
    }
    pub fn keep_cursor_account(
        &self,
        provider: &str,
        account: &str,
        label: Option<&str>,
        identity: &str,
        user: &str,
        scanned: Option<i64>,
        mark: &str,
    ) -> Result<()> {
        let _gate = self.gate.lock().expect("insights gate");
        let db = self.open()?;
        super::super::cursor_history_store::account(
            &db, provider, account, label, identity, user, scanned, mark,
        )?;
        self.changed_index();
        Ok(())
    }
    pub fn forget_cursor_signin(&self, provider: &str, mark: &str) -> Result<()> {
        let _gate = self.gate.lock().expect("insights gate");
        let db = self.open()?;
        if super::super::cursor_history_store::forget_changed(&db, provider, mark)? {
            self.changed_index();
        }
        Ok(())
    }
    pub fn retain_cursor_accounts(&self, providers: BTreeSet<String>) -> Result<()> {
        let _gate = self.gate.lock().expect("insights gate");
        let db = self.open()?;
        if super::super::cursor_history_store::retain(&db, &providers)? {
            self.changed_index();
        }
        Ok(())
    }
    pub fn subscribe_index(&self) -> tokio::sync::watch::Receiver<u64> {
        self.index_changes.subscribe()
    }
    pub fn subscribe_manual(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.manual_refresh.subscribe()
    }
    pub(super) fn changed_index(&self) {
        self.index_changes.send_modify(|n| *n = n.wrapping_add(1));
        self.summary_revision
            .fetch_add(1, std::sync::atomic::Ordering::Release);
    }
    pub(super) fn request_peer_refresh(&self) {
        let _ = self.manual_refresh.send(());
    }
    pub(super) fn scope(&self) -> Scope {
        self.scope.lock().expect("Insights origins").clone()
    }
    pub fn peer_roster(&self, scope: Scope) -> Result<()> {
        let mut old = self.scope.lock().expect("Insights origins");
        if *old == scope {
            return Ok(());
        }
        let roster = scope.setup.as_ref().map(|_| scope.peers.clone());
        *old = scope;
        drop(old);
        self.summary_revision
            .fetch_add(1, std::sync::atomic::Ordering::Release);
        let Some(roster) = roster else {
            return Ok(());
        };
        let mut book = self.history_peers.lock().expect("usage peer book");
        if book.roster(&roster) {
            book.keep(&self.home.join("agents/usage/peers.json"))?;
        }
        Ok(())
    }
    pub(super) fn local_accounts(
        &self,
        db: &Connection,
        now: i64,
    ) -> Result<Vec<super::super::history::Account>> {
        let identities = self
            .history
            .as_ref()
            .map(|h| {
                h.limits
                    .peer_snapshot(None, now.max(0) as u64)
                    .accounts
                    .into_iter()
                    .filter_map(|a| Some((a.provider, a.identity?)))
                    .collect()
            })
            .unwrap_or_default();
        super::super::history::own(db, now, &identities)
    }
    pub fn history_snapshot(&self, now: i64) -> Result<history_peers::Snapshot> {
        let _gate = self.gate.lock().expect("insights gate");
        let mut db = self.open()?;
        self.price_index(&mut db, now)?;
        Ok(history_peers::Snapshot::new(
            self.local_accounts(&db, now)?,
            now,
        ))
    }
    pub fn take_history(&self, sender: &str, mail: &Value, now: i64) -> Result<Option<bool>> {
        let scope = self.scope();
        if !valid_sender(&scope, sender) {
            return Ok(None);
        }
        let mut book = self.history_peers.lock().expect("usage peer book");
        let ask = book.take(sender, mail, now);
        if mail["accounts"].is_array() && ask.is_some() {
            book.keep(&self.home.join("agents/usage/peers.json"))?;
        }
        Ok(ask)
    }
    pub(super) fn peer_history(&self, now: i64) -> Vec<super::super::history::Computer> {
        if self.scope().setup.is_none() {
            return vec![];
        }
        self.history_peers
            .lock()
            .expect("usage peer book")
            .computers(now)
    }
    pub(super) fn keys(&self, scope: &Scope, selected: Option<&str>) -> (Vec<String>, Vec<String>) {
        let local = [self.own_key().to_owned(), self.alias().to_owned()]
            .into_iter()
            .collect::<BTreeSet<_>>();
        let all = local
            .iter()
            .cloned()
            .chain(scope.eligible())
            .collect::<BTreeSet<_>>();
        let selected = match selected {
            None => all.clone(),
            Some(key) if local.contains(key) => local,
            Some(key) if all.contains(key) => BTreeSet::from([key.to_owned()]),
            _ => BTreeSet::new(),
        };
        (all.into_iter().collect(), selected.into_iter().collect())
    }
    pub(super) fn computers(&self, scope: &Scope) -> BTreeMap<String, String> {
        let mut names = scope.peers.clone();
        names.extend(scope.historical.clone());
        names.insert(self.own_key().into(), self.name.clone());
        names
    }
    pub(super) fn pending_computers(
        &self,
        db: &Connection,
        scope: &Scope,
        selected: Option<&str>,
    ) -> Result<Vec<Value>> {
        let mut pending = vec![];
        for id in &scope.connected {
            if selected.is_some_and(|key| key != id) {
                continue;
            }
            if get(db, &format!("peer/{id}"))?.is_none() {
                if let Some(name) = scope.peers.get(id) {
                    pending.push(json!({"id":id,"name":name}));
                }
            }
        }
        pending.sort_by(|a, b| {
            a["name"]
                .as_str()
                .cmp(&b["name"].as_str())
                .then(a["id"].as_str().cmp(&b["id"].as_str()))
        });
        Ok(pending)
    }
    fn peer_cursor_from(db: &Connection, sender: &str, scope: &Scope) -> Result<Cursor> {
        let scope_hash = history_scope(scope);
        let stored = string(db, &format!("cursor-scope/{sender}"))?;
        if scope_hash != stored {
            return Ok(Cursor::default());
        }
        Ok(Cursor {
            after: integer(db, &format!("cursor/{sender}"))?,
            epoch: string(db, &format!("peer-epoch/{sender}"))?,
        })
    }
    pub fn peer_cursor(&self, sender: &str) -> Result<Option<Cursor>> {
        let scope = self.scope();
        if !valid_sender(&scope, sender) {
            return Ok(None);
        }
        let _gate = self.gate.lock().expect("insights gate");
        let db = self.open()?;
        Ok(Some(Self::peer_cursor_from(&db, sender, &scope)?))
    }
    pub fn served_cursor(&self, sender: &str) -> Result<Option<Cursor>> {
        let scope = self.scope();
        if !valid_sender(&scope, sender) {
            return Ok(None);
        }
        let _gate = self.gate.lock().expect("insights gate");
        let db = self.open()?;
        Ok(get(&db, &format!("served/{sender}"))?.and_then(|v| serde_json::from_value(v).ok()))
    }
    pub fn peer_page(&self, sender: &str, cursor: Cursor, remember: bool) -> Result<Option<Mail>> {
        let scope = self.scope();
        if !valid_sender(&scope, sender)
            || !valid_cursor(cursor.after)
            || cursor.epoch.as_ref().is_some_and(|s| !small_epoch(s))
        {
            return Ok(None);
        }
        let _gate = self.gate.lock().expect("insights gate");
        let mut db = self.open()?;
        let tx = db.transaction()?;
        if remember {
            set(
                &tx,
                &format!("served/{sender}"),
                &serde_json::to_value(&cursor)?,
            )?;
        }
        let index_epoch = if let Some(epoch) = string(&tx, "epoch")? {
            epoch
        } else {
            let mut bytes = [0u8; 16];
            getrandom::fill(&mut bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
            let epoch = hex::encode(bytes);
            set(&tx, "epoch", &json!(epoch))?;
            epoch
        };
        let (eligible, _) = self.keys(&scope, None);
        let export_scope = hash(&json!([scope.setup, eligible]));
        let epoch = hash(&json!([index_epoch, export_scope]));
        let mut sequence = integer(&tx, "sequence")?;
        // A survivor can relay a historical origin after the signed membership
        // accepted its retirement. Current peers' copied rows are not re-exported.
        if !scope.historical.is_empty() {
            let promoted=tx.execute("WITH pending AS MATERIALIZED (SELECT key, ?1+ROW_NUMBER() OVER (ORDER BY at,key) AS seq FROM event WHERE seq IS NULL AND computer IN (SELECT value FROM json_each(?2))) UPDATE event SET seq=pending.seq FROM pending WHERE event.key=pending.key",params![sequence,serde_json::to_string(&scope.historical.keys().collect::<Vec<_>>())?])?;
            sequence += promoted as u64;
            if promoted > 0 {
                set(&tx, "sequence", &json!(sequence))?;
            }
        }
        let reset =
            cursor.after > sequence || cursor.epoch.as_ref().is_some_and(|old| old != &epoch);
        let start = if reset { 0 } else { cursor.after };
        let mut next = start;
        let more;
        let mut records = vec![];
        let now = Timestamp::now().as_millisecond();
        let seen = scope.eligible();
        {
            let mut query = tx.prepare(
                "SELECT seq,body,received FROM event WHERE seq>? ORDER BY seq LIMIT 100",
            )?;
            let mut rows = query.query([start])?;
            let mut decoded = 0;
            let mut bytes = 0usize;
            let mut limited = false;
            while let Some(row) = rows.next()? {
                let at_seq: u64 = row.get(0)?;
                let body: String = row.get(1)?;
                let Ok(mut record) = serde_json::from_str::<Record>(&body) else {
                    next = at_seq;
                    decoded += 1;
                    continue;
                };
                record.observed_at = Some(row.get(2)?);
                if record.computer == self.alias() {
                    record.computer = self.own_key().into();
                    if let Some(a) = record.account.as_mut() {
                        if let Some(tail) = a.strip_prefix(&format!("{}/", self.alias())) {
                            *a = format!("{}/{}", self.own_key(), tail);
                        }
                    }
                }
                if record.validate(now, &seen) {
                    let size = serde_json::to_vec(&record)?.len();
                    if !records.is_empty() && bytes.saturating_add(size) > 400_000 {
                        limited = true;
                        break;
                    }
                    bytes += size;
                    records.push(record);
                }
                next = at_seq;
                decoded += 1;
            }
            more = limited || decoded == 100;
        }
        let scanned = self.counted(&tx)?.filter(|_| self.ready());
        let issues = self
            .issues
            .lock()
            .expect("usage issues")
            .iter()
            .cloned()
            .collect();
        tx.commit()?;
        Ok(Some(Mail::Page {
            after: cursor.after,
            next,
            more,
            epoch,
            reset,
            scanned,
            issues,
            records,
        }))
    }
    pub fn take_page(&self, sender: &str, mail: Mail) -> Result<Option<Cursor>> {
        let scope = self.scope();
        let Mail::Page {
            after,
            next,
            more,
            epoch,
            reset,
            scanned,
            issues,
            records,
        } = mail
        else {
            return Ok(None);
        };
        if !valid_sender(&scope, sender)
            || !valid_cursor(after)
            || !valid_cursor(next)
            || (!reset && next < after)
            || !small_epoch(&epoch)
            || records.len() > 100
        {
            return Ok(None);
        }
        let now = Timestamp::now().as_millisecond();
        let pricing = self.pricing()?;
        let _gate = self.gate.lock().expect("insights gate");
        let mut db = self.open()?;
        let tx = db.transaction()?;
        let expected = Self::peer_cursor_from(&tx, sender, &scope)?;
        if after != expected.after
            || expected
                .epoch
                .as_ref()
                .is_some_and(|old| old != &epoch && !reset)
        {
            return Ok(None);
        }
        let mut seq = integer(&tx, "sequence")?;
        let origins = scope.eligible();
        for mut record in records {
            if record.validate(now, &origins) {
                self.take_record(&tx, &mut seq, sender, record, now, &scope, &pricing)?;
            }
        }
        set(&tx, "sequence", &json!(seq))?;
        set(&tx, &format!("cursor/{sender}"), &json!(next))?;
        set(&tx, &format!("peer-epoch/{sender}"), &json!(epoch))?;
        set(
            &tx,
            &format!("cursor-scope/{sender}"),
            &json!(history_scope(&scope)),
        )?;
        if !more && scanned.is_some_and(|at| at > 0 && at <= now + DAY) {
            let issues = issues
                .into_iter()
                .filter(|s| !s.is_empty() && s.len() <= 512)
                .take(12)
                .collect::<Vec<_>>();
            set(
                &tx,
                &format!("peer/{sender}"),
                &json!({"scanned":scanned,"issues":issues}),
            )?;
        }
        tx.execute("DELETE FROM event WHERE at<?", [now - 366 * DAY])?;
        tx.commit()?;
        self.summary_revision
            .fetch_add(1, std::sync::atomic::Ordering::Release);
        // Acknowledge even the final nonempty page by asking at its new cursor.
        // An empty final reply stops the exchange. Later live pages can be pushed
        // from the acknowledged cursor without waiting for the next heartbeat.
        Ok((reset || next > after).then_some(Cursor {
            after: next,
            epoch: Some(epoch),
        }))
    }
    fn take_record(
        &self,
        db: &Connection,
        seq: &mut u64,
        sender: &str,
        mut record: Record,
        now: i64,
        scope: &Scope,
        pricing: &Pricing,
    ) -> Result<()> {
        let prior: Option<(String, Option<u64>, Option<String>, i64)> = db
            .query_row(
                "SELECT body,seq,sender,received FROM event WHERE key=?",
                [&record.key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let first_seen = record
            .observed_at
            .take()
            .map_or(now, |at| at.max(record.at).min(now));
        let mut received = first_seen;
        let mut source = Some(sender.to_owned());
        let mut export = false;
        if let Some((body, old_seq, old_sender, old_received)) = prior {
            let before: Record = serde_json::from_str(&body)?;
            let local_equivalent =
                |computer: &str| computer == self.alias() || computer == self.own_key();
            let same = before.computer == record.computer
                || local_equivalent(&before.computer) && local_equivalent(&record.computer);
            if !same {
                let eligible = scope.eligible().contains(&before.computer)
                    || local_equivalent(&before.computer);
                if eligible
                    && (before.attributed && !record.attributed
                        || before.attributed == record.attributed
                            && (old_received, &before.computer) <= (first_seen, &record.computer))
                {
                    return Ok(());
                }
            }
            let historical = same && scope.historical.contains_key(&record.computer);
            let keep_local =
                old_sender.is_none() && same && (!record.attributed || before.attributed);
            let quality = |r: &Record| {
                if r.tokens.is_none() {
                    0
                } else if r.partial
                    || r.tokens
                        .as_ref()
                        .and_then(insight_mail::Buckets::complete)
                        .is_none()
                {
                    1
                } else {
                    2
                }
            };
            let keep = if historical {
                (
                    before.attributed,
                    quality(&before),
                    std::cmp::Reverse(old_received),
                ) > (
                    record.attributed,
                    quality(&record),
                    std::cmp::Reverse(first_seen),
                ) || (before.attributed, quality(&before), old_received)
                    == (record.attributed, quality(&record), first_seen)
                    && serde_json::to_string(&before)? <= serde_json::to_string(&record)?
            } else {
                before.tokens.is_some() && (record.tokens.is_none() || keep_local)
            };
            if keep {
                if !same
                    || before.account_observed
                    || !record.account_observed
                    || record.account.is_none()
                {
                    return Ok(());
                }
                let account = record.account.take();
                record = before.clone();
                record.account = account;
                record.account_observed = true;
                if keep_local {
                    source = None;
                }
            }
            if same && before.account_observed {
                record.account = before.account.clone();
                record.account_observed = true;
            }
            if before.session == record.session {
                if record.root.is_none() {
                    record.root = before.root.clone();
                }
                if record.bot.is_none() {
                    record.bot = before.bot.clone();
                    record.bot_name = before.bot_name.clone();
                    record.bot_own = before.bot_own;
                }
            }
            if same && local_equivalent(&before.computer) {
                record.computer = before.computer.clone();
            }
            received = if historical {
                if keep { old_received } else { first_seen }
            } else if same {
                old_received.min(first_seen)
            } else {
                first_seen
            };
            if serde_json::to_string(&record)? == body && (!historical || received == old_received)
            {
                return Ok(());
            }
            export = old_seq.is_some();
        }
        let body = serde_json::to_value(&record)?;
        let t = record.tokens.as_ref();
        let fields = t.map(insight_mail::Buckets::fields).unwrap_or([None; 5]);
        let [input, output, cache, write5, write1] = fields;
        let count = insight_mail::total([input, output, write5, write1]);
        let processed = record
            .reported_total
            .or_else(|| insight_mail::total(fields));
        let creation = insight_mail::total([write5, write1]);
        let event_seq = if export {
            *seq += 1;
            Some(*seq)
        } else {
            None
        };
        db.execute("INSERT OR REPLACE INTO event VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",params![
            record.key,event_seq,source,received,record.at,record.session,record.computer,record.bot,record.bot_name,record.title,record.provider,record.account,record.model,record.agent,record.human,record.is_partial(),estimate_body(&body,pricing,now),count,input,cache,creation,output,serde_json::to_string(record.skills.as_deref().unwrap_or(&[]))?,serde_json::to_string(&body)?,record.supersedes,processed
        ])?;
        Ok(())
    }
}
