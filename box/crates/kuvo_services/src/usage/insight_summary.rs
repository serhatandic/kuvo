//! SQL reduces the period to grouped session, provider/model, and computer
//! rows; opening Insights never materialises all of its calls in memory.
use super::*;

pub(super) struct Kept {
    until: i64,
    revision: u64,
    value: Value,
}
/// A bounded snapshot of the last complete summary, separate from the busy index.
/// It is consulted only when catch-up is still running and only under the same
/// admitted computer set. Routine ready reads remain mutation-invalidated SQL.
#[derive(Deserialize, Serialize)]
struct Snapshot {
    at: i64,
    data: Value,
}
#[derive(Default, Deserialize, Serialize)]
pub(super) struct KeptSummaries(BTreeMap<String, Snapshot>);
impl KeptSummaries {
    fn load(path: &Path) -> Self {
        kuvo_store::files::text_of(path)
            .and_then(|text| serde_json::from_str::<Self>(&text).ok())
            .unwrap_or_default()
    }
    fn previous(&self, key: &str) -> Option<Value> {
        let found = self.0.get(key)?;
        if found.data["data"]["state"] == "loading" {
            return None;
        }
        let mut answer = found.data.clone();
        answer["data"]["asOf"] = json!(found.at);
        Some(answer)
    }
    fn keep(&mut self, path: &Path, key: String, now: i64, answer: &Value) -> Result<()> {
        self.0.insert(
            key,
            Snapshot {
                at: now,
                data: answer.clone(),
            },
        );
        while self.0.len() > 24 {
            let Some(oldest) = self
                .0
                .iter()
                .min_by_key(|(_, snapshot)| snapshot.at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.0.remove(&oldest);
        }
        kuvo_store::files::write_atomic(path, &serde_json::to_vec(self)?)?;
        Ok(())
    }
}
const SUMS: &str = "COALESCE(SUM(cost),0),COALESCE(SUM(processed),0),COALESCE(SUM(cache_read),0),COALESCE(SUM(input),0)+COALESCE(SUM(cache_read),0)+COALESCE(SUM(cache_write),0)";
fn sums(row: &rusqlite::Row<'_>, i: usize) -> rusqlite::Result<Sums> {
    Ok(Sums {
        cost: row.get(i)?,
        tokens: row.get(i + 1)?,
        cached: row.get(i + 2)?,
        input: row.get(i + 3)?,
    })
}
fn capital(s: &str) -> String {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    first.to_uppercase().chain(chars).collect()
}
fn provider_name(id: &str) -> String {
    let base = id.split('@').next().unwrap_or(id);
    match base {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "cursor" => "Cursor",
        "grok" => "Grok",
        "antigravity" => "Antigravity",
        "opencode" => "OpenCode",
        "opencode-go" => "OpenCode Go",
        "openai" => "OpenAI",
        "anthropic" => "Anthropic",
        "google" => "Google",
        "openrouter" => "OpenRouter",
        "github-copilot" | "copilot" => "GitHub Copilot",
        "xai" => "xAI",
        "zai" | "zai-coding-plan" => "Z.ai",
        "ollama" => "Ollama",
        "mistral" => "Mistral",
        "deepseek" => "DeepSeek",
        "engine" => "Kuvo",
        _ => {
            let name = base
                .split(['-', '_'])
                .filter(|s| !s.is_empty())
                .map(capital)
                .collect::<Vec<_>>()
                .join(" ");
            return if name.is_empty() {
                "Unknown".into()
            } else {
                name
            };
        }
    }
    .into()
}
fn digits(s: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}
fn version(s: &str) -> bool {
    digits(s, 1, 2)
}
fn words(parts: &[&str]) -> Vec<String> {
    let mut out = vec![];
    let mut i = 0;
    while i < parts.len() {
        let part = parts[i];
        if version(part) {
            let mut next = i + 1;
            while next < parts.len() && version(parts[next]) {
                next += 1;
            }
            out.push(parts[i..next].join("."));
            i = next;
            continue;
        }
        let name = if part.as_bytes().first() == Some(&b'o')
            && part.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
        {
            part.into()
        } else if matches!(part, "gpt" | "glm" | "oss") {
            part.to_ascii_uppercase()
        } else {
            capital(part)
        };
        out.push(name);
        i += 1;
    }
    out
}
pub(super) fn model_name(id: Option<&str>) -> (String, String) {
    let lower = id.unwrap_or("").trim().to_lowercase();
    let raw = lower.rsplit('/').next().unwrap_or("");
    let raw = if raw.ends_with(']') {
        raw.rsplit_once('[').map_or(raw, |(head, _)| head)
    } else {
        raw
    };
    let raw = raw.split('@').next().unwrap_or("");
    let mut parts = raw
        .split(|c: char| c.is_whitespace() || matches!(c, '-' | '_' | '.' | ':'))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    while parts.len() > 1
        && parts
            .last()
            .is_some_and(|s| digits(s, 6, 8) || *s == "latest")
    {
        parts.pop();
    }
    if parts.len() > 3 {
        let n = parts.len();
        if digits(parts[n - 3], 4, 4) && digits(parts[n - 2], 2, 2) && digits(parts[n - 1], 2, 2) {
            parts.truncate(n - 3);
        }
    }
    if parts.is_empty() {
        return ("unknown".into(), "Unknown model".into());
    }
    let name = if let Some(family) = parts
        .iter()
        .find(|s| matches!(**s, "opus" | "sonnet" | "haiku" | "fable"))
    {
        let numbers = parts
            .iter()
            .copied()
            .filter(|s| version(s))
            .collect::<Vec<_>>()
            .join(".");
        let rest = parts
            .iter()
            .copied()
            .filter(|s| *s != "claude" && *s != *family && !version(s))
            .collect::<Vec<_>>();
        let mut out = vec![capital(family)];
        if !numbers.is_empty() {
            out.push(numbers);
        }
        out.extend(words(&rest));
        out.join(" ")
    } else if parts[0] == "gpt" {
        let after = &parts[1..];
        let mut n = 0;
        while n < after.len()
            && (version(after[n])
                || n == 0
                    && after[n]
                        .as_bytes()
                        .last()
                        .is_some_and(u8::is_ascii_lowercase)
                    && digits(&after[n][..after[n].len() - 1], 1, usize::MAX))
        {
            n += 1;
        }
        let mut out = vec![if n > 0 {
            format!("GPT-{}", after[..n].join("."))
        } else {
            "GPT".into()
        }];
        out.extend(words(&after[n..]));
        out.join(" ")
    } else {
        words(&parts).join(" ")
    };
    let mut key = String::new();
    let mut separator = false;
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() || c == '.' {
            if separator {
                key.push('-');
                separator = false;
            }
            key.push(c);
        } else {
            separator = true;
        }
    }
    if separator {
        key.push('-');
    }
    (key, name)
}
#[derive(Clone)]
struct SessionRow {
    id: String,
    title: Option<String>,
    bot: Option<String>,
    name: Option<String>,
    root: Option<String>,
    own: bool,
    sums: Sums,
}
#[derive(Default)]
struct Top {
    title: Option<String>,
    bot: Option<String>,
    name: Option<String>,
    own: bool,
    subagents: usize,
    sums: Sums,
}
fn measures(s: &Sums, overall: Option<f64>) -> Value {
    let mut row = s.measures();
    if s.rate()
        .zip(overall)
        .is_some_and(|(rate, overall)| rate <= overall - 0.15)
    {
        row["lowCache"] = json!(true);
    }
    row
}
fn title(t: Option<&str>) -> &str {
    t.filter(|s| !s.is_empty()).unwrap_or("Untitled session")
}
fn item(kind: &str, id: &str, name: &str, s: &Sums, overall: Option<f64>) -> Value {
    let mut row = measures(s, overall);
    row["kind"] = json!(kind);
    row["id"] = json!(id);
    row["name"] = json!(name);
    row
}
fn conversation(id: &str, top: &Top, alone: bool, overall: Option<f64>) -> Value {
    let mut row = item(
        "session",
        id,
        title(top.title.as_deref()),
        &top.sums,
        overall,
    );
    if top.own {
        row["note"] = json!("Own chat");
    } else if top.subagents != 0 {
        row["note"] = json!(format!(
            "{} subagent{}",
            top.subagents,
            if top.subagents == 1 { "" } else { "s" }
        ));
    } else if alone {
        row["note"] = json!("No bot");
    }
    row
}
fn rank_rows(mut rows: Vec<Value>, limit: Option<usize>) -> Vec<Value> {
    rows.sort_by(|a, b| {
        b["cost"]
            .as_f64()
            .unwrap_or_default()
            .total_cmp(&a["cost"].as_f64().unwrap_or_default())
            .then(b["tokens"].as_u64().cmp(&a["tokens"].as_u64()))
    });
    if let Some(limit) = limit {
        rows.truncate(limit);
    }
    rows
}
fn standouts(tops: &BTreeMap<String, Top>, overall: Option<f64>) -> Vec<Value> {
    let candidates = tops.iter().filter(|(_, top)| !top.own).collect::<Vec<_>>();
    if candidates.len() < 4 {
        return vec![];
    }
    let mut picked = BTreeSet::<String>::new();
    let mut result = vec![];
    let take = |id: &str, top: &Top, reason: &str, kind: &str| {
        let mut row = measures(&top.sums, overall);
        row["id"] = json!(id);
        row["name"] = json!(title(top.title.as_deref()));
        row["reason"] = json!(reason);
        row["kind"] = json!(kind);
        row
    };
    if let Some((id, top)) = candidates
        .iter()
        .max_by(|(_, a), (_, b)| a.sums.cost.total_cmp(&b.sums.cost))
        .filter(|(_, top)| top.sums.cost > 0.)
    {
        picked.insert((*id).clone());
        result.push(take(id, top, "Highest cost", "cost"));
    }
    let mut costs = candidates
        .iter()
        .map(|(_, t)| t.sums.cost)
        .collect::<Vec<_>>();
    costs.sort_by(f64::total_cmp);
    let middle = costs[costs.len() / 2];
    if let Some((id, top)) = candidates
        .iter()
        .filter(|(id, t)| {
            !picked.contains(*id)
                && t.sums.cost >= middle
                && t.sums
                    .rate()
                    .zip(overall)
                    .is_some_and(|(rate, overall)| rate <= overall - 0.15)
        })
        .min_by(|(_, a), (_, b)| {
            a.sums
                .rate()
                .unwrap_or(1.)
                .total_cmp(&b.sums.rate().unwrap_or(1.))
        })
    {
        picked.insert((*id).clone());
        result.push(take(id, top, "Low cache", "cache"));
    }
    if let Some((id, top)) = candidates
        .iter()
        .filter(|(id, t)| !picked.contains(*id) && t.subagents >= 3)
        .max_by(|(_, a), (_, b)| {
            a.subagents
                .cmp(&b.subagents)
                .then(a.sums.cost.total_cmp(&b.sums.cost))
        })
    {
        result.push(take(
            id,
            top,
            &format!("{} subagents", top.subagents),
            "subagents",
        ));
    }
    result
}
fn where_summary(
    db: &Connection,
    keys: &[String],
    selected: &[String],
    params: &BTreeMap<String, String>,
    start: i64,
    now: i64,
    prior: i64,
) -> Result<Filter> {
    use rusqlite::types::Value as Sql;
    let mut given = params.clone();
    given.remove("session");
    let mut filter = Filter::new(keys, selected, &given, start, now + 1);
    if let Some(session) = params.get("session").filter(|s| !s.is_empty()) {
        filter.values.push(Sql::Text(session.clone()));
        let n = filter.values.len();
        filter.clauses.push_str(&format!(
            " AND (event.session=?{n} OR json_extract(event.body,'$.root')=?{n})"
        ));
    }
    if let Some(model) = params.get("model").filter(|s| !s.is_empty()) {
        let mut query=db.prepare("SELECT DISTINCT model FROM event WHERE at>=? AND computer IN (SELECT value FROM json_each(?))")?;
        let rows = query.query_map(params![prior, serde_json::to_string(selected)?], |row| {
            row.get::<_, Option<String>>(0)
        })?;
        let mut models = vec![];
        let mut null = false;
        for row in rows {
            let row = row?;
            if model_name(row.as_deref()).0 == *model {
                if let Some(id) = row {
                    models.push(id);
                } else {
                    null = true;
                }
            }
        }
        filter
            .values
            .push(Sql::Text(serde_json::to_string(&models)?));
        let n = filter.values.len();
        filter.clauses.push_str(&format!(
            " AND (event.model IN (SELECT value FROM json_each(?{n})){})",
            if null { " OR event.model IS NULL" } else { "" }
        ));
    }
    Ok(filter)
}
impl Service {
    pub(super) fn summary(&self, params: BTreeMap<String, String>) -> Result<Value> {
        let now = super::super::limits::now() as i64;
        let period = params
            .get("period")
            .map(|s| s.parse::<i32>())
            .transpose()?
            .unwrap_or(30);
        if ![7, 30, 90].contains(&period) {
            return Err("Choose a 7, 30 or 90 day period.".into());
        }
        let zone = params
            .get("zone")
            .and_then(|z| TimeZone::get(z).ok())
            .unwrap_or_else(TimeZone::system);
        let bounds = time_bounds(now, &zone, period * 2)?;
        let scope = self.scope();
        let (keys, selected) = self.keys(&scope, params.get("computer").map(String::as_str));
        // The full admitted scope (including the setup identity) binds a snapshot;
        // a removed computer's former records cannot be returned via this cache.
        let mut narrowed = params.clone();
        let fresh = narrowed.remove("refresh").as_deref() == Some("1");
        let kept_key = hash(&json!([
            narrowed,
            scope.setup,
            keys,
            selected,
            zone.iana_name()
        ]));
        let kept_path = self.home.join("agents/usage/insights-kept-rust.json");
        if !fresh && !self.ready() {
            let mut slot = self.kept_summary.lock().expect("kept Insights summary");
            let kept = slot.get_or_insert_with(|| KeptSummaries::load(&kept_path));
            if let Some(previous) = kept.previous(&kept_key) {
                return Ok(previous);
            }
        }
        let _gate = self.gate.lock().expect("insights gate");
        let mut db = self.open()?;
        self.ensure(&mut db, now)?;
        let scope = self.scope();
        let (keys, selected) = self.keys(&scope, params.get("computer").map(String::as_str));
        let kept_key = hash(&json!([
            narrowed,
            scope.setup,
            keys,
            selected,
            zone.iana_name()
        ]));
        let pending =
            self.pending_computers(&db, &scope, params.get("computer").map(String::as_str))?;
        let named = self.computers(&scope);
        let revision = self
            .summary_revision
            .load(std::sync::atomic::Ordering::Acquire);
        let cache_key = hash(&json!([
            params,
            keys,
            selected,
            bounds[period as usize].1,
            zone.iana_name()
        ]));
        if !fresh && self.ready() && pending.is_empty() {
            let cache = self.summary_cache.lock().expect("Insights summary cache");
            if let Some(kept) = cache
                .get(&cache_key)
                .filter(|kept| kept.revision == revision && kept.until >= now)
            {
                return Ok(kept.value.clone());
            }
        }

        let start = bounds[period as usize].1;
        let filter = where_summary(&db, &keys, &selected, &params, start, now, bounds[0].1)?;
        let wh = &filter.clauses;
        let rows = {
            let mut query=db.prepare(&format!("SELECT session,MAX(title),MAX(bot),MAX(bot_name),MAX(json_extract(body,'$.root')),COALESCE(MAX(json_extract(body,'$.botOwn')),0),{SUMS} FROM event WHERE {wh} GROUP BY session"))?;
            let rows = query.query_map(filter.args(), |r| {
                Ok(SessionRow {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    bot: r.get(2)?,
                    name: r.get(3)?,
                    root: r.get(4)?,
                    own: r.get::<_, i64>(5)? != 0,
                    sums: sums(r, 6)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut total = Sums::default();
        let mut tops = BTreeMap::<String, Top>::new();
        for row in &rows {
            total.add(&row.sums);
            let id = row.root.as_ref().unwrap_or(&row.id);
            let top = tops.entry(id.clone()).or_default();
            top.sums.add(&row.sums);
            if id == &row.id {
                if row.title.is_some() {
                    top.title = row.title.clone();
                }
                top.own |= row.own;
                if row.bot.is_some() {
                    top.bot = row.bot.clone();
                    top.name = row.name.clone();
                }
            } else {
                top.subagents += 1;
            }
            if top.bot.is_none() {
                top.bot = row.bot.clone();
                top.name = row.name.clone();
            }
        }
        let missing = tops
            .iter()
            .filter(|(_, top)| top.title.is_none())
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            let mut q=db.prepare("SELECT session,MAX(title) FROM event WHERE session IN (SELECT value FROM json_each(?)) AND computer IN (SELECT value FROM json_each(?)) GROUP BY session")?;
            let titles = q.query_map(
                params![
                    serde_json::to_string(&missing)?,
                    serde_json::to_string(&keys)?
                ],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
            )?;
            for row in titles {
                let (id, title) = row?;
                if let Some(top) = tops.get_mut(&id) {
                    top.title = title;
                }
            }
        }
        let overall = total.rate();
        let mut by_bot = BTreeMap::<String, Vec<(&String, &Top)>>::new();
        let mut loose = vec![];
        for (id, top) in &tops {
            if let Some(bot) = &top.bot {
                by_bot.entry(bot.clone()).or_default().push((id, top));
            } else {
                loose.push(conversation(id, top, true, overall));
            }
        }
        let sessions = if params.contains_key("session") {
            rank_rows(
                rows.iter()
                    .map(|row| {
                        item(
                            "session",
                            &row.id,
                            title(
                                row.title
                                    .as_deref()
                                    .or_else(|| tops.get(&row.id).and_then(|t| t.title.as_deref())),
                            ),
                            &row.sums,
                            overall,
                        )
                    })
                    .collect(),
                None,
            )
        } else if params.contains_key("bot") {
            rank_rows(
                by_bot
                    .values()
                    .flatten()
                    .map(|(id, top)| conversation(id, top, false, overall))
                    .collect(),
                None,
            )
        } else {
            let appearances = lineage::Bots::read(&self.home);
            let remote = self
                .peer_bots()
                .into_iter()
                .filter_map(|(_, bot)| Some((bot.id.clone()?, bot)))
                .collect::<BTreeMap<_, _>>();
            for (id, children) in by_bot {
                let mut sums = Sums::default();
                let mut overseen = 0;
                for (_, top) in &children {
                    sums.add(&top.sums);
                    if !top.own {
                        overseen += 1;
                    }
                }
                let appearance = appearances.appearance(&id);
                let peer = remote.get(&id);
                let name = children
                    .iter()
                    .find_map(|(_, top)| top.name.as_deref())
                    .or_else(|| appearance.map(|(name, _)| name))
                    .or_else(|| peer.map(|bot| bot.name.as_str()))
                    .unwrap_or("A bot");
                let mut row = item("bot", &id, name, &sums, overall);
                let look = match appearance {
                    Some((_, look)) => look.cloned(),
                    None => peer
                        .and_then(|bot| bot.look.as_ref())
                        .and_then(|look| serde_json::to_value(look).ok()),
                };
                if let Some(look) = look.filter(|look| {
                    ["color", "shape", "face"]
                        .iter()
                        .all(|field| look[*field].is_string())
                }) {
                    row["look"] =
                        json!({"color":look["color"],"shape":look["shape"],"face":look["face"]});
                }
                if overseen > 0 {
                    row["note"] = json!(format!(
                        "{} session{}",
                        overseen,
                        if overseen == 1 { "" } else { "s" }
                    ));
                }
                row["sessions"] = json!(rank_rows(
                    children
                        .iter()
                        .map(|(id, top)| conversation(id, top, false, overall))
                        .collect(),
                    None
                ));
                loose.push(row);
            }
            rank_rows(loose, None)
        };
        let mut providers = BTreeMap::<String, Sums>::new();
        let mut models = BTreeMap::<String, (String, Sums, BTreeMap<String, Sums>)>::new();
        {
            let mut query=db.prepare(&format!("SELECT provider,model,{SUMS} FROM event WHERE {wh} AND human=0 GROUP BY provider,model"))?;
            let rows = query.query_map(filter.args(), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    sums(r, 2)?,
                ))
            })?;
            for row in rows {
                let (provider, model, s) = row?;
                providers.entry(provider.clone()).or_default().add(&s);
                let (key, name) = model_name(model.as_deref());
                let entry = models
                    .entry(key)
                    .or_insert_with(|| (name, Sums::default(), BTreeMap::new()));
                entry.1.add(&s);
                entry.2.entry(provider).or_default().add(&s);
            }
        }
        let providers = rank_rows(
            providers
                .into_iter()
                .map(|(id, s)| {
                    let mut row = item("provider", &id, &provider_name(&id), &s, overall);
                    row["provider"] = json!(id);
                    row
                })
                .collect(),
            Some(200),
        );
        let models = rank_rows(
            models
                .into_iter()
                .map(|(id, (name, s, providers))| {
                    let provider = providers
                        .into_iter()
                        .max_by(|(_, a), (_, b)| {
                            (a.cost + a.tokens as f64 / 1e9)
                                .total_cmp(&(b.cost + b.tokens as f64 / 1e9))
                        })
                        .map(|(p, _)| p);
                    let mut row = item("model", &id, &name, &s, overall);
                    row["provider"] = json!(provider);
                    row
                })
                .collect(),
            Some(200),
        );
        let computers = {
            let mut query = db.prepare(&format!(
                "SELECT computer,{SUMS} FROM event WHERE {wh} AND human=0 GROUP BY computer"
            ))?;
            let rows =
                query.query_map(filter.args(), |r| Ok((r.get::<_, String>(0)?, sums(r, 1)?)))?;
            let mut groups = BTreeMap::<String, Sums>::new();
            for row in rows {
                let (id, s) = row?;
                groups.entry(id).or_default().add(&s);
            }
            rank_rows(
                groups
                    .into_iter()
                    .map(|(id, s)| {
                        let name = if id == self.alias() {
                            self.name.as_str()
                        } else {
                            named
                                .get(&id)
                                .map(String::as_str)
                                .unwrap_or("Another computer")
                        };
                        item("computer", &id, name, &s, overall)
                    })
                    .collect(),
                Some(200),
            )
        };
        let prior_filter = filter.range(bounds[0].1, start);
        let (prior, calls) = db.query_row(
            &format!("SELECT {SUMS},COUNT(*) FILTER(WHERE human=0) FROM event WHERE {wh}"),
            prior_filter.args(),
            |r| Ok((sums(r, 0)?, r.get::<_, u64>(4)?)),
        )?;
        let change=(calls>0).then(||json!({"cost":(prior.cost>0.).then(||(total.cost-prior.cost)/prior.cost),"tokens":(prior.tokens>0).then(||(total.tokens as f64-prior.tokens as f64)/prior.tokens as f64),"cacheRate":overall.zip(prior.rate()).map(|(a,b)|a-b)}));
        let mut trend = vec![];
        {
            let mut query=db.prepare(&format!("SELECT COALESCE(SUM(cost),0),COALESCE(SUM(processed),0) FROM event INDEXED BY event_time WHERE {wh}"))?;
            for (i, (name, from)) in bounds.iter().enumerate().skip(period as usize) {
                let until = bounds.get(i + 1).map_or(now + 1, |(_, at)| *at);
                let range = filter.range(*from, until);
                trend.push(query.query_row(range.args(), |r| {
                    Ok(json!({"day":name,"cost":r.get::<_,f64>(0)?,"tokens":r.get::<_,u64>(1)?}))
                })?);
            }
        }
        let scoped = params.contains_key("session") || params.contains_key("bot");
        let counted = self.counted(&db)?;
        let ready = self.ready() && pending.is_empty();
        let mut answer = json!({"data":{"state":if !ready{"loading"}else if rows.is_empty(){"empty"}else{"ready"},"period":period,"zone":zone.iana_name().unwrap_or("UTC"),"pendingComputers":pending,"totals":measures(&total,overall),"change":change,"trend":trend,"lists":[{"key":if scoped{"sessions"}else{"bots"},"name":if scoped{"Sessions"}else{"Bots"},"rows":sessions},{"key":"providers","name":"Providers","rows":providers},{"key":"models","name":"Models","rows":models},{"key":"computers","name":"Computers","rows":computers}],"standouts":standouts(&tops,overall)}});
        if let Some(counted) = counted {
            answer["data"]["countedAt"] = json!(counted);
        }
        if ready {
            // Take the on-disk snapshot only once per completed view, not during
            // an incremental scan. Its failure must not make a valid summary fail.
            {
                let mut slot = self.kept_summary.lock().expect("kept Insights summary");
                let kept = slot.get_or_insert_with(|| KeptSummaries::load(&kept_path));
                let _ = kept.keep(&kept_path, kept_key, now, &answer);
            }
            let mut cache = self.summary_cache.lock().expect("Insights summary cache");
            if cache.len() >= 24 {
                cache.clear();
            }
            cache.insert(
                cache_key,
                Kept {
                    until: now + 10_000,
                    revision,
                    value: answer.clone(),
                },
            );
        }
        Ok(answer)
    }
}

#[cfg(test)]
mod kept_tests {
    use super::*;
    use std::sync::atomic::Ordering::Release;

