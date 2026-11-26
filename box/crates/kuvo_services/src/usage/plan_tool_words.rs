//! Read-only tool words from the same accepted, merged rows as the panel.
use super::{
    plan_facts::{Balance, Extra, Problem, Unit},
    readings::Window,
};
use jiff::{Timestamp, tz::TimeZone};
use serde::Deserialize;
use serde_json::Value;
#[derive(Deserialize)]
struct Holder {
    name: String,
    provider: Option<String>,
    #[serde(default)]
    here: bool,
}
#[derive(Deserialize)]
struct ShownWindow {
    #[serde(flatten)]
    window: Window,
    #[serde(default)]
    expired: bool,
}
#[derive(Deserialize)]
struct Row {
    id: String,
    provider: String,
    service: String,
    agent: Option<String>,
    label: Option<String>,
    plan: Option<String>,
    #[serde(default)]
    boxes: Vec<Holder>,
    #[serde(default)]
    away: bool,
    #[serde(default)]
    ended: bool,
    #[serde(default)]
    windows: Vec<ShownWindow>,
    balance: Option<Balance>,
    #[serde(default)]
    extras: Vec<Value>,
    problem: Option<Problem>,
}
fn title<'a>(service: &'a str, agent: Option<&'a str>) -> &'a str {
    match (agent, service) {
        (Some("claude"), _) => "Claude Code",
        (Some("codex"), _) => "Codex",
        (Some("grok"), _) => "Grok",
        (Some("cursor"), _) => "Cursor",
        (Some("antigravity"), _) => "Antigravity",
        (Some(unknown), _) => unknown,
        (None, "codex") => "ChatGPT",
        (None, "grok") => "SuperGrok",
        (None, "copilot") => "GitHub Copilot",
        (None, "openrouter") => "OpenRouter",
        (None, "zai") => "Z.ai",
        (None, "ollama") => "Ollama",
        (None, "opencode") => "OpenCode Go",
        (None, unknown) => unknown,
    }
}
fn amount(n: f64, unit: Unit) -> String {
    match unit {
        Unit::Usd => format!("${n:.2}"),
        Unit::Requests => format!("{} requests", n.round()),
        Unit::Credits => format!("{} credits", n.round()),
    }
}
fn since(at: u64, now: u64) -> String {
    let minutes = now.saturating_sub(at).saturating_add(30_000) / 60_000;
    match minutes {
        0 => "just now".into(),
        1..=59 => format!("{minutes}m ago"),
        60..=2879 => format!("{}h ago", minutes / 60),
        _ => format!("{} days ago", (minutes / 60 + 12) / 24),
    }
}
fn until(at: u64, now: u64) -> String {
    let minutes = (at.saturating_sub(now).saturating_add(30_000) / 60_000).max(1);
    if minutes < 60 {
        format!("in {minutes}m")
    } else if minutes < 2880 {
        let extra = minutes % 60;
        format!(
            "in {}h{}",
            minutes / 60,
            if extra == 0 {
                String::new()
            } else {
                format!(" {extra}m")
            }
        )
    } else {
        format!("in {} days", (minutes / 60 + 12) / 24)
    }
}
fn absolute(at: u64, now: u64, zone: &TimeZone) -> String {
    let stamp = |n: u64| {
        Timestamp::from_millisecond(i64::try_from(n).ok()?)
            .ok()
            .map(|t| t.to_zoned(zone.clone()))
    };
    let Some(at) = stamp(at) else {
        return String::new();
    };
    let time = at.strftime("%-I:%M %p").to_string();
    if stamp(now).is_some_and(|n| n.date() == at.date()) {
        time
    } else {
        format!("{}, {time}", at.strftime("%b %-d"))
    }
}
fn span(window: &Window) -> String {
    let length = match window.minutes {
        Some(300) => "5-hour window".into(),
        Some(10080) => "weekly limit".into(),
        Some(40000..=45000) => "monthly limit".into(),
        Some(m) if m >= 1440 => format!("{}-day window", (m + 720) / 1440),
        Some(m) => format!("{}-hour window", (m + 30) / 60),
        None => "limit".into(),
    };
    if let Some(name) = window.name.as_ref().filter(|s| !s.is_empty()) {
        return name.clone();
    }
    window
        .model
        .as_ref()
        .filter(|s| !s.is_empty())
        .map_or(length.clone(), |m| format!("{m} {length}"))
}
pub fn describe(name: &str, given: &Value, now: u64, zone: Option<&str>) -> String {
    let rows = given
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| serde_json::from_value::<Row>(v.clone()).ok())
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return format!(
            "No plan limits are known on {name}: no coding agent here is signed in to a plan whose limits can be read."
        );
    }
    let zone = zone
        .and_then(|zone| TimeZone::get(zone).ok())
        .unwrap_or_else(TimeZone::system);
    let mut lines = vec![format!(
        "Plan limits of your accounts (signed in on {name} unless it says where; used is how much of the window is gone):"
    )];
    for row in rows {
        let mut title = title(&row.service, row.agent.as_deref()).to_owned();
        if let Some(label) = row.label.as_ref().filter(|s| !s.is_empty()) {
            title.push_str(&format!(" ({label})"));
        }
        if let Some(plan) = row.plan.as_ref().filter(|s| !s.is_empty()) {
            title.push_str(&format!(", {plan}"));
        }
        if row.boxes.iter().any(|b| b.here) || row.boxes.is_empty() {
            title.push_str(&format!(
                ", provider {}",
                row.boxes
                    .iter()
                    .find(|b| b.here)
                    .and_then(|b| b.provider.as_deref())
                    .unwrap_or(row.provider.as_str())
            ));
        } else {
            title.push_str(&format!(
                ", signed in on {}{}",
                row.boxes
                    .iter()
                    .map(|b| b.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                if row.away {
                    " (not heard from lately: as last told)"
                } else {
                    ""
                }
            ));
        }
        lines.push(format!("- {title}:"));
        if row.ended {
            lines.push("  - its plan has ended (not active now): nothing to use on it".into());
            continue;
        }
        for shown in &row.windows {
            let w = &shown.window;
            let title = span(w);
            if shown.expired {
                lines.push(format!("  - {title}: started over, not read since"));
                continue;
            }
            if let Some(at) = w.resets_at.filter(|at| *at <= now) {
                lines.push(format!(
                    "  - {title}: started over at {}, not read since",
                    absolute(at, now, &zone)
                ));
                continue;
            }
            let counted = w.amount.as_ref().map_or(String::new(), |a| {
                format!(
                    " ({} of {})",
                    amount(a.used, a.unit),
                    amount(a.limit, a.unit)
                )
            });
            let reset = w.resets_at.map_or(String::new(), |at| {
                format!(", resets {} ({})", until(at, now), absolute(at, now, &zone))
            });
            lines.push(format!(
                "  - {title}: {}% used{counted}{reset}, as of {}",
                (w.used * 100.).round(),
                since(w.read, now)
            ));
        }
        if let Some(b) = &row.balance {
            lines.push(format!(
                "  - credits: {} left{}",
                amount(b.left, b.unit),
                b.limit
                    .map_or(String::new(), |n| format!(" of {}", amount(n, b.unit)))
            ));
        }
        for given in &row.extras {
            let Ok(extra) = serde_json::from_value::<Extra>(given.clone()) else {
                continue;
            };
            let reset = given["resetsAt"].as_u64();
            if reset.is_some_and(|at| at <= now) {
                continue;
            }
            let resets = reset.map_or(String::new(), |at| {
                format!(", starts over {}", until(at, now))
            });
            match extra {
                Extra::Spent {
                    name,
                    amount: n,
                    unit,
                    ..
                } => lines.push(format!("  - {name}: {} spent{resets}", amount(n, unit))),
                Extra::Capped {
                    name,
                    amount: n,
                    limit,
                    unit,
                    ..
                } => lines.push(format!(
                    "  - {name}: {} of {} used{resets}",
                    amount(n, unit),
                    amount(limit, unit)
                )),
                Extra::Left {
                    name,
                    amount: n,
                    unit,
                    ..
                } => lines.push(format!("  - {name}: {} left", amount(n, unit))),
                Extra::Unreadable { message, .. } => lines.push(format!("  - {message}")),
                Extra::Resets { count, usable, .. } => {
                    let more = if count > 0 && usable {
                        let where_ = if given["on"]["here"] != true {
                            given["on"]["name"].as_str().map_or(String::new(), |name| {
                                format!(" (used on {name}, which has its sign-in)")
                            })
                        } else {
                            String::new()
                        };
                        format!(
                            "; use_limit_reset with account \"{}\" uses one{where_} ({})",
                            row.id,
                            if given["agents"] == true {
                                "the user allows it without asking"
                            } else {
                                "the user approves each"
                            }
                        )
                    } else if count > 0 {
                        "; none can be used from your computers now".into()
                    } else {
                        String::new()
                    };
                    lines.push(format!("  - rate-limit resets: {count} available{more}"));
                }
                Extra::Future(_) => {}
            }
        }
        if row.windows.is_empty() && row.balance.is_none() && row.extras.is_empty() {
            lines.push("  - none read yet".into());
        }
        if let Some(problem) = row.problem {
            let next = problem
                .until
                .filter(|at| *at > now)
                .map_or(String::new(), |at| {
                    format!(" Read again {}.", until(at, now))
                });
            lines.push(format!("  - Not read just now: {}{next}", problem.message));
        }
    }
    lines.join("\n")
}
