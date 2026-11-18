//! Prompt history shared with kou-conveyor's other cockpits: the file the
//! terminal cockpit keeps in `<project>/.harness/ui-history.json`, read and
//! written the way it does, so a prompt sent in any of them comes back with
//! Up in the others.

use serde::{Deserialize, Serialize};
use std::{
    io::Write as _,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// How many prompts the file keeps.
const LIMIT: usize = 200;

/// One prompt, with the field names of the Go cockpits' JSON.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Entry {
    #[serde(rename = "Prompt")]
    pub prompt: String,
    #[serde(rename = "SessionID", default)]
    pub session: String,
    #[serde(rename = "Title", default)]
    pub title: String,
    #[serde(rename = "Model", default)]
    pub model: String,
    /// RFC 3339 in UTC, to the nanosecond, as Go writes a time.
    #[serde(rename = "CreatedAt", default)]
    pub created_at: String,
}

impl Entry {
    /// A prompt sent now.
    pub(crate) fn now(prompt: &str, session: &str, model: &str) -> Self {
        Self {
            prompt: prompt.to_owned(),
            session: session.to_owned(),
            title: title(prompt),
            model: model.to_owned(),
            created_at: format_time(SystemTime::now()),
        }
    }

    /// When it was sent, as nanoseconds since 1970; 0 when the file says nothing usable.
    pub(crate) fn at(&self) -> i128 {
        parse_time(&self.created_at).unwrap_or(0)
    }
}

/// Where a project keeps its prompt history.
pub(crate) fn path(project: &Path) -> PathBuf {
    project.join(".harness").join("ui-history.json")
}

/// The prompts in the file, oldest first; none when there is no file or it
/// cannot be read. Nothing in it may reach the screen as a control character.
pub(crate) fn load(path: &Path) -> Vec<Entry> {
    let Ok(data) = std::fs::read(path) else {
        return Vec::new();
    };
    let Ok(mut entries) = serde_json::from_slice::<Vec<Entry>>(&data) else {
        return Vec::new();
    };
    for entry in &mut entries {
        entry.prompt = clean(&entry.prompt);
        entry.title = clean(&entry.title);
        entry.model = clean(&entry.model);
        entry.session = clean(&entry.session);
    }
    entries
}

/// Merges `entries` with what other cockpits saved since the file was read,
/// writes the result atomically and returns it, so that two cockpits never
/// erase each other's prompts.
pub(crate) fn save(path: &Path, entries: &[Entry]) -> std::io::Result<Vec<Entry>> {
    let merged = merge(load(path), entries);
    let directory = path
        .parent()
        .ok_or_else(|| std::io::Error::other("the history file has no folder"))?;
    std::fs::create_dir_all(directory)?;
    let mut data = serde_json::to_vec_pretty(&merged).map_err(std::io::Error::other)?;
    data.push(b'\n');
    let temporary = directory.join(format!(".ui-history-{}.tmp", std::process::id()));
    let written = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(&data)?;
        file.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written.map(|()| merged)
}

/// The union of two lists, each prompt once by its time and text, oldest
/// first, the newest [`LIMIT`] of them.
pub(crate) fn merge(on_disk: Vec<Entry>, ours: &[Entry]) -> Vec<Entry> {
    let mut seen = std::collections::HashSet::new();
    let mut merged: Vec<Entry> = Vec::new();
    for entry in on_disk.into_iter().chain(ours.iter().cloned()) {
        if seen.insert((entry.at(), entry.prompt.clone())) {
            merged.push(entry);
        }
    }
    merged.sort_by_key(Entry::at);
    if merged.len() > LIMIT {
        merged.drain(..merged.len() - LIMIT);
    }
    merged
}

/// Records a prompt unless it repeats the one before. Returns whether it did.
pub(crate) fn append(entries: &mut Vec<Entry>, entry: Entry) -> bool {
    if entries
        .last()
        .is_some_and(|last| last.prompt == entry.prompt)
    {
        return false;
    }
    entries.push(entry);
    true
}

/// A prompt's title: its words on one line, at most 72 characters.
fn title(prompt: &str) -> String {
    let words = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if words.chars().count() > 72 {
        let mut short: String = words.chars().take(71).collect();
        short.push('…');
        short
    } else {
        words
    }
}

/// Text without control characters other than line breaks and tabs.
fn clean(text: &str) -> String {
    text.chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
        .collect()
}

/// A time as Go's RFC 3339 with nanoseconds: `2026-11-11T17:20:01.5Z`.
pub(crate) fn format_time(time: SystemTime) -> String {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    let seconds = since.as_secs() as i64;
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let (year, month, day) = civil(days);
    let mut text = format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    );
    let nanos = since.subsec_nanos();
    if nanos > 0 {
        let digits = format!("{nanos:09}");
        text.push('.');
        text.push_str(digits.trim_end_matches('0'));
    }
    text.push('Z');
    text
}

