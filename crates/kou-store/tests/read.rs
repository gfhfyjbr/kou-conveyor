use kou_domain::{ItemData, Record};
use kou_store::{Error, SessionLog, open_session};
use std::{fs, io::Write, path::PathBuf};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../harness/sessionstore/localfile/testdata")
        .join(format!("{name}.session.jsonl"))
}

#[test]
fn reads_go_golden_and_keeps_original_status_snapshot() {
    let log = SessionLog::open(fixture("golden-session")).unwrap();
    assert_eq!(log.item_count(), 4);
    let item = log.item(4).unwrap();
    let ItemData::ToolCallStatus(status) = item.data else {
        panic!()
    };
    assert_eq!(status.operations[0].status, "ready");
    assert_eq!(status.operations[0].state.as_str(), Some(r#"{"step":1}"#));
    let resume = log.resume_full().unwrap();
    assert_eq!(resume.operations[0].status, "awaiting");
    assert_eq!(resume.operations[0].state.as_str(), Some(r#"{"step":2}"#));
    assert_eq!(resume.external_input_ids, ["input-1"]);
    assert!(!log.items(0, 4).unwrap().more);
    assert!(log.items(0, 1).unwrap().more);
    assert_eq!(log.items(u64::MAX, 1).unwrap().next_after, u64::MAX);
    assert!(log.items(0, 0).is_err());
}

#[test]
fn reads_go_fork_and_rejects_legacy() {
    let log = SessionLog::open(fixture("golden-fork")).unwrap();
    assert_eq!(log.item_count(), 8);
    assert_eq!(
        log.resume_full().unwrap().external_input_ids,
        ["child-input"]
    );
    assert!(SessionLog::open(fixture("legacy-session")).is_err());
    assert!(SessionLog::open(fixture("soft-stop-session")).is_err());
}

#[test]
fn torn_tail_is_ignored_and_committed_copy_is_verbatim() {
    let original = fs::read(fixture("golden-session")).unwrap();
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&original).unwrap();
    file.write_all(b"{broken final record").unwrap();
    let log = SessionLog::open(file.path()).unwrap();
    assert_eq!(log.committed_bytes(), original.len() as u64);
    assert_eq!(log.summary().ignored_tail_bytes, 20);
    let mut copied = Vec::new();
    log.copy_committed(&mut copied).unwrap();
    assert_eq!(copied, original);
}

#[test]
fn newline_commits_corrupt_record_as_an_error() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&fs::read(fixture("golden-session")).unwrap())
        .unwrap();
    file.write_all(b"{bad}\n").unwrap();
    assert!(SessionLog::open(file.path()).is_err());
}

#[test]
fn rejects_append_truncate_and_atomic_replacement_after_indexing() {
    for action in 0..3 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        fs::copy(fixture("golden-session"), &path).unwrap();
        let log = SessionLog::open(&path).unwrap();
        match action {
            0 => fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(b" \n")
                .unwrap(),
            1 => fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_len(1)
                .unwrap(),
            _ => {
                let replacement = dir.path().join("replacement");
                fs::copy(fixture("golden-session"), &replacement).unwrap();
                fs::rename(replacement, &path).unwrap();
            }
        }
        assert!(matches!(log.item(1), Err(Error::Changed)));
    }
}

#[test]
fn record_roundtrip_retains_opaque_reasoning_and_usage() {
    let text = fs::read_to_string(fixture("golden-session")).unwrap();
    for line in text.lines() {
        let record = Record::decode(line.as_bytes()).unwrap();
        let roundtrip = Record::decode(&record.encode_line().unwrap()).unwrap();
        assert_eq!(roundtrip, record);
    }
}

#[test]
fn limits_committed_records_but_ignores_oversized_torn_tail() {
    let original = fs::read(fixture("golden-session")).unwrap();
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&original).unwrap();
    let max = original.len();
    file.write_all(&vec![b'x'; max * 3]).unwrap();
    assert!(SessionLog::open_with_record_limit(file.path(), max).is_ok());
    file.write_all(b"\n").unwrap();
    assert!(matches!(
        SessionLog::open_with_record_limit(file.path(), max),
        Err(Error::RecordLimit(_))
    ));
}

#[test]
fn validates_session_id_before_opening_any_path() {
    for id in [
        "",
        "../golden-session",
        "golden_session",
        "a/b",
        "a\\b",
        "é",
    ] {
        assert!(open_session(fixture("golden-session").parent().unwrap(), id).is_err());
    }
    assert!(
        open_session(
            fixture("golden-session").parent().unwrap(),
            "golden-session"
        )
        .is_ok()
    );
}
