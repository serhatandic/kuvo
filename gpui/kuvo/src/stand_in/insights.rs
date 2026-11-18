//! Synthetic Insights responses used by desktop tests and the shot runner.
use serde_json::{Value, json};

const ROUTE: &str = "kuvo/agents/api/kuvo/insights";
const MODELS: [(&str, &str); 3] = [("gpt-5.5", "codex"), ("claude-sonnet-5", "claude"), ("grok-4.4", "grok")];
const SESSIONS: [(&str, &str, &str); 3] = [
    ("ses_signup", "Fix the sign-up form", "Mira"),
    ("ses_palette", "Adjust the menu palette", "Build"),
    ("ses_notes", "Write project notes", "Notes"),
];
// Days back, session, model, tokens without cache reads, skill.
const ACTIVITY: [(i64, usize, usize, u32, &str); 9] = [
    (11, 1, 1, 45_000, "review"),
    (11, 2, 2, 15_000, "writing"),
    (8, 1, 1, 30_000, "review"),
    (6, 0, 0, 20_000, "research"),
    (6, 0, 1, 40_000, "review"),
    (4, 0, 0, 50_000, "research"),
    (4, 2, 2, 35_000, "writing"),
    (2, 1, 1, 15_000, "review"),
    (1, 0, 0, 70_000, "research"),
];

#[derive(Clone, Copy)]
struct Call {
    back: i64,
    session: usize,
    model: usize,
    tokens: u32,
    skill: &'static str,
    computer: &'static str,
}

fn cost(tokens: u32) -> f64 {
    f64::from(tokens) * 0.000002
}

fn measures(calls: &[&Call]) -> Value {
    let tokens: u32 = calls.iter().map(|call| call.tokens).sum();
    json!({
        "cost": cost(tokens), "tokens": tokens, "cached": tokens * 29 / 10,
        "input": tokens * 74 / 100, "creation": tokens * 18 / 100, "output": tokens * 8 / 100,
        "processed": tokens * 39 / 10, "messages": calls.len(), "unpriced": 0, "calls": calls.len()
    })
}

