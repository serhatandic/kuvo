//! `kuvo-wake`'s verbs for a conversation's or a bot's own wake-ups (add, watch, list, cancel), for
//! a computer that can't run the script (native Windows): the same folders, checks and words.

use std::{
    fs,
    future::Future,
    path::{Path, PathBuf},
};

use jiff::{Timestamp, civil::DateTime, tz::TimeZone};

use super::{
    Wakes, clock,
    fire::now,
    folder::{self, names, text},
    routes::Said,
};

/// What's asked: `kuvo-wake <whose> <verb> …`'s verb and its words.
pub enum Verb {
    Add {
        when: String,
        every: Option<String>,
        summary: Option<String>,
        note: String,
    },
    Watch {
        every: String,
        check: String,
        summary: Option<String>,
        note: String,
    },
    List,
    Cancel {
        id: String,
    },
}

/// A time as `kuvo-wake` reads it (seconds since 1970): +30m, +2h, +1d; a time like
/// 2026-10-16T09:00 on the folder's clock, or with its zone (Z, +03:00, -0500); or seconds.
fn seconds(when: &str, zone: &TimeZone, now: i64) -> Option<i64> {
    if let Some(count) = when.strip_prefix('+') {
        let (count, unit) = count.split_at(count.len().checked_sub(1)?);
        // (No leading zero, which the script reads as octal.)
        if count.is_empty()
            || count.starts_with('0')
            || count.len() > 6
            || !count.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let count: i64 = count.parse().ok()?;
        return match unit {
            "m" => Some(now + count * 60),
            "h" => Some(now + count * 3600),
            "d" => Some(now + count * 86400),
            _ => None,
        };
    }
    if !when.is_empty() && when.bytes().all(|b| b.is_ascii_digit()) {
        return when.parse().ok();
    }
    if !when.is_ascii() {
        return None;
    }
    let mut at = when.replacen('T', " ", 1);
    let mut offset: Option<i32> = None;
    let colon = at.contains(':');
    if let Some(rest) = at.strip_suffix('Z') {
        offset = Some(0);
        at = rest.to_owned();
    } else if colon && at.len() > 6 {
        let tail = &at[at.len() - 6..];
        let short = &at[at.len() - 5..];
        let sign = |c: u8| (c == b'+' || c == b'-').then_some(if c == b'-' { -1 } else { 1 });
        let digits = |text: &str| text.bytes().all(|b| b.is_ascii_digit());
        if let Some(sign) = sign(tail.as_bytes()[0])
            .filter(|_| tail.as_bytes()[3] == b':' && digits(&tail[1..3]) && digits(&tail[4..]))
        {
            offset = Some(
                sign * (tail[1..3].parse::<i32>().ok()? * 3600
                    + tail[4..].parse::<i32>().ok()? * 60),
            );
            at.truncate(at.len() - 6);
        } else if let Some(sign) = sign(short.as_bytes()[0]).filter(|_| digits(&short[1..])) {
            offset = Some(
                sign * (short[1..3].parse::<i32>().ok()? * 3600
                    + short[3..].parse::<i32>().ok()? * 60),
            );
            at.truncate(at.len() - 5);
        }
    }
    if at.matches(':').count() == 1 {
        at.push_str(":00");
    }
    // Only "2026-10-16 09:00:00", as every computer's `date` reads it.
    let shape = at.len() == 19
        && at.bytes().enumerate().all(|(i, b)| match i {
            4 | 7 => b == b'-',
            10 => b == b' ',
            13 | 16 => b == b':',
            _ => b.is_ascii_digit(),
        });
    if !shape {
        return None;
    }
    let civil: DateTime = at.replacen(' ', "T", 1).parse().ok()?;
    let stamp = match offset {
        Some(offset) => civil
            .to_zoned(TimeZone::fixed(
                jiff::tz::Offset::from_seconds(offset).ok()?,
            ))
            .ok()?,
        None => civil.to_zoned(zone.clone()).ok()?,
    };
    Some(stamp.timestamp().as_second())
}

