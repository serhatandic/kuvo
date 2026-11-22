//! Synthetic Insights answers used by desktop tests and the shot runner: the summary a box makes
//! (`insights/summary`), and the older overview an older box answers. A month of sessions under six
//! bots and on their own, consistent at every level (the #930 drafts' figures).
use serde_json::{Value, json};

const ROUTE: &str = "kuvo/agents/api/kuvo/insights";

struct Bot {
    id: &'static str,
    name: &'static str,
    color: &'static str,
    shape: &'static str,
}

const BOTS: [Bot; 6] = [
    Bot { id: "bot_foreman", name: "Foreman", color: "indigo", shape: "squircle" },
    Bot { id: "bot_sentinel", name: "Sentinel", color: "mint", shape: "dome" },
    Bot { id: "bot_scout", name: "Scout", color: "sky", shape: "pebble" },
    Bot { id: "bot_builder", name: "Builder", color: "amber", shape: "gem" },
    Bot { id: "bot_triage", name: "Triage", color: "coral", shape: "round" },
    Bot { id: "bot_scribe", name: "Scribe", color: "violet", shape: "bloom" },
];

const PROVIDERS: [(&str, &str); 4] = [("claude", "Claude Code"), ("codex", "Codex"), ("cursor", "Cursor"), ("grok", "Grok")];
// Key, name, provider.
const MODELS: [(&str, &str, &str); 8] = [
    ("opus-5.5", "Opus 5.5", "claude"), ("gpt-5.5", "GPT-5.5", "codex"), ("sonnet-5", "Sonnet 5", "claude"),
    ("gpt-5.5-codex", "GPT-5.5 Codex", "codex"), ("haiku-4.5", "Haiku 4.5", "claude"), ("composer-2", "Composer 2", "cursor"),
    ("grok-4.4", "Grok 4.4", "grok"), ("gpt-5.5-mini", "GPT-5.5 Mini", "codex"),
];
const COMPUTERS: [(&str, &str); 4] = [("key_stand_in", "Stand-in Mac"), ("key_studio", "Mac Studio"), ("key_devbox", "devbox"), ("key_mini", "Build Mini")];

/// One top-level conversation: its bot (and whether it's the bot's own chat), subagents, provider,
/// model and computer (by index), the day it peaked (29 is today) and over how many days, its
/// cost, millions of tokens and cache rate, and why it stands out where it does.
struct Session {
    id: &'static str,
    title: &'static str,
    bot: Option<usize>,
    own: bool,
    subagents: u32,
    provider: usize,
    model: usize,
    computer: usize,
    day: usize,
    span: usize,
    cost: f64,
    millions: f64,
    cache: f64,
    standout: Option<&'static str>,
}

macro_rules! sessions {
    ($( ($id:literal, $title:literal, $bot:expr, $own:literal, $subagents:literal, $provider:literal, $model:literal, $computer:literal, $day:literal, $span:literal, $cost:literal, $millions:literal, $cache:literal, $standout:expr) ),* $(,)?) => {
        &[$(Session { id: $id, title: $title, bot: $bot, own: $own, subagents: $subagents, provider: $provider, model: $model, computer: $computer, day: $day, span: $span, cost: $cost, millions: $millions, cache: $cache, standout: $standout }),*]
    };
}

