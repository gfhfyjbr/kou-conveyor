//! Times as the services write them, and how long until one.

use std::time::{Duration, SystemTime};

/// Days from 1970-01-01 to a date of the Gregorian calendar.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// A time written as ISO 8601 (`2026-11-18T00:00:01.000905+00:00`, with `Z`,
/// with or without a fraction, without an offset meaning UTC). `None` for
/// anything else.
pub fn parse_time(text: &str) -> Option<SystemTime> {
    let text = text.trim();
    let bytes = text.as_bytes();
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = text.get(range)?;
        (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| part.parse().ok())
            .flatten()
    };
    if bytes.len() < 19
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't' | b' ')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    // A fraction of a second, to the nanosecond; then the offset.
    let mut at = 19;
    let mut nanos = 0u32;
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        let from = at;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        let digits = &text[from..at];
        if digits.is_empty() {
            return None;
        }
        let padded: String = digits
            .chars()
            .chain(std::iter::repeat('0'))
            .take(9)
            .collect();
        nanos = padded.parse().ok()?;
    }
    let offset = match &text[at..] {
        "" | "Z" | "z" => 0,
        rest => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let rest = &rest[1..];
            let (hours, minutes) = match rest.len() {
                5 if rest.as_bytes()[2] == b':' => (rest.get(0..2)?, rest.get(3..5)?),
                4 => (rest.get(0..2)?, rest.get(2..4)?),
                2 => (rest, "00"),
                _ => return None,
            };
            let digits = |part: &str| -> Option<i64> {
                part.bytes()
                    .all(|byte| byte.is_ascii_digit())
                    .then(|| part.parse().ok())
                    .flatten()
            };
            sign * (digits(hours)? * 3600 + digits(minutes)? * 60)
        }
    };
    let seconds =
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset;
    let since = Duration::new(u64::try_from(seconds).ok()?, nanos);
    SystemTime::UNIX_EPOCH.checked_add(since)
}

/// A time given as seconds since 1970.
pub(crate) fn from_epoch(seconds: i64) -> Option<SystemTime> {
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(u64::try_from(seconds).ok()?))
}

/// How long from `now` until `then`, in the two largest units that say
/// something: `12m`, `3h 12m`, `4d 8h`; `now` once it has come.
pub fn until(then: SystemTime, now: SystemTime) -> String {
    let Ok(left) = then.duration_since(now) else {
        return "now".to_owned();
    };
    // Rounded up to the minute: "0m" is never what is left.
    let minutes = left.as_secs().div_ceil(60);
    if minutes == 0 {
        return "now".to_owned();
    }
    let (days, hours, minutes) = (minutes / 1440, minutes % 1440 / 60, minutes % 60);
    match (days, hours, minutes) {
        (0, 0, minutes) => format!("{minutes}m"),
        (0, hours, 0) => format!("{hours}h"),
        (0, hours, minutes) => format!("{hours}h {minutes}m"),
        (days, 0, _) => format!("{days}d"),
        (days, hours, _) => format!("{days}d {hours}h"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn epoch(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn times_are_read_as_the_services_write_them() {
        // 2026-11-18T00:00:01Z is 1 794 960 001 seconds after 1970.
        let expected = epoch(1_794_960_001) + Duration::from_micros(905);
        assert_eq!(
            parse_time("2026-11-18T00:00:01.000905+00:00"),
            Some(expected)
        );
        assert_eq!(
            parse_time("2026-11-18T00:00:01Z"),
            Some(epoch(1_794_960_001))
        );
        assert_eq!(
            parse_time("2026-11-18 00:00:01"),
            Some(epoch(1_794_960_001))
        );
        // An offset is taken off: three in the morning in Moscow is midnight UTC.
        assert_eq!(
            parse_time("2026-11-18T03:00:01+03:00"),
            Some(epoch(1_794_960_001))
        );
        assert_eq!(
            parse_time("2026-11-17T19:00:01-0500"),
            Some(epoch(1_794_960_001))
        );
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(epoch(0)));
        assert_eq!(parse_time("2000-02-29T12:00:00Z"), Some(epoch(951_825_600)));
        for not_a_time in [
            "",
            "tomorrow",
            "2026-11-18",
            "2026-13-01T00:00:00Z",
            "2026-11-18T24:00:00Z",
            "2026-11-18T00:00:01.Z",
            "2026-11-18T00:00:01+3",
            "2026-11-18T00:00:01 UTC",
            "1969-12-31T23:59:59Z",
            "２０２６-11-18T00:00:01Z",
        ] {
            assert_eq!(parse_time(not_a_time), None, "{not_a_time:?}");
        }
        assert_eq!(from_epoch(1_794_960_001), Some(epoch(1_794_960_001)));
        assert_eq!(from_epoch(-1), None);
    }

    #[test]
    fn what_is_left_is_said_in_its_two_largest_units() {
        let now = epoch(1_000_000);
        let after = |seconds: u64| until(now + Duration::from_secs(seconds), now);
        assert_eq!(after(0), "now");
        assert_eq!(until(epoch(10), now), "now");
        assert_eq!(after(20), "1m");
        assert_eq!(after(59 * 60), "59m");
        assert_eq!(after(3600), "1h");
        assert_eq!(after(3 * 3600 + 12 * 60), "3h 12m");
        assert_eq!(after(24 * 3600), "1d");
        assert_eq!(after(4 * 86_400 + 8 * 3600 + 30 * 60), "4d 8h");
    }
}
