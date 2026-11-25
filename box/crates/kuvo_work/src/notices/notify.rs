//! notify_user (PROTOCOL.md, "Agents' tools"): an agent tells you something on purpose, in one
//! sentence, in your clients' notifications. A reply of an agent working on its own (a wake-up, a
//! watch, another agent's message) isn't news (NOTIFY_RULE), so this is how it reaches you then.
//! Limited, so an agent in a loop can't keep at it.

use serde_json::{Map, Value};

use super::{Notice, Told, news, words};

/// How many a conversation may send: in an hour, in a day, and how long after the last; and all
/// of this box's agents together in an hour (a bot's many sessions can't add up to a flood).
pub const HOUR_MOST: usize = 3;
pub const DAY_MOST: usize = 12;
pub const APART: i64 = 2 * 60_000;
pub const BOX_HOUR_MOST: usize = 10;

const HOUR: i64 = 60 * 60_000;
const DAY: i64 = 24 * HOUR;

/// Why one more from `session` saying `sentence` isn't sent now, given what's kept; none where it
/// may go.
pub fn past_limit(kept: &[Told], session: &str, sentence: &str, now: i64) -> Option<String> {
    let by_agents: Vec<&Told> = kept
        .iter()
        .filter(|each| each.by.as_deref() == Some("agent"))
        .collect();
    let sent: Vec<&&Told> = by_agents
        .iter()
        .filter(|each| each.notice.thread.as_deref() == Some(session))
        .collect();
    let within = |each: &&&Told, span: i64| now - each.at < span;
    let last_hour = sent.iter().filter(|each| within(each, HOUR)).count();
    let last_day: Vec<&&&Told> = sent.iter().filter(|each| within(each, DAY)).collect();
    if last_day
        .iter()
        .any(|each| each.notice.body.as_deref() == Some(sentence))
    {
        return Some(
            "You already sent the user that today. They'll see it; don't send it again.".to_owned(),
        );
    }
    if sent.last().is_some_and(|last| now - last.at < APART) {
        return Some(
            "You notified the user a moment ago. Wait for them: put anything more in your reply."
                .to_owned(),
        );
    }
    if last_hour >= HOUR_MOST {
        return Some(format!(
            "You've notified the user {last_hour} times this hour, the most there is. Put it in your reply; they'll read it there."
        ));
    }
    if last_day.len() >= DAY_MOST {
        return Some(format!(
            "You've notified the user {} times today, the most there is. Put it in your reply; they'll read it there.",
            last_day.len()
        ));
    }
    if by_agents.iter().filter(|each| now - each.at < HOUR).count() >= BOX_HOUR_MOST {
        return Some("The user has had many notifications from agents this hour. Put it in your reply; they'll read it there.".to_owned());
    }
    None
}

impl news::News<'_> {
    /// The notice a conversation's sentence makes: who's telling (a bot by its name, a
    /// conversation by its title, with its project) and the sentence.
    pub fn sent_notice(&self, session: &Value, name: Option<&str>, sentence: String) -> Notice {
        let bot = news::is_bot(session);
        let title = session["title"].as_str().filter(|title| !title.is_empty());
        let mut notice = self.about(
            session,
            if bot { "report" } else { "finished" },
            sentence,
            Map::new(),
            true,
            "New Session",
        );
        notice.title = bot
            .then_some(name)
            .flatten()
            .filter(|name| !name.is_empty())
            .or(title)
            .unwrap_or("New Session")
            .to_owned();
        notice
    }
}

/// notify_user's sentence: one, from what the agent wrote; none for nothing to say.
pub fn sentence_of(text: &Value) -> Option<String> {
    let text = match text {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    Some(words::one_sentence(&text, words::SENTENCE_LONGEST))
        .filter(|sentence| !sentence.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn told(session: &str, body: &str, at: i64, by_agent: bool) -> Told {
        let notice = Notice {
            body: Some(body.to_owned()),
            category: "finished".to_owned(),
            thread: Some(session.to_owned()),
            ..Notice::default()
        };
        let by = by_agent.then(|| "agent".to_owned());
        Told {
            id: format!("ntc_{at}"),
            at,
            notice,
            by,
        }
    }

    #[test]
    fn an_agent_is_held_to_its_limits_and_all_agents_together_too() {
        let (now, minute) = (100 * HOUR, 60_000);
        // The same sentence today, past news written since.
        let kept = [
            told("ses_a", "Ready.", now - 30 * minute, true),
            told("ses_x", "News.", now - minute, false),
        ];
        assert!(
            past_limit(&kept, "ses_a", "Ready.", now)
                .unwrap()
                .starts_with("You already sent")
        );
        assert_eq!(past_limit(&kept, "ses_a", "Other.", now), None);
        let soon = [told("ses_a", "One.", now - minute, true)];
        assert!(
            past_limit(&soon, "ses_a", "Two.", now)
                .unwrap()
                .contains("a moment ago")
        );
        let hour: Vec<Told> = (1..=3)
            .map(|n| told("ses_a", &format!("{n}."), now - n * 10 * minute, true))
            .collect();
        assert!(
            past_limit(&hour, "ses_a", "Four.", now)
                .unwrap()
                .contains("3 times this hour")
        );
        let day: Vec<Told> = (1..=12)
            .map(|n| told("ses_a", &format!("{n}."), now - n * HOUR, true))
            .collect();
        assert!(
            past_limit(&day, "ses_a", "More.", now)
                .unwrap()
                .contains("12 times today")
        );
        let many: Vec<Told> = (1..=10)
            .map(|n| told(&format!("ses_{n}"), "Hi.", now - n * 5 * minute, true))
            .collect();
        assert!(
            past_limit(&many, "ses_new", "Hello.", now)
                .unwrap()
                .contains("many notifications from agents")
        );
    }
}
