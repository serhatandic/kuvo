//! When a repeating wake-up comes back: on your clock (the time zone its folder names), keeping its
//! time of day and day of the week through daylight saving and a new time zone, as `kuvo-wake`
//! works it out.

use std::path::Path;

use jiff::{
    Timestamp,
    civil::{Date, Time},
    tz::{Disambiguation, TimeZone},
};

const DAY: i64 = 86_400;
const NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// The time zone a folder's times are in: its `timezone` file's, else the one in the folder above
/// it (a conversation's own folder has none of its own), from the computer's zone files or from
/// the rules kept beside it (`zoneinfo`) on a computer without them; this computer's own clock
/// without one; UTC for a name that isn't a zone.
pub fn zone_of(folder: &Path) -> TimeZone {
    let named = |at: &Path| {
        std::fs::read_to_string(at.join("timezone"))
            .ok()
            .filter(|name| !name.is_empty())
    };
    let (at, name) = match named(folder) {
        Some(name) => (folder.to_owned(), Some(name)),
        None => match folder
            .parent()
            .and_then(|up| named(up).map(|name| (up, name)))
        {
            Some((up, name)) => (up.to_owned(), Some(name)),
            None => (folder.to_owned(), None),
        },
    };
    let Some(name) = name else {
        return TimeZone::system();
    };
    let name = name.trim();
    if let Ok(zone) = jiff::tz::db().get(name) {
        return zone;
    }
    std::fs::read(at.join("zoneinfo"))
        .ok()
        .filter(|rules| !rules.is_empty())
        .and_then(|rules| TimeZone::tzif(name, &rules).ok())
        .unwrap_or(TimeZone::UTC)
}

/// A repeat's period, and the time of day (and for weeks the day) it keeps.
struct Repeat<'a> {
    every: &'a str,
    at: Option<&'a str>,
}

impl<'a> Repeat<'a> {
    fn of(repeat: &'a str) -> Self {
        match repeat.split_once('@') {
            Some((every, at)) => Self {
                every,
                at: Some(at),
            },
            None => Self {
                every: repeat,
                at: None,
            },
        }
    }
}

/// Whether a repeat reads: 30m, 2h, 1d, 1w (at least 5m), or weekdays; days, weeks and weekdays
/// may keep their time of day (`1d@0900`), and weeks their day too (`1w@Mon0900`).
pub fn repeats(repeat: &str) -> bool {
    let Repeat { every, at } = Repeat::of(repeat);
    if let Some(at) = at {
        if !(every.ends_with('d') || every.ends_with('w') || every == "weekdays") {
            return false;
        }
        let at =
            if every.ends_with('w') && at.len() >= 4 && day_number(&at[..at.len() - 4]).is_some() {
                &at[3..]
            } else {
                at
            };
        if !hhmm(at) {
            return false;
        }
    }
    super::folder::repeat_text(every).is_some()
}

fn hhmm(at: &str) -> bool {
    let bytes = at.as_bytes();
    bytes.len() == 4
        && bytes.iter().all(u8::is_ascii_digit)
        && matches!((bytes[0], bytes[1]), (b'0' | b'1', _) | (b'2', b'0'..=b'3'))
        && bytes[2] <= b'5'
}

fn day_number(name: &str) -> Option<i64> {
    NAMES
        .iter()
        .position(|each| *each == name)
        .map(|index| index as i64)
}

fn offset(zone: &TimeZone, at: i64) -> i64 {
    Timestamp::from_second(at)
        .map(|stamp| i64::from(zone.to_offset(stamp).seconds()))
        .unwrap_or_default()
}

/// The day of a time on the zone's clock, counted from 1 January 1970 (a Thursday).
fn day(zone: &TimeZone, at: i64) -> i64 {
    (at + offset(zone, at)).div_euclid(DAY)
}

fn weekday(day: i64) -> bool {
    (day + 3).rem_euclid(7) < 5
}

fn clock(zone: &TimeZone, at: i64) -> String {
    Timestamp::from_second(at)
        .map(|stamp| stamp.to_zoned(zone.clone()).strftime("%H%M").to_string())
        .unwrap_or_else(|_| "0000".into())
}

/// The time on `day` at `at` (`0900`) on the zone's clock: when clocks go back, the first of the
/// two; when they go forward, one in the hour skipped is that much later (2:30 is 3:30).
fn daytime(zone: &TimeZone, day: i64, at: &str) -> i64 {
    let hours: i8 = at[..2].parse().unwrap_or_default();
    let minutes: i8 = at[2..4].parse().unwrap_or_default();
    let date = Date::new(1970, 1, 1)
        .ok()
        .and_then(|epoch| epoch.checked_add(jiff::Span::new().days(day)).ok());
    let Some(date) = date else {
        return day * DAY;
    };
    let civil = date.to_datetime(Time::new(hours, minutes, 0, 0).unwrap_or_default());
    zone.to_ambiguous_zoned(civil)
        .disambiguate(Disambiguation::Compatible)
        .map(|zoned| zoned.timestamp().as_second())
        .unwrap_or(day * DAY)
}

/// A weekday wake-up's first time: on a weekend day, the next weekday at the same time of day.
pub fn next_weekday(zone: &TimeZone, due: i64) -> i64 {
    let mut on = day(zone, due);
    if weekday(on) {
        return due;
    }
    let at = clock(zone, due);
    while !weekday(on) {
        on += 1;
    }
    daytime(zone, on, &at)
}

