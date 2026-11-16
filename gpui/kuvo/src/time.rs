//! Times as the screens say them.

use std::time::{SystemTime, UNIX_EPOCH};

/// Now, in ms since 1970.
pub fn now_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |since| since.as_millis() as f64)
}

/// How long ago `then` was, at `now` (both ms since 1970), as a sidebar row says it: "now", "5m",
/// "3h", "2d", "1w", "1mo", "1y".
pub fn ago(then: f64, now: f64) -> String {
    let seconds = ((now - then) / 1000.0).max(0.0);
    let minutes = seconds / 60.0;
    let hours = minutes / 60.0;
    let days = hours / 24.0;
    if minutes < 1.0 {
        "now".into()
    } else if hours < 1.0 {
        format!("{}m", minutes as u64)
    } else if days < 1.0 {
        format!("{}h", hours as u64)
    } else if days < 7.0 {
        format!("{}d", days as u64)
    } else if days < 30.0 {
        format!("{}w", (days / 7.0) as u64)
    } else if days < 365.0 {
        format!("{}mo", (days / 30.0) as u64)
    } else {
        format!("{}y", (days / 365.0) as u64)
    }
}

/// When something last looked (the app for an update), at `now`: "Last checked 5 minutes ago."
pub fn last_checked(checked: Option<f64>, now: f64) -> String {
    let Some(checked) = checked else { return "Not checked yet.".into() };
    let minutes = ((now - checked) / 60_000.0).max(0.0) as u64;
    let (count, unit) = match minutes {
        0 => return "Last checked just now.".into(),
        1..60 => (minutes, "minute"),
        60..1440 => (minutes / 60, "hour"),
        _ => (minutes / 1440, "day"),
    };
    format!("Last checked {count} {unit}{} ago.", if count == 1 { "" } else { "s" })
}

/// How long until `then`, at `now`: "52 min", "3 h 40 min", "4 days".
pub fn until(then: f64, now: f64) -> String {
    let minutes = (((then - now) / 60_000.0).max(0.0)).ceil() as u64;
    if minutes < 60 {
        format!("{} min", minutes.max(1))
    } else if minutes < 24 * 60 {
        match minutes % 60 {
            0 => format!("{} h", minutes / 60),
            rest => format!("{} h {rest} min", minutes / 60),
        }
    } else {
        let days = minutes / (24 * 60);
        if days == 1 { "1 day".into() } else { format!("{days} days") }
    }
}

/// A turn's length: "Worked for 25s", "Worked for 3m 10s", "Worked for 1h 5m".
pub fn worked_for(ms: f64) -> String {
    let seconds = (ms / 1000.0).round().max(1.0) as u64;
    let length = if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        match seconds % 60 {
            0 => format!("{}m", seconds / 60),
            rest => format!("{}m {rest}s", seconds / 60),
        }
    } else {
        match (seconds % 3600) / 60 {
            0 => format!("{}h", seconds / 3600),
            rest => format!("{}h {rest}m", seconds / 3600),
        }
    };
    format!("Worked for {length}")
}

/// The days from 1970-01-01 to a day of the calendar.
pub fn day_number(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year.rem_euclid(400);
    let of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    era * 146_097 + of_era * 365 + of_era / 4 - of_era / 100 + of_year - 719_468
}

/// The day of the calendar that many days from 1970-01-01: its year, month and day.
pub fn civil(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let of_era = days.rem_euclid(146_097);
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * of_year + 2) / 153;
    let day = of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 { shifted + 3 } else { shifted - 9 };
    (year_of_era + era * 400 + i64::from(month <= 2), month, day)
}

/// A day as the box writes one (`YYYY-MM-DD`), read as its days from 1970-01-01.
pub fn parse_day(day: &str) -> Option<i64> {
    let mut parts = day.split('-');
    let (year, month, date) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || year.len() != 4 || month.len() != 2 || date.len() != 2 {
        return None;
    }
    let (year, month, date): (i64, i64, i64) = (year.parse().ok()?, month.parse().ok()?, date.parse().ok()?);
    ((1..=12).contains(&month) && (1..=31).contains(&date)).then(|| day_number(year, month, date))
}