    #[test]
    fn a_completed_summary_survives_restart_and_does_not_wait_for_the_catch_up_gate() {
        let folder = tempfile::tempdir().unwrap();
        let service = Service::new(folder.path().into(), "Fixture".into(), "".into(), "fixture");
        let params = BTreeMap::from([("period".into(), "7".into()), ("zone".into(), "UTC".into())]);
        service.caught_up.store(true, Release);
        service.native_ready.store(true, Release);
        service.engine_ready.store(true, Release);
        let first = service.summary(params.clone()).unwrap();
        assert_eq!(first["data"]["state"], "empty");
        drop(service);

        let reopened = Service::new(folder.path().into(), "Fixture".into(), "".into(), "fixture");
        let gate = reopened.gate.lock().unwrap();
        std::thread::scope(|scope| {
            let (send, receive) = std::sync::mpsc::channel();
            let read = reopened.clone();
            let query = params.clone();
            scope.spawn(move || {
                send.send(read.summary(query).unwrap()).unwrap();
            });
            let answer = receive.recv_timeout(std::time::Duration::from_secs(2));
            drop(gate);
            let answer =
                answer.expect("The previous summary must not wait for the catch-up worker");
            assert_eq!(answer["data"]["state"], "empty");
            assert!(answer["data"]["asOf"].is_u64());
            assert_eq!(answer["data"]["totals"], first["data"]["totals"]);
        });
        // A different signed setup never gets the former scope's saved answer.
        let mut new_scope = reopened.scope();
        new_scope.setup = Some("different-setup".into());
        reopened.peer_roster(new_scope).unwrap();
        let loading = reopened.summary(params.clone()).unwrap();
        assert_eq!(loading["data"]["state"], "loading");
        assert!(loading["data"].get("asOf").is_none());
    }
}