/// A repeat as a wake-up's name keeps it, from the time it's due: days, weeks and weekdays with
/// the time of day they keep, and weeks with their day too.
pub fn kept(zone: &TimeZone, due: i64, repeat: &str) -> String {
    let Repeat { every, at } = Repeat::of(repeat);
    let at = at.map_or_else(|| clock(zone, due), str::to_owned);
    if every.ends_with('w') {
        let at = if at.starts_with(|ch: char| ch.is_ascii_digit()) {
            format!("{}{at}", NAMES[(day(zone, due) + 3).rem_euclid(7) as usize])
        } else {
            at
        };
        format!("{every}@{at}")
    } else if every.ends_with('d') || every == "weekdays" {
        format!("{every}@{at}")
    } else {
        repeat.to_owned()
    }
}

/// A repeating wake-up's next time after `now`, from the time it was due and its repeat as its
/// name keeps it (an older one without its time of day keeps the one it was due at).
pub fn next(zone: &TimeZone, due: i64, repeat: &str, now: i64) -> i64 {
    let Repeat { every, at } = Repeat::of(repeat);
    if let Some(count) = every.strip_suffix('m').or_else(|| every.strip_suffix('h')) {
        let unit = if every.ends_with('h') { 3_600 } else { 60 };
        let period = count.parse::<i64>().unwrap_or(1).max(1) * unit;
        return due + ((now - due) / period + 1) * period;
    }
    let at = at.map_or_else(|| clock(zone, due), str::to_owned);
    let (mut week_day, at) = match at
        .len()
        .checked_sub(4)
        .and_then(|cut| day_number(&at[..cut]))
    {
        Some(number) => (Some(number), at[3..].to_owned()),
        None => (None, at),
    };
    let days = if every == "weekdays" {
        week_day = None;
        1
    } else if let Some(weeks) = every.strip_suffix('w') {
        weeks.parse::<i64>().unwrap_or(1) * 7
    } else {
        week_day = None;
        every
            .strip_suffix('d')
            .and_then(|count| count.parse().ok())
            .unwrap_or(1)
    };
    let mut start = day(zone, due);
    // The day it keeps nearest the one it was due (a new zone can put that a day before or after).
    if let Some(wanted) = week_day {
        let mut shift = (wanted - (start + 3).rem_euclid(7) + 7).rem_euclid(7);
        if shift > 3 {
            shift -= 7;
        }
        start += shift;
    }
    // Whole periods missed first, but not one still to come today; then one at a time.
    let mut count = ((day(zone, now) - start) / days).max(1);
    loop {
        let at_time = daytime(zone, start + count * days, &at);
        if at_time > now && (every != "weekdays" || weekday(start + count)) {
            return at_time;
        }
        count += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokyo() -> TimeZone {
        TimeZone::get("Asia/Tokyo").unwrap()
    }

    #[test]
    fn repeats_keep_their_time_of_day_on_your_clock() {
        // 2030-01-01 09:00 in Tokyo is midnight UTC.
        let due = 1_893_456_000;
        assert_eq!(kept(&tokyo(), due, "1d"), "1d@0900");
        assert_eq!(kept(&tokyo(), due, "1w"), "1w@Tue0900");
        assert_eq!(kept(&tokyo(), due, "weekdays"), "weekdays@0900");
        assert_eq!(kept(&tokyo(), due, "30m"), "30m");
        assert_eq!(next(&tokyo(), due, "1d@0900", due + 60), due + DAY);
        assert_eq!(
            next(&tokyo(), due, "1d@0900", due + 3 * DAY + 60),
            due + 4 * DAY
        );
        assert_eq!(next(&tokyo(), due, "30m", due + 45 * 60), due + 60 * 60);
        // A Friday's weekday wake-up comes back on Monday.
        let friday = due + 3 * DAY;
        assert_eq!(
            next(&tokyo(), friday, "weekdays@0900", friday + 60),
            friday + 3 * DAY
        );
        assert_eq!(next(&tokyo(), due, "1w@Tue0900", due + 60), due + 7 * DAY);
    }

    #[test]
    fn daylight_saving_keeps_the_hour() {
        let york = TimeZone::get("America/New_York").unwrap();
        // 2030-03-09 09:00 EST, the day before clocks go forward.
        let due = 1_899_295_200;
        let after = next(&york, due, "1d@0900", due + 60);
        assert_eq!(after - due, DAY - 3_600);
        // 02:30 on the skipped hour is 03:30.
        let skipped = next(&york, due - 6 * 3_600 - 30 * 60, "1d@0230", due);
        assert_eq!(
            Timestamp::from_second(skipped)
                .unwrap()
                .to_zoned(york)
                .strftime("%H%M")
                .to_string(),
            "0330"
        );
    }

    #[test]
    fn repeats_that_read() {
        for good in ["1d@0900", "1w@Mon2359", "weekdays@0000", "2h", "1w@0900"] {
            assert!(repeats(good), "{good}");
        }
        for bad in ["2h@0900", "1d@2400", "1d@0960", "1w@Xyz0900", "0d"] {
            assert!(!repeats(bad), "{bad}");
        }
    }
}
