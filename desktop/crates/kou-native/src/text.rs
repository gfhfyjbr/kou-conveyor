//! Text helpers shared by the transcript and the session list: what the Go
//! cockpits do to untrusted text before they show it, and the few time
//! conversions the session files need.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Makes untrusted text safe to show as prose: terminal escapes and control
/// characters go, and a carriage return rewrites its line as a terminal would.
pub(crate) fn clean(text: &str) -> String {
    if is_plain(text) {
        return text.to_owned();
    }
    let stripped = strip_escapes(text).replace("\r\n", "\n");
    let mut out = String::with_capacity(stripped.len());
    for (index, line) in stripped.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let line = match line.trim_end_matches('\r').rfind('\r') {
            Some(cut) => &line[cut + 1..],
            None => line,
        };
        out.extend(line.chars().filter(|&ch| ch == '\t' || !ch.is_control()));
    }
    out
}

/// Whether [`clean`] would leave the text as it is.
fn is_plain(text: &str) -> bool {
    text.chars()
        .all(|ch| ch == '\n' || ch == '\t' || !ch.is_control())
}

/// Like [`clean`] for text that arrives in pieces: a carriage return stays,
/// since what follows it may come in the next piece.
pub(crate) fn clean_stream(text: &str) -> String {
    if is_plain(text) {
        return text.to_owned();
    }
    strip_escapes(text)
        .chars()
        .filter(|&ch| matches!(ch, '\n' | '\t' | '\r') || !ch.is_control())
        .collect()
}

/// Removes terminal escape sequences: CSI, OSC and the other string
/// sequences, and two-byte escapes.
pub(crate) fn strip_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for next in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&next) {
                        break;
                    }
                }
            }
            Some(']' | 'P' | '_' | '^' | 'X') => {
                while let Some(next) = chars.next() {
                    if next == '\u{07}' {
                        break;
                    }
                    if next == '\u{1b}' {
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            // A charset selection takes one more byte.
            Some('(' | ')' | '*' | '+') => {
                chars.next();
            }
            Some(_) | None => {}
        }
    }
    out
}

/// Collapses whitespace and shortens the text to at most `limit` characters.
pub(crate) fn headline(text: &str, limit: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
            count += 1;
        }
        for ch in word.chars() {
            out.push(ch);
            count += 1;
        }
        if count > limit {
            break;
        }
    }
    if count <= limit {
        return out;
    }
    let mut cut: String = out.chars().take(limit.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// A token count the way people say it: 812k, 1.2M.
pub(crate) fn approximate_tokens(count: u64) -> String {
    match count {
        0..1000 => count.to_string(),
        1000..999_500 => format!("{}k", (count + 500) / 1000),
        _ => {
            let millions = format!("{:.1}", count as f64 / 1e6);
            format!("{}M", millions.trim_end_matches(".0"))
        }
    }
}

/// A duration in seconds as a person reads it: 0.4s, 12s, 3m 05s.
pub(crate) fn short_seconds(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return String::new();
    }
    if seconds < 10.0 {
        return format!("{seconds:.1}s");
    }
    let whole = seconds.round() as u64;
    if whole < 60 {
        return format!("{whole}s");
    }
    format!("{}m {:02}s", whole / 60, whole % 60)
}

/// Wraps text a machine produced in a code fence longer than any run of
/// backticks inside it, so markdown shows it verbatim.
pub(crate) fn fenced(text: &str, language: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for ch in text.chars() {
        if ch == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat(longest.max(2) + 1);
    let newline = if text.ends_with('\n') { "" } else { "\n" };
    format!("{fence}{language}\n{text}{newline}{fence}")
}

/// The fence language for a file, by its extension; empty when unknown.
pub(crate) fn language_of(path: &str) -> &'static str {
    let extension = path.rsplit('.').next().unwrap_or_default();
    match extension.to_ascii_lowercase().as_str() {
        "rs" => "rust",
        "go" => "go",
        "py" => "python",
        "js" | "mjs" | "cjs" => "js",
        "ts" => "ts",
        "tsx" => "tsx",
        "jsx" => "jsx",
        "json" | "jsonl" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "md" => "markdown",
        "sh" | "bash" | "zsh" => "sh",
        "c" | "h" => "c",
        "cc" | "cpp" | "hpp" => "cpp",
        "swift" => "swift",
        "java" => "java",
        "kt" => "kotlin",
        "html" => "html",
        "css" => "css",
        "sql" => "sql",
        _ => "",
    }
}