const FULL: &[Session] = sessions![
    ("ses_foreman", "Foreman", Some(0), true, 0, 0, 2, 1, 28, 26, 21.40, 6.2, 0.91, None),
    ("ses_f01", "Release pipeline fix", Some(0), false, 4, 0, 0, 1, 27, 4, 68.40, 15.8, 0.89, Some("Highest cost")),
    ("ses_f02", "Auth rewrite loop", Some(0), false, 3, 1, 1, 0, 24, 3, 52.80, 11.4, 0.61, Some("Low cache")),
    ("ses_f03", "Repo index sweep", Some(0), false, 9, 0, 0, 1, 21, 3, 44.60, 12.1, 0.84, Some("9 subagents")),
    ("ses_f04", "Relay lease failover", Some(0), false, 3, 1, 1, 2, 18, 3, 31.20, 7.6, 0.86, None),
    ("ses_f05", "Fleet update gates", Some(0), false, 2, 0, 2, 1, 15, 2, 24.90, 6.9, 0.88, None),
    ("ses_f06", "Mac installer signing", Some(0), false, 2, 0, 2, 0, 12, 2, 19.30, 5.4, 0.85, None),
    ("ses_f07", "Queue status sync", Some(0), false, 1, 1, 3, 2, 10, 2, 16.10, 4.7, 0.83, None),
    ("ses_f08", "Peer mail backoff", Some(0), false, 2, 0, 2, 1, 8, 2, 14.20, 4.1, 0.87, None),
    ("ses_f09", "Schema v4 migration", Some(0), false, 1, 1, 1, 0, 5, 2, 12.60, 3.3, 0.79, None),
    ("ses_f10", "Warm standby removal", Some(0), false, 2, 0, 0, 1, 3, 2, 11.80, 2.8, 0.82, None),
    ("ses_f11", "Crash log collector", Some(0), false, 1, 0, 4, 3, 1, 2, 6.40, 2.5, 0.90, None),
    ("ses_sentinel", "Sentinel", Some(1), true, 0, 0, 4, 2, 29, 28, 12.80, 5.1, 0.92, None),
    ("ses_s01", "Security audit pass", Some(1), false, 5, 0, 0, 2, 26, 3, 41.20, 9.8, 0.86, None),
    ("ses_s02", "TLS cert rotation", Some(1), false, 2, 1, 1, 2, 22, 2, 29.50, 7.1, 0.81, None),
    ("ses_s03", "Vault permission check", Some(1), false, 3, 0, 2, 1, 19, 3, 24.40, 6.8, 0.88, None),
    ("ses_s04", "Sandbox escape tests", Some(1), false, 4, 1, 3, 2, 14, 2, 22.10, 6.2, 0.77, None),
    ("ses_s05", "Device key revoke", Some(1), false, 1, 0, 2, 0, 11, 2, 17.80, 4.9, 0.89, None),
    ("ses_s06", "OAuth token expiry", Some(1), false, 2, 3, 6, 2, 7, 2, 15.60, 4.3, 0.74, None),
    ("ses_s07", "Dependency CVE sweep", Some(1), false, 3, 0, 4, 3, 4, 2, 11.40, 4.6, 0.91, None),
    ("ses_s08", "Keychain lock retry", Some(1), false, 1, 1, 7, 0, 2, 2, 8.90, 3.2, 0.84, None),
    ("ses_scout", "Scout", Some(2), true, 0, 1, 7, 0, 28, 25, 8.60, 3.9, 0.90, None),
    ("ses_c01", "GPUI scroll profile", Some(2), false, 3, 0, 2, 0, 25, 3, 28.40, 7.9, 0.85, None),
    ("ses_c02", "Pricing catalog diff", Some(2), false, 2, 2, 5, 0, 20, 2, 21.70, 6.1, 0.78, None),
    ("ses_c03", "Model alias cleanup", Some(2), false, 2, 1, 3, 1, 16, 2, 18.90, 5.3, 0.83, None),
    ("ses_c04", "Window focus trace", Some(2), false, 1, 0, 2, 0, 13, 2, 16.20, 4.4, 0.87, None),
    ("ses_c05", "Wake timer drift", Some(2), false, 2, 3, 6, 2, 9, 2, 13.80, 3.7, 0.72, None),
    ("ses_c06", "Dark contrast check", Some(2), false, 1, 2, 5, 0, 6, 2, 11.20, 3.4, 0.80, None),
    ("ses_c07", "Font metrics probe", Some(2), false, 1, 0, 4, 0, 1, 2, 7.40, 2.9, 0.89, None),
    ("ses_builder", "Builder", Some(3), true, 0, 1, 3, 3, 27, 22, 6.90, 2.8, 0.88, None),
    ("ses_b01", "Arm64 release bundle", Some(3), false, 3, 1, 1, 3, 23, 2, 21.60, 5.5, 0.84, None),
    ("ses_b02", "Sparkle feed builder", Some(3), false, 2, 0, 2, 3, 18, 2, 16.40, 4.6, 0.87, None),
    ("ses_b03", "Cargo cache prune", Some(3), false, 1, 1, 3, 2, 15, 2, 13.50, 3.9, 0.82, None),
    ("ses_b04", "DMG notarization", Some(3), false, 2, 0, 2, 3, 11, 2, 11.90, 3.4, 0.86, None),
    ("ses_b05", "Shot runner speedups", Some(3), false, 1, 2, 5, 0, 6, 2, 9.80, 2.9, 0.76, None),
    ("ses_b06", "Bun worker bundle", Some(3), false, 1, 1, 7, 2, 2, 2, 6.80, 2.6, 0.85, None),
    ("ses_triage", "Triage", Some(4), true, 0, 0, 4, 1, 29, 24, 4.10, 1.9, 0.91, None),
    ("ses_t01", "Linear sync intake", Some(4), false, 2, 0, 2, 1, 25, 2, 11.20, 3.2, 0.87, None),
    ("ses_t02", "Flaky test bisect", Some(4), false, 2, 1, 3, 2, 19, 2, 9.40, 2.7, 0.79, None),
    ("ses_t03", "Duplicate issue merge", Some(4), false, 1, 3, 6, 1, 13, 2, 7.10, 2.1, 0.71, None),
    ("ses_t04", "User report repro", Some(4), false, 1, 2, 5, 0, 8, 2, 5.90, 1.8, 0.76, None),
    ("ses_t05", "Alert noise filter", Some(4), false, 1, 0, 4, 3, 3, 2, 3.80, 1.6, 0.90, None),
    ("ses_scribe", "Scribe", Some(5), true, 0, 0, 4, 0, 26, 20, 2.90, 1.3, 0.92, None),
    ("ses_r01", "Changelog draft", Some(5), false, 1, 0, 2, 0, 22, 2, 9.60, 2.8, 0.88, None),
    ("ses_r02", "API docs refresh", Some(5), false, 1, 1, 7, 0, 14, 2, 6.40, 2.3, 0.84, None),
    ("ses_r03", "Release notes edit", Some(5), false, 0, 3, 6, 0, 5, 2, 4.80, 1.5, 0.73, None),
    ("ses_n01", "SQLite worker audit", None, false, 0, 0, 0, 0, 28, 3, 46.80, 11.2, 0.85, None),
    ("ses_n02", "Composer paste bugs", None, false, 0, 0, 2, 0, 23, 2, 27.40, 7.6, 0.87, None),
    ("ses_n03", "Sidebar resize feel", None, false, 0, 2, 5, 0, 20, 2, 19.20, 5.4, 0.79, None),
    ("ses_n04", "Codex rate fallback", None, false, 0, 1, 1, 1, 17, 2, 16.80, 4.3, 0.82, None),
    ("ses_n05", "Quick search ranking", None, false, 0, 0, 2, 0, 14, 2, 13.40, 3.8, 0.86, None),
    ("ses_n06", "Diff fold animation", None, false, 0, 2, 5, 0, 11, 2, 10.60, 3.1, 0.77, None),
    ("ses_n07", "Linux clipboard fix", None, false, 0, 1, 3, 2, 8, 2, 8.50, 2.5, 0.81, None),
    ("ses_n08", "Grok stream retry", None, false, 0, 3, 6, 2, 6, 2, 6.90, 2.0, 0.69, None),
    ("ses_n09", "Icon stroke audit", None, false, 0, 0, 4, 0, 3, 1, 4.60, 1.8, 0.90, None),
    ("ses_n10", "Keymap doc check", None, false, 0, 1, 7, 0, 1, 1, 3.10, 1.2, 0.86, None),
];

