//! A bot's job, kept in its folder (`job.json`): what it does in a few words (what shows under its
//! name) and all of it. One from before bots kept it there is taken once from its memory's Job
//! section, or else from its description.

use std::path::Path;

use serde_json::{Map, Value, json};
use unicode_segmentation::UnicodeSegmentation;

use super::here::{BotHere, change_description, plain_name};
use crate::wakes::folder::moving;

const JOB_FILE: &str = "job.json";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Job {
    pub line: String,
    pub details: String,
}

impl Job {
    fn of(value: &Value) -> Option<Self> {
        let kept = value.as_object()?;
        Some(Self {
            line: kept
                .get("line")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            details: kept
                .get("details")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        })
    }
}

fn read_job(folder: &Path) -> Option<Job> {
    let kept: Value =
        serde_json::from_str(&kuvo_store::files::text_of(&folder.join(JOB_FILE))?).ok()?;
    Job::of(&kept)
}

/// The time now as Swift writes dates: to the second.
pub fn stamp() -> String {
    jiff::Timestamp::now()
        .strftime("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

pub fn project_job(folder: &Path, job: &Job) -> std::io::Result<()> {
    let file = folder.join(JOB_FILE);
    let fresh = folder.join(format!("{JOB_FILE}.new"));
    let value = json!({ "line": job.line, "details": job.details, "updated": stamp() });
    std::fs::write(&fresh, serde_json::to_vec_pretty(&value)?)?;
    std::fs::rename(&fresh, file)
}

pub fn write_job(here: &BotHere, job: &Job) -> std::io::Result<()> {
    let offline = here.described.contains_key("offlineEdits");
    change_description(
        &here.folder,
        |now| {
            let Some(mut now) = now else {
                return Err(std::io::Error::other("That bot isn't here any more."));
            };
            project_job(&here.folder, job)?;
            // COMPAT(0.7 bot-json-job): the line in the description is what an earlier Mac reads.
            if now.contains_key("offlineJob") {
                now.insert("job".into(), json!(job.line));
                now.insert(
                    "offlineJob".into(),
                    json!({ "line": job.line, "details": job.details }),
                );
                return Ok(Some(now));
            }
            if now.get("job").and_then(Value::as_str) == Some(job.line.as_str()) {
                return Ok(None);
            }
            now.insert("job".into(), json!(job.line));
            Ok(Some(now))
        },
        offline,
    )
}

/// A bot's job; one from before is kept in its folder as it's read (`save`), not while it moves.
pub fn job_of(here: &BotHere, save: bool) -> Job {
    if let Some(atomic) = here.described.get("offlineJob")
        && atomic.get("line").is_some_and(Value::is_string)
        && atomic.get("details").is_some_and(Value::is_string)
    {
        return Job::of(atomic).unwrap_or_default();
    }
    if let Some(kept) = read_job(&here.folder) {
        return kept;
    }
    let memory = kuvo_store::files::text_of(&here.folder.join("MEMORY.md")).unwrap_or_default();
    let job = match job_in_memory(&memory) {
        Some(memory) => Job {
            details: details_beyond(&memory.line, &memory.section),
            line: memory.line,
        },
        None => Job {
            line: plain_name(here.field("job").unwrap_or_default()),
            details: String::new(),
        },
    };
    if save && !moving(&here.folder) {
        let _ = write_job(here, &job);
    }
    job
}

fn is_space(ch: char) -> bool {
    ch == '\t' || is_zs(ch)
}

fn is_zs(ch: char) -> bool {
    matches!(
        ch,
        ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
    )
}

/// Spaces and tabs trimmed off both ends.
fn spaces(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// Separators and line ends trimmed off both ends.
pub fn trimmed(text: &str) -> &str {
    text.trim_matches(|ch: char| {
        is_zs(ch)
            || matches!(
                ch,
                '\u{2028}' | '\u{2029}' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | '\u{85}'
            )
    })
}

fn heading_level(line: &str) -> Option<usize> {
    let text = spaces(line);
    let marks = text.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&marks) {
        return None;
    }
    let after = &text[marks..];
    (after.is_empty() || after.starts_with(' ')).then_some(marks)
}

fn is_job_heading(line: &str) -> bool {
    let text = spaces(line);
    if !text.starts_with('#') {
        return false;
    }
    let title = spaces(text.trim_start_matches('#')).to_lowercase();
    if matches!(title.as_str(), "job" | "my job" | "the job") || title.starts_with("job:") {
        return true;
    }
    let Some(rest) = title.strip_prefix("job ") else {
        return false;
    };
    spaces(rest)
        .chars()
        .next()
        .is_some_and(|first| "-–—:(".contains(first))
}

fn job_in_heading(line: &str) -> Option<(String, bool)> {
    let title = spaces(spaces(line).trim_start_matches('#'));
    if !title.to_lowercase().starts_with("job") {
        return None;
    }
    let mut rest = spaces(title.get(3..)?);
    let mark = rest.chars().next()?;
    if !":-–—(".contains(mark) {
        return None;
    }
    rest = spaces(&rest[mark.len_utf8()..]);
    if mark == '(' && rest.ends_with(')') {
        rest = spaces(&rest[..rest.len() - 1]);
    }
    (!rest.is_empty()).then(|| (rest.to_owned(), mark == ':'))
}

/// A job as a memory kept it: its line, and its section, all of it.
pub struct MemoryJob {
    pub line: String,
    pub section: String,
}

pub fn job_in_memory(text: &str) -> Option<MemoryJob> {
    let all: Vec<&str> = text.split('\n').collect();
    let start = all.iter().position(|line| is_job_heading(line))?;
    let level = heading_level(all[start]).unwrap_or(1);
    let mut end = all.len();
    let mut in_code = false;
    for (index, line) in all.iter().enumerate().skip(start + 1) {
        let spaced = spaces(line);
        if spaced.starts_with("```") || spaced.starts_with("~~~") {
            in_code = !in_code;
            continue;
        }
        if !in_code && heading_level(line).is_some_and(|other| other <= level) {
            end = index;
            break;
        }
    }
    let mut section = trimmed(&all[start + 1..end].join("\n")).to_owned();
    if let Some((inline, colon)) = job_in_heading(all[start])
        && (colon || section.is_empty())
    {
        section = [inline, section]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
    }
    let first = section
        .split('\n')
        .map(spaces)
        .find(|part| !part.is_empty())
        .unwrap_or_default();
    Some(MemoryJob {
        line: job_line(first),
        section,
    })
}

/// A job's line as it shows under a bot's name: no list mark, bold or "Job:", its first sentence
/// when it goes on, without the sentence's end.
pub fn job_line(text: &str) -> String {
    let mut line = spaces(text).to_owned();
    for mark in ["- ", "* ", "> "] {
        if let Some(rest) = line.strip_prefix(mark) {
            line = rest.to_owned();
        }
    }
    line = line.replace("**", "").replace("__", "");
    if line.to_lowercase().starts_with("job:") {
        line = line[4..].to_owned();
    }
    let mut line = spaces(&line).to_owned();
    if line.graphemes(true).count() > 48
        && let Some(end) = line.find(". ")
    {
        line.truncate(end);
    }
    if line.ends_with('.') && !line.ends_with("..") {
        line.pop();
    }
    line
}

/// A bot's job as the first message to it, for your composer: what it does in a few words, then
/// how, then to start; empty for no job at all.
pub fn first_message_of(line: &str, details: &str) -> String {
    let job = trimmed(line);
    let rest = details_beyond(job, details);
    if job.is_empty() && rest.is_empty() {
        return String::new();
    }
    let start = "Start now: ask me for anything you need first, do the first round, and if it repeats or keeps an eye on something, set that up.";
    [
        if job.is_empty() {
            String::new()
        } else {
            format!("Your job, in a few words: {job}.")
        },
        rest,
        start.to_owned(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n")
}

/// A job's details without its line, for details that begin with it: said once.
pub fn details_beyond(job: &str, details: &str) -> String {
    let mut lines: Vec<String> = trimmed(details).split('\n').map(str::to_owned).collect();
    if job.is_empty() || job_line(&lines[0]) != job {
        return lines.join("\n");
    }
    let first = lines[0].clone();
    match first.find(". ") {
        Some(end) if !spaces(&first[end + 2..]).is_empty() && job_line(&first[..end]) == job => {
            lines[0] = spaces(&first[end + 2..]).to_owned();
        }
        _ => {
            lines.remove(0);
        }
    }
    trimmed(&lines.join("\n")).to_owned()
}

/// A job as a bot or Edit Bot gives it (`{line, details}`), or why it can't be.
pub fn given_job(given: &Value) -> Result<Job, &'static str> {
    let shape = "A job is {line, details}: what it does in a few words, and the details.";
    let given = given.as_object().ok_or(shape)?;
    let line = given.get("line").and_then(Value::as_str).ok_or(shape)?;
    let details = match given.get("details") {
        None => "",
        Some(Value::String(details)) => details,
        Some(_) => return Err(shape),
    };
    let job = Job {
        line: job_line(&plain_name(line)),
        details: trimmed(details).to_owned(),
    };
    if job.line.is_empty() && (!line.is_empty() || !job.details.is_empty()) {
        return Err("Provide a non-empty job line, or explicitly clear the job.");
    }
    if job.line.chars().count() > 200 {
        return Err("Say what it does in a few words: that line is too long.");
    }
    if job.details.chars().count() > 8_000 {
        return Err("That job is too long to keep: shorten its details.");
    }
    Ok(job)
}

/// The job fields Edit Bot gives (`jobLine`, `jobDetails`), each only where given.
pub fn given_job_fields(
    fields: &Map<String, Value>,
) -> Result<(Option<String>, Option<String>), &'static str> {
    let line = match fields.get("jobLine") {
        None => None,
        Some(Value::String(line)) => {
            let line = job_line(&plain_name(line));
            if line.chars().count() > 200 {
                return Err("Shorten Job to 200 characters or fewer.");
            }
            Some(line)
        }
        Some(_) => return Err("Job must be text."),
    };
    let details = match fields.get("jobDetails") {
        None => None,
        Some(Value::String(details)) => {
            let details = trimmed(details).to_owned();
            if details.chars().count() > 8_000 {
                return Err("Shorten Instructions to 8,000 characters or fewer.");
            }
            Some(details)
        }
        Some(_) => return Err("Instructions must be text."),
    };
    Ok((line, details))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_from_a_memory_from_before() {
        let memory =
            "# Ada\n\n## Job\n- **Keeps the build green.** Every morning.\n\nMore.\n\n## Notes\nx";
        let job = job_in_memory(memory).unwrap();
        assert_eq!(job.line, "Keeps the build green. Every morning");
        let inline = job_in_memory("## Job: Watches the news\nDaily").unwrap();
        assert_eq!(inline.line, "Watches the news");
        assert_eq!(inline.section, "Watches the news\nDaily");
        assert!(job_in_memory("# Jobs\nnone").is_none());
    }

    #[test]
    fn lines_and_first_messages() {
        assert_eq!(
            job_line("Job: Keeps the build green."),
            "Keeps the build green"
        );
        assert_eq!(
            job_line("Reads the news every morning and tells me what matters. Then more."),
            "Reads the news every morning and tells me what matters"
        );
        assert_eq!(first_message_of("", ""), "");
        assert_eq!(
            first_message_of("Reads news", "Reads news.\nEvery day."),
            "Your job, in a few words: Reads news.\n\nEvery day.\n\nStart now: ask me for anything you need first, do the first round, and if it repeats or keeps an eye on something, set that up."
        );
    }
}