pub fn answer(method: &str, route: &str, query: impl Fn(&str) -> Option<String>, now: f64, empty: bool, partial: bool) -> Option<(u16, Value)> {
    let tail = route.strip_prefix(ROUTE)?;
    if (method, tail) == ("POST", "/refresh") { return Some((202, json!({"data": {"scanning": true}}))); }
    if method != "GET" { return None; }

    let days = query("period").and_then(|days| days.parse::<i64>().ok()).unwrap_or(30).clamp(7, 365);
    let computer = query("computer");
    let provider = query("provider").or_else(|| query("account").and_then(|id| {
        id.split_once('#').map(|(name, _)| name.to_owned())
            .or_else(|| id.rsplit('/').next().map(|name| name.split('@').next().unwrap_or(name).trim_end_matches("-code").trim_end_matches("-cli").to_owned()))
    }));
    let bot = query("bot");
    let activity: Vec<_> = ACTIVITY.into_iter().map(|(back, session, model, tokens, skill)| Call {
        back, session, model, tokens, skill, computer: if session == 1 { "key_devbox" } else { "key_here" },
    }).filter(|call| !empty && call.back < days
        && computer.as_deref().is_none_or(|id| id == call.computer)
        && provider.as_deref().is_none_or(|id| id == MODELS[call.model].1)
        && bot.as_deref().is_none_or(|id| id == "bot_mira" && call.session == 0)).collect();
    let activity: Vec<_> = activity.iter().collect();
    let at = |call: &Call| now - call.back as f64 * 86_400_000.;
    let total: u32 = activity.iter().map(|call| call.tokens).sum();
    let row = |name: &str, provider: Option<&str>, calls: Vec<&Call>| {
        let mut row = measures(&calls);
        let share = (total != 0).then(|| row["tokens"].as_f64().unwrap_or_default() / f64::from(total));
        row["name"] = json!(name);
        row["provider"] = json!(provider);
        row["costShare"] = json!(share);
        row["tokenShare"] = json!(share);
        row
    };
    let by_session = |index| activity.iter().copied().filter(|call| call.session == index).collect::<Vec<_>>();
    let data = match tail {
        "/sessions" => {
            let mut sessions: Vec<_> = SESSIONS.iter().enumerate().filter_map(|(i, (session, title, _))| {
                let calls = by_session(i);
                let latest = *calls.last()?;
                let mut out = row(title, None, calls);
                out["session"] = json!(session);
                out["title"] = json!(title);
                out["computer"] = json!(latest.computer);
                out["at"] = json!(at(latest));
                Some(out)
            }).collect();
            sessions.sort_by(|a, b| b["at"].as_f64().unwrap_or_default().total_cmp(&a["at"].as_f64().unwrap_or_default()));
            json!({ "sessions": sessions, "next": null, "localComputers": ["key_here"] })
        }
        tail if tail.starts_with("/sessions/") => {
            let calls: Vec<_> = SESSIONS.iter().position(|(id, _, _)| Some(*id) == tail.strip_prefix("/sessions/"))
                .map(by_session).unwrap_or_default().into_iter().map(|call| json!({
                    "at": at(call), "computer": call.computer, "model": MODELS[call.model].0,
                    "provider": MODELS[call.model].1, "tokenCount": call.tokens, "cost": cost(call.tokens),
                })).collect();
            json!({ "calls": calls, "next": null })
        }
        "" => {
            let daily: Vec<_> = (0..days).rev().map(|back| row(&crate::time::day(now, 0, -back), None,
                activity.iter().copied().filter(|call| call.back == back).collect())).collect();
            let model: Vec<_> = MODELS.iter().enumerate().filter_map(|(i, (model, provider))| {
                let calls: Vec<_> = activity.iter().copied().filter(|call| call.model == i).collect();
                (!calls.is_empty()).then(|| row(model, Some(provider), calls))
            }).collect();
            let agent: Vec<_> = SESSIONS.iter().enumerate().filter_map(|(i, (_, _, name))| {
                let calls = by_session(i);
                let provider = MODELS[calls.last()?.model].1;
                Some(row(name, Some(provider), calls))
            }).collect();
            let skill: Vec<_> = ["research", "review", "writing"].into_iter().filter_map(|skill| {
                let calls: Vec<_> = activity.iter().copied().filter(|call| call.skill == skill).collect();
                (!calls.is_empty()).then(|| row(skill, None, calls))
            }).collect();
            let mut totals = measures(&activity);
            totals["sessions"] = json!(agent.len());
            json!({ "totals": totals, "daily": daily,
                "breakdowns": { "model": model, "agent": agent, "day": daily, "skill": skill },
                "coverage": [{"name": "This Mac", "scanned": now - 60_000., "stale": false, "issues": if partial { vec!["antigravity"] } else { vec![] } }],
                "unavailable": if partial { vec!["Cursor API call counts where the run only reports context size"] } else { vec![] },
                "computers": [{"key": "key_here", "name": "This Mac"}, {"key": "key_devbox", "name": "devbox"}],
                "bots": [{"key": "bot_mira", "name": "Mira"}], "scanning": false,
                "state": if empty || activity.is_empty() { "empty" } else if partial { "partial" } else { "ready" }
            })
        }
        _ => return None,
    };
    Some((200, json!({ "data": data })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insights_sample_reconciles_daily_and_session_totals() {
        let now = 1_794_960_000_000.;
        for computer in [None, Some("key_devbox")] {
            let query = |name: &str| (name == "computer").then_some(computer).flatten().map(str::to_owned);
            let (_, overview) = answer("GET", ROUTE, query, now, false, false).unwrap();
            let (_, sessions) = answer("GET", &format!("{ROUTE}/sessions"), query, now, false, false).unwrap();
            let totals = &overview["data"]["totals"];
            for rows in [&overview["data"]["daily"], &sessions["data"]["sessions"]] {
                let rows = rows.as_array().unwrap();
                assert_eq!(rows.iter().map(|row| row["tokens"].as_u64().unwrap()).sum::<u64>(), totals["tokens"].as_u64().unwrap());
                let cost: f64 = rows.iter().map(|row| row["cost"].as_f64().unwrap()).sum();
                assert!((cost - totals["cost"].as_f64().unwrap()).abs() < 1e-10);
            }
        }
    }
}