/// Parses an RFC 3339 timestamp as Go writes it (`2026-10-25T22:20:00.05452Z`,
/// or with a numeric offset).
pub(crate) fn parse_time(text: &str) -> Option<SystemTime> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let number = |from: usize, to: usize| text.get(from..to)?.parse::<i64>().ok();
    let (year, month, day) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
    let (hour, minute, second) = (number(11, 13)?, number(14, 16)?, number(17, 19)?);
    let mut rest = &text[19..];
    let mut nanos = 0u32;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        let mut value = 0u32;
        for index in 0..9 {
            let digit = fraction.as_bytes().get(index).filter(|_| index < digits);
            value = value * 10 + digit.map_or(0, |byte| u32::from(byte - b'0'));
        }
        nanos = value;
        rest = &fraction[digits..];
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let hours: i64 = rest.get(1..3)?.parse().ok()?;
            let minutes: i64 = rest.get(4..6)?.parse().ok()?;
            sign * (hours * 3600 + minutes * 60)
        }
    };
    // Days from the civil date (Howard Hinnant's algorithm).
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset;
    let seconds = u64::try_from(seconds).ok()?;
    Some(UNIX_EPOCH + Duration::new(seconds, nanos))
}

/// Formats a time as RFC 3339 in UTC with microseconds, as Go's cockpits do.
pub(crate) fn format_time(time: SystemTime) -> String {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let days = (since.as_secs() / 86_400) as i64;
    let of_day = since.as_secs() % 86_400;
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:06}Z",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60,
        since.subsec_micros()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_drops_escapes_and_resolves_carriage_returns() {
        assert_eq!(clean("plain\ttext\n"), "plain\ttext\n");
        assert_eq!(clean("\u{1b}[32mgreen\u{1b}[0m"), "green");
        assert_eq!(clean("10%\r50%\r100%\nnext\r\n"), "100%\nnext\n");
        assert_eq!(clean("a\u{1b}]0;title\u{07}b\u{0}c"), "abc");
        assert_eq!(clean_stream("a\rb\u{1b}[1m"), "a\rb");
    }

    #[test]
    fn headline_collapses_and_cuts() {
        assert_eq!(headline("  a\n b\t c ", 80), "a b c");
        assert_eq!(headline("abcdef", 4), "abc…");
        assert_eq!(headline("привет мир", 6), "приве…");
    }

    #[test]
    fn tokens_and_durations_read_as_people_say_them() {
        assert_eq!(approximate_tokens(812), "812");
        assert_eq!(approximate_tokens(811_600), "812k");
        assert_eq!(approximate_tokens(1_200_000), "1.2M");
        assert_eq!(approximate_tokens(2_000_000), "2M");
        assert_eq!(short_seconds(0.42), "0.4s");
        assert_eq!(short_seconds(185.0), "3m 05s");
    }

    #[test]
    fn a_fence_outgrows_the_backticks_inside() {
        assert_eq!(fenced("a", "rust"), "```rust\na\n```");
        assert_eq!(fenced("```\nx\n```\n", ""), "````\n```\nx\n```\n````");
    }

    #[test]
    fn times_round_trip_through_go_s_format() {
        let time = parse_time("2026-10-25T22:20:00.05452Z").expect("a time");
        assert_eq!(format_time(time), "2026-10-25T22:20:00.054520Z");
        assert_eq!(
            parse_time("1970-01-01T00:00:01Z"),
            Some(UNIX_EPOCH + Duration::from_secs(1))
        );
        assert_eq!(
            parse_time("2026-03-01T03:00:00+03:00"),
            parse_time("2026-03-01T00:00:00Z")
        );
        assert_eq!(parse_time("yesterday"), None);
    }
}