/// A first week: two small sessions, nothing to compare with.
const LITTLE: &[Session] = sessions![
    ("ses_first", "First repo setup", Some(0), false, 0, 0, 2, 0, 29, 1, 3.24, 0.86, 0.76, None),
    ("ses_lint", "Fix lint warning", None, false, 0, 0, 2, 0, 28, 1, 1.58, 0.41, 0.68, None),
];

const SUBAGENTS: [&str; 9] = ["Explore the codebase", "Run the tests", "Check the docs", "Review the change", "Trace the failure",
    "Search the history", "Read the logs", "Draft the fix", "Verify the build"];

/// What the stand-in has to say: a month of work, a first week, nothing at all, or that it
/// couldn't read its usage; that it's still reading it; or a month of work as it last read it, a
/// minute ago, while it reads it afresh (`asOf`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sample {
    Full,
    Little,
    Nothing,
    Failing,
    Reading,
    Kept,
}

#[derive(Clone, Copy, Default)]
struct Sums {
    cost: f64,
    tokens: f64,
    cached: f64,
    input: f64,
}

impl Sums {
    fn of(session: &Session, scale: f64) -> Self {
        let tokens = session.millions * 1e6 * scale;
        // (About nine tokens in ten are input.)
        let input = tokens * 0.91;
        Self { cost: session.cost * scale, tokens, cached: input * session.cache, input }
    }