/// Nanoseconds since 1970 of an RFC 3339 time, with or without a fraction
/// and in any offset.
pub(crate) fn parse_time(text: &str) -> Option<i128> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || !matches!(bytes[10], b'T' | b't')
    {
        return None;
    }
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    let mut at = 19;
    let mut nanos: i128 = 0;
    if bytes.get(at) == Some(&b'.') {
        let digits = text[at + 1..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        let fraction = &text[at + 1..at + 1 + digits.min(9)];
        nanos = format!("{fraction:0<9}").parse().ok()?;
        at += 1 + digits;
    }
    let offset = match bytes.get(at)? {
        b'Z' | b'z' => 0,
        sign @ (b'+' | b'-') => {
            let hours = number(at + 1..at + 3)?;
            let minutes = number(at + 4..at + 6)?;
            let minutes = hours * 60 + minutes;
            if *sign == b'+' { minutes } else { -minutes }
        }
        _ => return None,
    };
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset * 60;
    Some(seconds as i128 * 1_000_000_000 + nanos)
}

/// The date of a day counted from 1970-01-01 (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// How long ago a time was, shortly: `just now`, `4m ago`, `3h ago`, `2d ago`.
pub(crate) fn ago(at: i128, now: SystemTime) -> String {
    let now = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos() as i128);
    let seconds = ((now - at) / 1_000_000_000).max(0);
    match seconds {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{}m ago", seconds / 60),
        3600..86_400 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_round_trip_as_go_writes_them() {
        let time = UNIX_EPOCH + Duration::new(1_794_417_601, 500_000_000);
        let text = format_time(time);
        assert_eq!(text, "2026-11-11T17:20:01.5Z");
        assert_eq!(
            parse_time(&text),
            Some(1_794_417_601_500_000_000),
            "parsed back"
        );
        assert_eq!(
            parse_time("2026-11-11T19:20:01.5+02:00"),
            parse_time(&text),
            "an offset is taken into account"
        );
        assert_eq!(
            parse_time("2026-11-11T17:20:01Z"),
            Some(1_794_417_601_000_000_000)
        );
        assert_eq!(parse_time("yesterday"), None);
        assert_eq!(format_time(UNIX_EPOCH), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn the_file_is_shared_merged_and_kept_short() {
        let folder = tempfile::tempdir().expect("a directory");
        let file = path(folder.path());
        // Another cockpit wrote first; its fields are Go's.
        std::fs::create_dir_all(file.parent().expect("a folder")).expect("a folder");
        std::fs::write(
            &file,
            r#"[{"Prompt":"from the terminal\u001b[31m","SessionID":"s1","Title":"from the terminal","Model":"m","CreatedAt":"2026-11-11T10:00:00.25Z"}]"#,
        )
        .expect("a file");
        let mut ours = load(&file);
        assert_eq!(
            ours[0].prompt, "from the terminal[31m",
            "no escape reaches the screen"
        );
        let mut entry = Entry::now("from the desktop", "s2", "");
        entry.created_at = "2026-11-11T11:00:00Z".into();
        assert!(append(&mut ours, entry.clone()));
        assert!(!append(&mut ours, entry), "a repeat is not recorded");

        // Meanwhile the terminal recorded another prompt.
        let mut theirs = load(&file);
        let mut late = Entry::now("later in the terminal", "s1", "");
        late.created_at = "2026-11-11T12:00:00Z".into();
        theirs.push(late);
        save(&file, &theirs).expect("saved");

        let merged = save(&file, &ours).expect("saved");
        let prompts: Vec<&str> = merged.iter().map(|entry| entry.prompt.as_str()).collect();
        assert_eq!(
            prompts,
            [
                "from the terminal[31m",
                "from the desktop",
                "later in the terminal"
            ]
        );
        let written = std::fs::read_to_string(&file).expect("the file");
        assert!(written.contains(r#""SessionID": "s2""#), "{written}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&file)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        let many: Vec<Entry> = (0..250)
            .map(|n| Entry {
                prompt: format!("p{n}"),
                created_at: format_time(UNIX_EPOCH + Duration::from_secs(n)),
                ..Entry::default()
            })
            .collect();
        let kept = merge(Vec::new(), &many);
        assert_eq!(kept.len(), LIMIT);
        assert_eq!(kept[0].prompt, "p50", "the oldest go");
    }

    #[test]
    fn titles_and_ages_are_short() {
        assert_eq!(title("  look   at\nthis  "), "look at this");
        assert_eq!(title(&"x".repeat(80)).chars().count(), 72);
        let now = UNIX_EPOCH + Duration::from_secs(10_000);
        let at = |seconds: u64| (seconds as i128) * 1_000_000_000;
        assert_eq!(ago(at(9_990), now), "just now");
        assert_eq!(ago(at(9_700), now), "5m ago");
        assert_eq!(ago(at(2_000), now), "2h ago");
    }
}
