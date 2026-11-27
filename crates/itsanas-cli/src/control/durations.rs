//! Durations and dates in words: what a person types for an interval or a
//! pause, and how the daemon says them back.

use super::{MAX_INTERVAL, MAX_PAUSE, MIN_INTERVAL, MIN_PAUSE};

/// The last second of the year 9999. A pause ending later has no date worth
/// printing, and the date arithmetic below stays far from any overflow.
const LAST_DATED_SECOND: u64 = 253_402_300_799;

/// Read `10m`, `1h`, `90s`, `1d` or a bare number of seconds; `auto` is `None`.
///
/// # Errors
///
/// Anything else, and anything outside [`MIN_INTERVAL`]..=[`MAX_INTERVAL`]:
/// said at the command line rather than clamped there, so the person learns
/// the bound instead of getting a number they did not type.
pub fn parse_every(text: &str) -> Result<Option<u64>, String> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    let seconds = seconds_in(text)
        .ok_or_else(|| format!("`{text}` is not a duration: say 30s, 10m, 1h, 1d, or auto"))?;
    if seconds < MIN_INTERVAL.as_secs() || seconds > MAX_INTERVAL.as_secs() {
        return Err(format!(
            "{} is outside what a daemon can be asked for: between {} and {}",
            describe_every(seconds),
            describe_every(MIN_INTERVAL.as_secs()),
            describe_every(MAX_INTERVAL.as_secs()),
        ));
    }
    Ok(Some(seconds))
}

/// Read how long `itsanas pause --for` lasts: `30m`, `2h`, `1d`, in seconds.
///
/// # Errors
///
/// Anything else, and anything outside [`MIN_PAUSE`]..=[`MAX_PAUSE`], said
/// with the bounds and the way to pause with no end.
pub fn parse_pause(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let seconds =
        seconds_in(text).ok_or_else(|| format!("`{text}` is not a duration: say 30m, 2h or 1d"))?;
    if seconds < MIN_PAUSE.as_secs() || seconds > MAX_PAUSE.as_secs() {
        return Err(format!(
            "a pause lasts between {} and {}; `itsanas pause` with no --for lasts until \
             `itsanas resume`",
            describe_every(MIN_PAUSE.as_secs()),
            describe_every(MAX_PAUSE.as_secs()),
        ));
    }
    Ok(seconds)
}

/// `90s`, `10m`/`10min`, `2h`, `1d` or bare seconds; `None` for anything else or
/// a number too large to be seconds.
fn seconds_in(text: &str) -> Option<u64> {
    let (digits, unit) = text
        .find(|c: char| !c.is_ascii_digit())
        .map_or((text, ""), |at| text.split_at(at));
    let scale = match unit.trim() {
        "" | "s" => 1,
        "m" | "min" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => return None,
    };
    digits.parse::<u64>().ok()?.checked_mul(scale)
}

/// A wait in words, rounded up to the minute so "in 0 min" is never said of a
/// pause that still holds: `5 min`, `2 h 5 min`, `3 days`.
#[must_use]
pub fn describe_wait(seconds: u64) -> String {
    let minutes = seconds.div_ceil(60);
    match minutes {
        0..=119 => format!("{minutes} min"),
        120..=2879 if minutes.is_multiple_of(60) => format!("{} h", minutes / 60),
        120..=2879 => format!("{} h {} min", minutes / 60, minutes % 60),
        _ => format!("{} days", minutes / 1440),
    }
}

/// A Unix time as `2026-10-06 14:30 UTC`; `None` past the year 9999.
///
/// UTC, and said so: the daemon of a headless machine has no reliable local
/// zone, and a tray shows the person's own clock itself. Days to a civil date
/// by Howard Hinnant's algorithm, in whole numbers only (no date crate for
/// one line of output).
#[must_use]
pub fn describe_moment(unix: u64) -> Option<String> {
    if unix > LAST_DATED_SECOND {
        return None;
    }
    let (days, second) = (unix / 86_400, unix % 86_400);
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    Some(format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        second / 3600,
        second % 3600 / 60
    ))
}

/// `600` -> `10 min`, for messages and the tray.
#[must_use]
pub fn describe_every(seconds: u64) -> String {
    match seconds {
        86_400 => "1 day".to_owned(),
        s if s % 86_400 == 0 && s > 0 => format!("{} days", s / 86_400),
        s if s % 3600 == 0 && s > 0 => format!("{} h", s / 3600),
        s if s % 60 == 0 && s > 0 => format!("{} min", s / 60),
        s => format!("{s} s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Control;

    /// A wall clock for the tests: 2025-10-06, give or take.
    const NOW: u64 = 1_759_750_000;

    #[test]
    fn a_person_types_durations_and_learns_the_bounds() {
        assert_eq!(parse_every("10m"), Ok(Some(600)));
        assert_eq!(parse_every("1h"), Ok(Some(3600)));
        assert_eq!(parse_every("90"), Ok(Some(90)));
        assert_eq!(parse_every("1d"), Ok(Some(86_400)));
        assert_eq!(parse_every("AUTO"), Ok(None));
        let short = parse_every("5s").expect_err("below the floor");
        assert!(short.contains("30 s"), "{short}");
        assert!(parse_every("2d").is_err());
        assert!(parse_every("ten minutes").is_err());
        assert_eq!(describe_every(600), "10 min");
        assert_eq!(describe_every(90), "90 s");
        assert_eq!(describe_every(86_400), "1 day");
    }

    #[test]
    fn a_pause_is_asked_for_in_words_and_said_back_in_dates() {
        assert_eq!(parse_pause("1h"), Ok(3600));
        assert_eq!(parse_pause("8h"), Ok(8 * 3600));
        assert_eq!(parse_pause("1m"), Ok(60));
        assert_eq!(parse_pause("30d"), Ok(30 * 86_400));
        let short = parse_pause("30s").expect_err("below a minute");
        assert!(
            short.contains("1 min") && short.contains("30 days"),
            "{short}"
        );
        assert!(parse_pause("31d").is_err());
        assert!(parse_pause("99999999999999999999d").is_err());
        assert!(parse_pause("soon").is_err());
        assert_eq!(describe_moment(0).as_deref(), Some("1970-01-01 00:00 UTC"));
        assert_eq!(
            describe_moment(951_825_600).as_deref(),
            Some("2000-02-29 12:00 UTC")
        );
        assert_eq!(
            describe_moment(1_759_750_000).as_deref(),
            Some("2025-10-06 11:26 UTC")
        );
        assert_eq!(
            describe_moment(LAST_DATED_SECOND).as_deref(),
            Some("9999-12-31 23:59 UTC")
        );
        assert_eq!(describe_moment(u64::MAX), None);
        assert_eq!(describe_wait(1), "1 min");
        assert_eq!(describe_wait(3600), "60 min");
        assert_eq!(describe_wait(7200), "2 h");
        assert_eq!(describe_wait(7500), "2 h 5 min");
        assert_eq!(
            describe_wait(u64::MAX),
            format!("{} days", u64::MAX.div_ceil(60) / 1440)
        );
        let hour = Control {
            paused_since: Some(NOW),
            until: Some(NOW + 3600),
            ..Control::default()
        };
        assert_eq!(hour.lasts(NOW), "until 2025-10-06 12:26 UTC (in 60 min)");
        assert_eq!(Control::default().lasts(NOW), "until you resume");
    }
}