/// A time as `kuvo-wake` says it: "Mon Oct 6, 09:00".
fn readable(zone: &TimeZone, at: i64) -> String {
    Timestamp::from_second(at)
        .map(|stamp| {
            stamp
                .to_zoned(zone.clone())
                .strftime("%a %b %-d, %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

/// A summary on one line, short: what its card shows.
fn summed(summary: Option<&str>) -> Result<Option<String>, String> {
    let Some(summary) = summary else {
        return Ok(None);
    };
    let line: String = summary
        .chars()
        .map(|c| if "\n\r\t".contains(c) { ' ' } else { c })
        .collect();
    let line = line.trim_matches(' ').to_owned();
    if line.len() > 300 {
        return Err("Keep the summary to one short line: it's what the user sees for it, and you get the whole note.".into());
    }
    Ok(Some(line).filter(|line| !line.is_empty()))
}

fn first_line(file: &Path) -> String {
    text(file).lines().next().unwrap_or_default().to_owned()
}

/// What a folder has waiting, as `kuvo-wake list` lists it.
fn listed(folder: &Path, zone: &TimeZone) -> String {
    let canceled = |letters: &str| folder.join("inbox/cancel").join(letters).exists();
    let mut lines = Vec::new();
    for at in ["wakes", "inbox/wakes"] {
        let mut ids = names(&folder.join(at));
        ids.sort();
        for id in ids {
            let file = folder.join(at).join(&id);
            if !file.is_file() {
                continue;
            }
            let mut parts = id.splitn(3, '-');
            let (due, letters, every) = (
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or_default(),
                parts.next(),
            );
            if canceled(letters) {
                continue;
            }
            let every = every
                .map(|every| format!(" (every {})", every.split('@').next().unwrap_or_default()))
                .unwrap_or_default();
            let due = readable(zone, due.parse().unwrap_or_default());
            lines.push(format!("{id}  {due}{every}  {}", first_line(&file)));
        }
    }
    for at in ["watches", "inbox/watches"] {
        let mut all = names(&folder.join(at));
        all.sort();
        for letters in all {
            let watch = folder.join(at).join(&letters);
            if !watch.join("check").is_file() || canceled(&letters) {
                continue;
            }
            let (every, check, note) = (
                text(&watch.join("every")),
                first_line(&watch.join("check")),
                first_line(&watch.join("note")),
            );
            lines.push(format!(
                "{letters}  watch every {every}: {check}  ->  {note}"
            ));
        }
    }
    if lines.is_empty() {
        "No wake-ups scheduled.".to_owned()
    } else {
        lines.join("\n")
    }
}

/// A wake-up or watch canceled by its ID (or a wake-up's letters, which a repeating one keeps).
fn canceled(folder: &Path, id: &str) -> Result<String, String> {
    let letters = id.split_once('-').map_or(id, |(_, rest)| rest);
    let letters = letters.split('-').next().unwrap_or_default();
    let fits = !id.starts_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '@' || c == '-');
    let mut found = false;
    if fits && folder::letters(letters) {
        for name in names(&folder.join("wakes")) {
            if folder::WakeName::parse(&name).is_some_and(|wake| wake.letters == letters) {
                found |= fs::remove_file(folder.join("wakes").join(name)).is_ok();
            }
        }
        if found {
            let _ = fs::remove_file(folder.join("summaries").join(letters));
        }
        let watch = folder.join("watches").join(letters);
        if letters == id && watch.is_dir() {
            found |= fs::remove_dir_all(watch).is_ok();
        }
    }
    if found {
        Ok(format!("Canceled {id}."))
    } else {
        Err(format!("No wake-up \"{id}\". See list_wake_ups."))
    }
}

impl Wakes {
    /// `verb` for a bot's folder (`bot`), else the conversation `session`'s, made with its first
    /// wake-up or watch: what it says, or why it was refused.
    pub async fn run_verb(
        &self,
        bot: Option<&Path>,
        session: &str,
        verb: Verb,
    ) -> Result<String, String> {
        let own = self.roots().sessions.join(session);
        let folder = bot.map_or(own, Path::to_path_buf);
        let fresh = bot.is_none() && !folder.is_dir();
        let said = verb_in(folder, fresh, verb, || self.folder_for(session)).await;
        if said.is_ok() {
            self.changed();
        }
        said
    }
}

/// `verb` in `folder`; `fresh`: it isn't there yet, and comes of `make` once what's asked checks
/// out.
pub(crate) async fn verb_in<F: Future<Output = Option<PathBuf>>>(
    folder: PathBuf,
    fresh: bool,
    verb: Verb,
    make: impl FnOnce() -> F,
) -> Result<String, String> {
    let zone = clock::zone_of(&folder);
    let made = || async {
        if !fresh {
            return Ok(folder.clone());
        }
        make()
            .await
            .ok_or_else(|| format!("Couldn't keep it in {}.", folder.display()))
    };
    match verb {
        Verb::List if fresh => Ok("No wake-ups scheduled.".into()),
        Verb::List => Ok(listed(&folder, &zone)),
        Verb::Cancel { id } => canceled(&folder, &id),
        Verb::Add {
            when,
            every,
            summary,
            note,
        } => {
            let (every, summary) = (every.as_deref(), summary.as_deref());
            let due = seconds(&when, &zone, now()).ok_or_else(|| {
                    format!("Couldn't read the time \"{when}\". Use +30m, +2h, +1d, or a time like 2026-10-16T09:00.")
                })?;
            if let Some(every) = every.filter(|every| !clock::repeats(every)) {
                return Err(format!(
                    "Couldn't read the repeat \"{every}\". Use 30m, 2h, 1d, 1w (at least 5m), or weekdays."
                ));
            }
            let summary = summed(summary)?;
            if note.is_empty() {
                return Err("Add a note for yourself: what to do when you wake up.".into());
            }
            let every = every.map(|every| every.split('@').next().unwrap_or_default());
            let shifted = if every == Some("weekdays") {
                clock::next_weekday(&zone, due)
            } else {
                due
            };
            if shifted <= now() + 30 {
                return Err(
                    "That's not in the future. Wake-ups are for later; do it now instead.".into(),
                );
            }
            let folder = made().await?;
            let id = Wakes::add_wake(&folder, due, every, summary.as_deref(), &note)
                .map_err(|Said(words)| words)?;
            let then = every
                .map(|every| format!(", then every {every} until canceled"))
                .unwrap_or_default();
            Ok(format!(
                "Scheduled {id} for {}{then}.",
                readable(&zone, shifted)
            ))
        }
        Verb::Watch {
            every,
            check,
            summary,
            note,
        } => {
            let summary = summary.as_deref();
            if !super::routes::period(&every) {
                return Err(format!(
                    "Couldn't read how often, \"{every}\". Use 5m or more, 2h or 1d."
                ));
            }
            if check.is_empty() {
                return Err("Give the check: a shell command that prints what you watch, and prints something else when it changes.".into());
            }
            let summary = summed(summary)?;
            if note.is_empty() {
                return Err("Add a note for yourself: what to do when it changes.".into());
            }
            let folder = made().await?;
            let letters = Wakes::add_watch(&folder, &every, &check, summary.as_deref(), &note)
                .map_err(|Said(words)| words)?;
            Ok(format!(
                "Watching ({letters}): every {every} the check runs, without you, and when what it prints changes (not the first time), you wake up with the new output. Cancel it with: cancel_wake_up {letters}"
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_reads_as_kuvo_wake_reads_it() {
        let zone = TimeZone::get("Europe/Istanbul").unwrap();
        let now = 1_800_000_000;
        assert_eq!(seconds("+30m", &zone, now), Some(now + 1800));
        assert_eq!(seconds("+08h", &zone, now), None);
        assert_eq!(seconds("1700000000", &zone, now), Some(1_700_000_000));
        assert_eq!(seconds("2026-10-16T09:00", &zone, now), Some(1_792_130_400));
        assert_eq!(
            seconds("2026-10-16T09:00Z", &zone, now),
            Some(1_792_141_200)
        );
        assert_eq!(
            seconds("2026-10-16T09:00:00+01:00", &zone, now),
            Some(1_792_137_600)
        );
        assert_eq!(seconds("yesterday-ish", &zone, now), None);
        assert_eq!(readable(&zone, 1_792_130_400), "Fri Oct 16, 09:00");
    }

    /// The same asks of a bot's folder, done here and by the script: the same words (ids aside).
    #[cfg(unix)]
    #[tokio::test]
    async fn the_verbs_say_what_kuvo_wake_says() {
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../src/keeper/kuvo-wake");
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("kuvo-wake-verbs-{}-{nanos}", std::process::id()));
        let (here, there) = (root.join("here/bots/bot_a"), root.join("there/bots/bot_a"));
        for folder in [&here, &there] {
            fs::create_dir_all(folder).unwrap();
            fs::write(folder.join("timezone"), "Europe/Istanbul").unwrap();
        }
        let ids = regex::Regex::new(r"(?m)[0-9]{10}-[A-Za-z0-9]{6}|\(([A-Za-z0-9]{6})\)|cancel_wake_up [A-Za-z0-9]{6}|^[A-Za-z0-9]{6}  watch").unwrap();
        let plain = |text: &str| ids.replace_all(text, "ID").into_owned();
        let owned = |text: &str| text.to_owned();
        let cases: Vec<(Vec<&str>, Verb)> = vec![
            (
                vec!["add", "2030-01-07T09:00", "--summary", "CI", "Check CI."],
                Verb::Add {
                    when: owned("2030-01-07T09:00"),
                    every: None,
                    summary: Some(owned("CI")),
                    note: owned("Check CI."),
                },
            ),
            (
                vec!["add", "2030-01-08T10:00", "--every", "1d", "Daily."],
                Verb::Add {
                    when: owned("2030-01-08T10:00"),
                    every: Some(owned("1d")),
                    summary: None,
                    note: owned("Daily."),
                },
            ),
            (
                vec![
                    "add",
                    "2030-01-12T11:00",
                    "--every",
                    "weekdays",
                    "Weekdays.",
                ],
                Verb::Add {
                    when: owned("2030-01-12T11:00"),
                    every: Some(owned("weekdays")),
                    summary: None,
                    note: owned("Weekdays."),
                },
            ),
            (
                vec!["add", "+0h", "Soon."],
                Verb::Add {
                    when: owned("+0h"),
                    every: None,
                    summary: None,
                    note: owned("Soon."),
                },
            ),
            (
                vec!["add", "+2h", "--every", "3x", "Odd."],
                Verb::Add {
                    when: owned("+2h"),
                    every: Some(owned("3x")),
                    summary: None,
                    note: owned("Odd."),
                },
            ),
            (
                vec!["add", "2020-01-01T09:00", "Past."],
                Verb::Add {
                    when: owned("2020-01-01T09:00"),
                    every: None,
                    summary: None,
                    note: owned("Past."),
                },
            ),
            (
                vec!["watch", "2m", "git log -1", "Changed."],
                Verb::Watch {
                    every: owned("2m"),
                    check: owned("git log -1"),
                    summary: None,
                    note: owned("Changed."),
                },
            ),
            (
                vec!["watch", "10m", "", "Changed."],
                Verb::Watch {
                    every: owned("10m"),
                    check: String::new(),
                    summary: None,
                    note: owned("Changed."),
                },
            ),
            (
                vec![
                    "watch",
                    "10m",
                    "git log -1",
                    "--summary",
                    "Main",
                    "Say what changed.",
                ],
                Verb::Watch {
                    every: owned("10m"),
                    check: owned("git log -1"),
                    summary: Some(owned("Main")),
                    note: owned("Say what changed."),
                },
            ),
            (vec!["list"], Verb::List),
            (vec!["cancel", "nope"], Verb::Cancel { id: owned("nope") }),
        ];
        let script_says = |folder: &Path, args: &[&str]| {
            let ran = std::process::Command::new("sh")
                .arg(script)
                .arg(folder)
                .args(args)
                .env_clear()
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .env("HOME", &root)
                .env("LC_ALL", "C")
                .env("KUVO_WAKE_TOOL", "1")
                .output()
                .unwrap();
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&ran.stdout),
                String::from_utf8_lossy(&ran.stderr)
            );
            (ran.status.success(), text.trim().to_owned())
        };
        let mut wrong = Vec::new();
        for (args, verb) in cases {
            let native = verb_in(here.clone(), false, verb, || async { None }).await;
            let native = (native.is_ok(), native.unwrap_or_else(|why| why));
            let theirs = script_says(&there, &args);
            if (native.0, plain(&native.1)) != (theirs.0, plain(&theirs.1)) {
                wrong.push(format!("{args:?}\n  here  {native:?}\n  there {theirs:?}"));
            }
        }
        // Each one's first wake-up canceled by its letters, and what's left listed alike.
        for (folder, theirs) in [(&here, false), (&there, true)] {
            let first = names(&folder.join("wakes")).into_iter().min().unwrap();
            let letters = first.split('-').nth(1).unwrap().to_owned();
            let said = if theirs {
                script_says(folder, &["cancel", &letters])
            } else {
                let said = verb_in(
                    folder.clone(),
                    false,
                    Verb::Cancel {
                        id: letters.clone(),
                    },
                    || async { None },
                )
                .await;
                (said.is_ok(), said.unwrap_or_else(|why| why))
            };
            assert_eq!(said, (true, format!("Canceled {letters}.")));
        }
        let listed_here = verb_in(here.clone(), false, Verb::List, || async { None })
            .await
            .unwrap();
        let listed_there = script_says(&there, &["list"]).1;
        if plain(&listed_here) != plain(&listed_there) {
            wrong.push(format!(
                "list\n  here  {listed_here}\n  there {listed_there}"
            ));
        }
        let _ = fs::remove_dir_all(&root);
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }
}