    fn add(&mut self, other: Sums) {
        self.cost += other.cost;
        self.tokens += other.tokens;
        self.cached += other.cached;
        self.input += other.input;
    }

    fn rate(self) -> Option<f64> {
        (self.input > 0.).then(|| self.cached / self.input)
    }

    fn json(self, overall: Option<f64>) -> Value {
        let rate = self.rate();
        let mut value = json!({ "cost": (self.cost * 100.).round() / 100., "tokens": self.tokens.round(), "cacheRate": rate });
        if let (Some(rate), Some(overall)) = (rate, overall) && rate <= overall - 0.15 {
            value["lowCache"] = json!(true);
        }
        value
    }
}

fn row(kind: &str, id: &str, name: &str, sums: Sums, overall: Option<f64>, extra: Value) -> Value {
    let mut value = sums.json(overall);
    value["kind"] = json!(kind);
    value["id"] = json!(id);
    value["name"] = json!(name);
    if let Value::Object(extra) = extra {
        for (key, field) in extra {
            value[key] = field;
        }
    }
    value
}

fn ranked(mut rows: Vec<(Sums, Value)>) -> Vec<Value> {
    rows.sort_by(|a, b| b.0.cost.total_cmp(&a.0.cost));
    rows.into_iter().map(|(_, value)| value).collect()
}

