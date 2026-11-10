use chrono::DateTime;
use serde::Deserialize;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

#[derive(Default, Deserialize)]
#[serde(default)]
struct Record {
    #[serde(rename = "type")]
    kind: String,
    data: Data,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Data {
    #[serde(rename = "Item")]
    item: Item,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Item {
    #[serde(rename = "RecordedAt")]
    at: String,
    #[serde(rename = "Kind")]
    kind: String,
    #[serde(rename = "Data")]
    data: Input,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Input {
    #[serde(rename = "Kind")]
    kind: String,
}

pub const ZERO_TIME: (i64, u32) = (-62_135_596_800, 0);

fn consider(line: &[u8], last: &mut (i64, u32)) {
    if memchr::memmem::find(line, b"\"Kind\":\"input\"").is_none() {
        return;
    }
    let Ok(record) = serde_json::from_slice::<Record>(line) else {
        return;
    };
    if record.kind != "item"
        || record.data.item.kind != "input"
        || record.data.item.data.kind != "external"
    {
        return;
    }
    if let Ok(at) = DateTime::parse_from_rfc3339(&record.data.item.at) {
        let candidate = (at.timestamp(), at.timestamp_subsec_nanos());
        if candidate > *last {
            *last = candidate;
        }
    }
}

pub fn run(path: &Path, reuse: bool) -> std::io::Result<(i64, u32)> {
    let mut reader = BufReader::with_capacity(64 << 10, File::open(path)?);
    let mut line = Vec::new();
    let mut last = ZERO_TIME;
    loop {
        if reuse {
            line.clear();
        } else {
            line = Vec::new();
        }
        let n = reader.read_until(b'\n', &mut line)?;
        if n == 0 || line.last() != Some(&b'\n') {
            break;
        }
        consider(&line, &mut last);
    }
    Ok(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_compared_in_utc() {
        let mut last = ZERO_TIME;
        consider(br#"{"type":"item","data":{"Item":{"Kind":"input","RecordedAt":"2026-11-09T12:00:00+03:00","Data":{"Kind":"external"}}}}"#, &mut last);
        let first = last;
        consider(br#"{"type":"item","data":{"Item":{"Kind":"input","RecordedAt":"2026-11-09T09:00:00.000000001Z","Data":{"Kind":"external"}}}}"#, &mut last);
        assert_eq!(last.0, first.0);
        assert_eq!(last.1, 1);
    }
}
