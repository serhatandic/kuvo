//! Synthetic Insights responses used by desktop tests and the shot runner.
use serde_json::{Value, json};

const ROUTE: &str = "kuvo/agents/api/kuvo/insights";

pub fn answer(method: &str, route: &str, query: impl Fn(&str) -> Option<String>, now: f64, empty: bool, partial: bool) -> Option<(u16, Value)> {
    let tail = route.strip_prefix(ROUTE)?;
    if (method, tail) == ("POST", "/refresh") {
        return Some((202, json!({"data": {"scanning": true}})));
    }
    if method != "GET" { return None; }
    let scoped = query("account").is_some() || query("provider").is_some() || query("computer").is_some();
    let factor: f64 = if scoped { 0.45 } else { 1.0 };
    let measures = |tokens: f64, calls: u64| json!({
        "cost": tokens * 0.000002, "tokens": tokens, "cached": tokens * 2.9,
        "input": tokens * 0.74, "creation": tokens * 0.18, "output": tokens * 0.08,
        "processed": tokens * 3.9, "messages": calls / 3, "unpriced": if partial { 2 } else { 0 }, "calls": calls
    });
    let row = |name: &str, provider: Option<&str>, amount: f64, share: f64, calls: u64| {
        let mut row = measures(amount * factor, calls);
        row["name"] = json!(name);
        row["provider"] = json!(provider);
        row["costShare"] = json!(share);
        row["tokenShare"] = json!(share);
        row
    };
    let examples = [("ses_signup", "Fix the sign-up form"), ("ses_palette", "Adjust the menu palette"), ("ses_notes", "Write project notes")];
    let data = match tail {
        "/sessions" => {
            let sessions: Vec<Value> = if empty { Vec::new() } else { examples.iter().enumerate().map(|(i, (session, title))| {
                let mut out = measures(54_000. * factor / (i + 1) as f64, 12);
                out["session"] = json!(session);
                out["title"] = json!(title);
                out["computer"] = json!("key_here");
                out["at"] = json!(now - 4_000_000. * (i + 1) as f64);
                out
            }).collect() };
            json!({ "sessions": sessions, "next": null, "localComputers": ["key_here"] })
        }
        tail if tail.starts_with("/sessions/") => {
            let calls: Vec<Value> = (0..7).rev().map(|i| json!({
                "at": now - 300_000. * (i + 1) as f64, "computer": "key_here",
                "model": if i % 2 == 0 { "gpt-5.5" } else { "claude-sonnet-5" },
                "provider": if i % 2 == 0 { "codex" } else { "claude" },
                "tokenCount": 11_250 - i * 130,
                "cost": 0.016 + 0.003 * i as f64
            })).collect();
            json!({ "calls": calls, "next": null })
        }
        "" => {
            let days = query("period").and_then(|days| days.parse::<i64>().ok()).unwrap_or(30).clamp(7, 365);
            let daily: Vec<Value> = (0..days).map(|i| {
                let amount = if empty || i % 7 == 0 { 0. } else { 1_500. + ((i * 89) % 37) as f64 * 480. };
                row(&crate::time::day(now, 0, i - days + 1), None, amount, amount / 320_000., if amount > 0. { 5 } else { 0 })
            }).collect();
            let mut totals = measures(if empty { 0. } else { 320_000. * factor }, if empty { 0 } else { 78 });
            totals["sessions"] = json!(if empty { 0 } else { 3 });
            let model = if empty { vec![] } else { vec![
                row("gpt-5.5", Some("codex"), 210_000., 0.66, 45),
                row("claude-sonnet-5", Some("claude"), 87_000., 0.27, 29),
                row("grok-4.4", Some("grok"), 23_000., 0.07, 4),
            ] };
            let providers = if empty { vec![] } else { vec![row("codex", None, 210_000., 0.66, 45), row("claude", None, 87_000., 0.27, 29), row("grok", None, 23_000., 0.07, 4)] };
            let agent = if empty { vec![] } else { vec![row("Mira", Some("codex"), 115_000., 0.36, 21), row("Build", Some("claude"), 205_000., 0.64, 57)] };
            let skill = if empty { vec![] } else { vec![row("research", None, 122_000., 0.38, 19), row("review", None, 97_000., 0.30, 14), row("writing", None, 46_000., 0.14, 5)] };
            json!({ "totals": totals, "daily": daily, "providers": providers,
                "breakdowns": { "model": model, "agent": agent, "day": daily, "skill": skill },
                "coverage": [{"name": "This Mac", "scanned": now - 60_000., "stale": false, "issues": if partial { vec!["antigravity"] } else { vec![] } }],
                "unavailable": if partial { vec!["Cursor API call counts where the run only reports context size"] } else { vec![] },
                "computers": [{"key": "key_here", "name": "This Mac"}, {"key": "key_devbox", "name": "devbox"}],
                "bots": [{"key": "bot_mira", "name": "Mira"}], "scanning": false,
                "state": if empty { "empty" } else if partial { "partial" } else { "ready" }
            })
        }
        _ => return None,
    };
    Some((200, json!({ "data": data })))
}