fn plural(n: u32, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

fn look(bot: &Bot) -> Value {
    json!({ "color": bot.color, "shape": bot.shape, "face": "calm" })
}

fn session_note(session: &Session, alone: bool) -> Value {
    if session.own { json!({ "note": "Own chat" }) }
    else if session.subagents > 0 { json!({ "note": plural(session.subagents, "subagent") }) }
    else if alone { json!({ "note": "No bot" }) }
    else { json!({}) }
}

/// Its answer to `method route`, or None for a route that isn't Insights'.
pub fn answer(method: &str, route: &str, query: impl Fn(&str) -> Option<String>, now: f64, sample: Sample) -> Option<(u16, Value)> {
    let tail = route.strip_prefix(ROUTE)?;
    if (method, tail) == ("POST", "/refresh") { return Some((202, json!({"data": {"scanning": true}}))); }
    if method != "GET" { return None; }
    if sample == Sample::Failing { return Some((500, json!({ "message": "Insights couldn\u{2019}t read its usage." }))); }
    if sample == Sample::Reading {
        return Some((200, json!({ "data": { "state": "loading", "totals": { "cost": 0, "tokens": 0, "cacheRate": null }, "change": null, "trend": [], "lists": [], "standouts": [] } })));
    }
    if sample == Sample::Kept {
        let (status, mut value) = answer(method, route, query, now, Sample::Full)?;
        value["data"]["asOf"] = json!(now - 60_000.);
        return Some((status, value));
    }
    let days = query("period").and_then(|days| days.parse::<usize>().ok()).unwrap_or(30);
    let sessions = match sample { Sample::Full => FULL, Sample::Little => LITTLE, Sample::Nothing | Sample::Failing | Sample::Reading | Sample::Kept => &[] };
    // (A week is the month's last seven days; a quarter, this month's work and two more like it.)
    let (scale, first) = match days { 7 => (1., 23), 90 => (2.75, 0), _ => (1., 0) };
    let narrowed = |name: &str| query(name).filter(|value| !value.is_empty());
    let (bot, session, provider, model, computer, account) =
        (narrowed("bot"), narrowed("session"), narrowed("provider"), narrowed("model"), narrowed("computer"), narrowed("account"));
    let account_provider = account.as_deref().map(|id| id.split(['#', '@', '/']).next().unwrap_or(id).trim_end_matches("-code").trim_end_matches("-cli").to_owned());
    let matching: Vec<&Session> = sessions.iter().filter(|each| each.day >= first
        && bot.as_deref().is_none_or(|id| each.bot.is_some_and(|i| BOTS[i].id == id))
        && session.as_deref().is_none_or(|id| each.id == id)
        && provider.as_deref().or(account_provider.as_deref()).is_none_or(|id| PROVIDERS[each.provider].0 == id)
        && model.as_deref().is_none_or(|id| MODELS[each.model].0 == id)
        && computer.as_deref().is_none_or(|id| COMPUTERS[each.computer].0 == id)).collect();
    let mut totals = Sums::default();
    let points = if days == 7 { 7 } else { days };
    let mut trend = vec![Sums::default(); points];
    for each in &matching {
        let sums = Sums::of(each, scale);
        totals.add(sums);
        // Spread over its days, rounder in the middle.
        let at = if days == 7 { each.day - first } else if days == 90 { 60 + each.day } else { each.day };
        let span = each.span.max(1).min(at + 1);
        let weights: Vec<f64> = (0..span).map(|i| 0.55 + 0.9 * (((i as f64 + 0.5) / span as f64) * std::f64::consts::PI).sin()).collect();
        let whole: f64 = weights.iter().sum();
        for (i, weight) in weights.into_iter().enumerate() {
            let day = &mut trend[at + 1 - span + i];
            day.cost += sums.cost * weight / whole;
            day.tokens += sums.tokens * weight / whole;
        }
        if days == 90 {
            // (The two months before: the same work, a little less of it.)
            for back in [30, 60] {
                let day = &mut trend[at - back];
                day.cost += sums.cost * 0.875 / 2.75;
                day.tokens += sums.tokens * 0.875 / 2.75;
            }
        }
    }
    let overall = totals.rate();
    let trend: Vec<Value> = trend.iter().enumerate().map(|(i, day)| json!({
        "day": crate::time::day(now, 0, i as i64 - (points as i64 - 1)), "cost": (day.cost * 100.).round() / 100., "tokens": day.tokens.round(),
    })).collect();
    let state = if matching.is_empty() { "empty" } else { "ready" };
    if tail.is_empty() {
        // An older box's overview: totals, the days and providers; no bots, cache or change.
        let providers: Vec<Value> = PROVIDERS.iter().enumerate().filter_map(|(i, (id, _))| {
            let sums = matching.iter().filter(|each| each.provider == i).fold(Sums::default(), |mut all, each| { all.add(Sums::of(each, scale)); all });
            (sums.tokens > 0.).then(|| json!({ "name": id, "provider": id, "cost": sums.cost, "processed": sums.tokens, "tokens": sums.tokens * 0.2, "calls": 1, "messages": 0, "unpriced": 0 }))
        }).collect();
        let daily: Vec<Value> = trend.iter().map(|day| json!({ "name": day["day"], "cost": day["cost"], "processed": day["tokens"], "calls": 1, "messages": 0, "unpriced": 0 })).collect();
        return Some((200, json!({ "data": {
            "state": state, "scanning": false, "totals": { "cost": totals.cost, "processed": totals.tokens, "tokens": totals.tokens * 0.2, "calls": matching.len(), "messages": 0, "unpriced": 0 },
            "daily": daily, "providers": providers, "breakdowns": { "model": [], "agent": [], "day": [], "skill": [] },
            "computers": [], "bots": [], "coverage": [], "unavailable": [],
        } })));
    }
    if tail != "/summary" { return None; }

    let conversation = |each: &Session, alone: bool| row("session", each.id, each.title, Sums::of(each, scale), overall, session_note(each, alone));
    let first_list = if let Some(id) = session.as_deref() {
        // The session and its subagents, each on its own.
        let rows = matching.iter().filter(|each| each.id == id).flat_map(|each| {
            let whole = Sums::of(each, scale);
            let share = |part: f64| Sums { cost: whole.cost * part, tokens: whole.tokens * part, cached: whole.cached * part, input: whole.input * part };
            let main = if each.subagents == 0 { 1. } else { 0.4 };
            let mut rows = vec![(share(main), row("session", each.id, each.title, share(main), overall, json!({})))];
            for (i, name) in SUBAGENTS.iter().take(each.subagents as usize).enumerate() {
                let part = 0.6 * (each.subagents as usize - i) as f64 / (1..=each.subagents as usize).sum::<usize>() as f64;
                rows.push((share(part), row("session", &format!("{}_sub{i}", each.id), name, share(part), overall, json!({}))));
            }
            rows
        }).collect();
        json!({ "key": "sessions", "name": "Sessions", "rows": ranked(rows) })
    } else if bot.is_some() {
        json!({ "key": "sessions", "name": "Sessions", "rows": ranked(matching.iter().map(|each| (Sums::of(each, scale), conversation(each, false))).collect()) })
    } else {
        let mut rows: Vec<(Sums, Value)> = BOTS.iter().enumerate().filter_map(|(i, bot)| {
            let mine: Vec<&&Session> = matching.iter().filter(|each| each.bot == Some(i)).collect();
            if mine.is_empty() { return None; }
            let sums = mine.iter().fold(Sums::default(), |mut all, each| { all.add(Sums::of(each, scale)); all });
            let overseen = mine.iter().filter(|each| !each.own).count() as u32;
            let children = ranked(mine.iter().map(|each| (Sums::of(each, scale), conversation(each, false))).collect());
            Some((sums, row("bot", bot.id, bot.name, sums, overall, json!({ "look": look(bot), "note": (overseen > 0).then(|| plural(overseen, "session")), "sessions": children }))))
        }).collect();
        rows.extend(matching.iter().filter(|each| each.bot.is_none()).map(|each| (Sums::of(each, scale), conversation(each, true))));
        json!({ "key": "bots", "name": "Bots", "rows": ranked(rows) })
    };
    let grouped = |kind: &str, of: &dyn Fn(&Session) -> usize, named: &dyn Fn(usize) -> (String, String, Value)| {
        let mut sums: Vec<(usize, Sums)> = Vec::new();
        for each in &matching {
            let key = of(*each);
            match sums.iter_mut().find(|(k, _)| *k == key) {
                Some((_, total)) => total.add(Sums::of(each, scale)),
                None => sums.push((key, Sums::of(each, scale))),
            }
        }
        ranked(sums.into_iter().map(|(key, total)| {
            let (id, name, extra) = named(key);
            (total, row(kind, &id, &name, total, overall, extra))
        }).collect())
    };
    let providers = grouped("provider", &|each| each.provider, &|i| (PROVIDERS[i].0.into(), PROVIDERS[i].1.into(), json!({ "provider": PROVIDERS[i].0 })));
    let models = grouped("model", &|each| each.model, &|i| (MODELS[i].0.into(), MODELS[i].1.into(), json!({ "provider": MODELS[i].2 })));
    let computers = grouped("computer", &|each| each.computer, &|i| (COMPUTERS[i].0.into(), COMPUTERS[i].1.into(), json!({})));

    // Against the month before: a little less of the same, its cache a little worse.
    let change = (sample == Sample::Full && session.is_none() && !matching.is_empty()).then(|| {
        let (cost, tokens, cache) = match (bot.as_deref(), provider.as_deref()) {
            (Some("bot_foreman"), _) => (0.84, 0.81, 0.032),
            (Some(_), _) => (1.07, 1.04, 0.014),
            (_, Some("claude")) => (0.88, 0.86, 0.021),
            (_, Some(_)) => (0.94, 0.92, 0.015),
            _ if model.is_some() => (0.91, 0.89, 0.018),
            _ if computer.is_some() => (0.95, 0.93, 0.011),
            _ => (0.90, 0.87, 0.028),
        };
        json!({ "cost": 1. / cost - 1., "tokens": 1. / tokens - 1., "cacheRate": cache })
    });

    // The tagged sessions, then the one that cost most, once there are enough to stand out among.
    let candidates: Vec<&&Session> = matching.iter().filter(|each| !each.own).collect();
    let mut standouts: Vec<Value> = Vec::new();
    if candidates.len() >= 4 {
        for each in candidates.iter().filter(|each| each.standout.is_some()) {
            let reason = each.standout.unwrap_or_default();
            let kind = if reason == "Low cache" { "cache" } else if reason.ends_with("subagents") { "subagents" } else { "cost" };
            let mut value = Sums::of(each, scale).json(overall);
            value["id"] = json!(each.id);
            value["name"] = json!(each.title);
            value["reason"] = json!(reason);
            value["kind"] = json!(kind);
            standouts.push(value);
        }
        if standouts.is_empty() && let Some(top) = candidates.iter().max_by(|a, b| a.cost.total_cmp(&b.cost)) {
            let mut value = Sums::of(top, scale).json(overall);
            value["id"] = json!(top.id);
            value["name"] = json!(top.title);
            value["reason"] = json!("Highest cost");
            value["kind"] = json!("cost");
            standouts.push(value);
        }
    }
    let lists = vec![
        first_list,
        json!({ "key": "providers", "name": "Providers", "rows": providers }),
        json!({ "key": "models", "name": "Models", "rows": models }),
        json!({ "key": "computers", "name": "Computers", "rows": computers }),
    ];
    // (When it last counted: five minutes ago, as its regular count would have; just now when asked
    // to count again, `refresh=1`. It keeps no count time of its own between answers.)
    let counted = if query("refresh").as_deref() == Some("1") { now } else { now - 5. * 60_000. };
    Some((200, json!({ "data": {
        "state": state, "period": days, "zone": query("zone"), "countedAt": counted, "totals": totals.json(None), "change": change, "trend": trend,
        "lists": lists, "standouts": standouts,
    } })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insights_sample_reconciles_its_lists_and_trend_with_its_totals() {
        let now = 1_794_960_000_000.;
        let (_, summary) = answer("GET", &format!("{ROUTE}/summary"), |name| (name == "period").then(|| "30".to_owned()), now, Sample::Full).unwrap();
        let data = &summary["data"];
        let total = data["totals"]["cost"].as_f64().unwrap();
        for list in data["lists"].as_array().unwrap() {
            let cost: f64 = list["rows"].as_array().unwrap().iter().map(|row| row["cost"].as_f64().unwrap()).sum();
            assert!((cost - total).abs() < 0.05, "{}: {cost} of {total}", list["key"]);
        }
        let trend: f64 = data["trend"].as_array().unwrap().iter().map(|day| day["cost"].as_f64().unwrap()).sum();
        assert!((trend - total).abs() < 0.2);
        assert_eq!(data["standouts"].as_array().unwrap().len(), 3);
    }
}