/// The day `offset` days after the one a clock `ahead` seconds of UTC shows at `ms`, as the box
/// writes a day (`YYYY-MM-DD`).
pub fn day(ms: f64, ahead: i64, offset: i64) -> String {
    let (year, month, date) = civil(((ms / 1000.) as i64 + ahead).div_euclid(86_400) + offset);
    format!("{year:04}-{month:02}-{date:02}")
}

/// The day `offset` days after `day` (as the box writes one); none where `day` can't be read.
pub fn day_after(day: &str, offset: i64) -> Option<String> {
    let (year, month, date) = civil(parse_day(day)? + offset);
    Some(format!("{year:04}-{month:02}-{date:02}"))
}

/// A due day in a few words, seen from `today` (both `YYYY-MM-DD`): "Today", "Tomorrow",
/// "Yesterday", else "Nov 16" (with its year where it isn't this one). One this can't read is
/// shown as written.
pub fn due_words(due: &str, today: &str) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (Some(day), Some(now)) = (parse_day(due), parse_day(today)) else { return due.to_owned() };
    match day - now {
        0 => "Today".into(),
        1 => "Tomorrow".into(),
        -1 => "Yesterday".into(),
        _ => {
            let ((year, month, date), (this_year, _, _)) = (civil(day), civil(now));
            let month = MONTHS[(month - 1) as usize];
            if year == this_year { format!("{month} {date}") } else { format!("{month} {date}, {year}") }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_check_is_said_in_words() {
        let now = 1_000_000_000_000.0;
        assert_eq!(last_checked(None, now), "Not checked yet.");
        assert_eq!(last_checked(Some(now - 20_000.0), now), "Last checked just now.");
        assert_eq!(last_checked(Some(now - 60_000.0), now), "Last checked 1 minute ago.");
        assert_eq!(last_checked(Some(now - 5.0 * 3_600_000.0), now), "Last checked 5 hours ago.");
        assert_eq!(last_checked(Some(now - 3.0 * 86_400_000.0), now), "Last checked 3 days ago.");
    }

    #[test]
    fn times_read_as_a_sidebar_says_them() {
        let now = 1_000_000_000_000.0;
        let ago_ = |ms: f64| ago(now - ms, now);
        assert_eq!(ago_(10_000.0), "now");
        assert_eq!(ago_(5.0 * 60_000.0), "5m");
        assert_eq!(ago_(3.0 * 3_600_000.0), "3h");
        assert_eq!(ago_(2.0 * 86_400_000.0), "2d");
        assert_eq!(ago_(8.0 * 86_400_000.0), "1w");
        assert_eq!(ago_(45.0 * 86_400_000.0), "1mo");
        assert_eq!(worked_for(25_000.0), "Worked for 25s");
        assert_eq!(worked_for(190_000.0), "Worked for 3m 10s");
        assert_eq!(worked_for(3_900_000.0), "Worked for 1h 5m");
    }

    #[test]
    fn a_due_day_reads_from_today() {
        // 2026-11-14T02:13:20Z; a clock three hours behind UTC still shows the 13th.
        let ms = 1_794_622_400_000.0;
        assert_eq!((day(ms, 0, 0), day(ms, -3 * 3600, 0), day(ms, 0, 7), day(ms, 0, 20)), ("2026-11-14".into(), "2026-11-13".into(), "2026-11-21".into(), "2026-12-04".into()));
        assert_eq!(civil(day_number(2028, 2, 29)), (2028, 2, 29));
        let words = |due: &str| due_words(due, "2026-11-14");
        assert_eq!((words("2026-11-14"), words("2026-11-15"), words("2026-11-13"), words("2026-11-30"), words("2027-01-02"), words("soon")), ("Today".into(), "Tomorrow".into(), "Yesterday".into(), "Nov 30".into(), "Jan 2, 2027".into(), "soon".into()));
        assert_eq!(parse_day("2026-13-01"), None);
        assert_eq!((day_after("2026-12-30", 7), day_after("today", 1)), (Some("2027-01-06".into()), None));
    }
}
