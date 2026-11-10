use super::*;

fn part(path: &str) -> FileState {
    FileState {
        action: "read".into(),
        path: path.into(),
        ..FileState::default()
    }
}
#[test]
fn journal_retains_only_keys_and_digests_not_read_bodies() {
    let journal = Journal::new(ReadJournalLimits::default());
    let part = part("/inert/read");
    let text = "x".repeat(2 << 20);
    assert_eq!(journal.count("one", &part, &text), 1);
    assert_eq!(journal.count("two", &part, &text), 2);
    assert_eq!(journal.count("three", &part, &text), 3);
    let state = journal.state.lock().unwrap();
    assert_eq!(state.parts.len(), 1);
    assert_eq!(state.calls.len(), 3);
    assert_eq!(
        state.key_bytes,
        part.path.len() + "one".len() + "two".len() + "three".len()
    );
    assert_eq!(state.order.len(), state.parts.len() + state.calls.len());
}
#[test]
fn journal_enforces_cardinality_and_key_bytes_and_skips_oversized_or_disabled_entries() {
    let limits = ReadJournalLimits {
        parts: 2,
        calls: 3,
        key_bytes: 40,
    };
    let journal = Journal::new(limits);
    for index in 0..100 {
        journal.count(
            &format!("call-{index}"),
            &part(&format!("/inert/{index}")),
            "same",
        );
        let state = journal.state.lock().unwrap();
        assert!(state.parts.len() <= limits.parts);
        assert!(state.calls.len() <= limits.calls);
        assert!(state.key_bytes <= limits.key_bytes);
        assert_eq!(state.order.len(), state.parts.len() + state.calls.len());
    }
    for limits in [
        ReadJournalLimits {
            parts: 0,
            calls: 3,
            key_bytes: 40,
        },
        ReadJournalLimits {
            parts: 2,
            calls: 0,
            key_bytes: 40,
        },
        ReadJournalLimits {
            parts: 2,
            calls: 3,
            key_bytes: 0,
        },
        ReadJournalLimits {
            parts: 2,
            calls: 3,
            key_bytes: 4,
        },
    ] {
        let journal = Journal::new(limits);
        assert_eq!(journal.count("call", &part("/too-long"), "same"), 1);
        assert_eq!(journal.count("call", &part("/too-long"), "same"), 1);
        let state = journal.state.lock().unwrap();
        assert!(state.parts.is_empty() && state.calls.is_empty() && state.order.is_empty());
        assert_eq!(state.key_bytes, 0);
    }
}
#[test]
fn journal_eviction_forgets_old_parts_and_ids_but_does_not_increment_cached_calls() {
    let journal = Journal::new(ReadJournalLimits {
        parts: 1,
        calls: 1,
        key_bytes: 1024,
    });
    let first = part("/a");
    assert_eq!(journal.count("one", &first, "same"), 1);
    assert_eq!(journal.count("two", &first, "same"), 2);
    assert_eq!(journal.count("three", &first, "same"), 3);
    assert_eq!(journal.count("three", &part("/other"), "changed"), 3);
    assert_eq!(journal.count("four", &part("/b"), "same"), 1);
    assert_eq!(journal.count("three", &first, "same"), 1);
}
#[test]
fn concurrent_read_formatting_is_linearizable_and_repeat_ids_are_idempotent() {
    let journal = Arc::new(Journal::new(ReadJournalLimits::default()));
    let part = Arc::new(part("/inert/read"));
    let mut workers = Vec::new();
    for index in 0..32 {
        let journal = journal.clone();
        let part = part.clone();
        workers.push(std::thread::spawn(move || {
            let id = format!("call-{index}");
            let count = journal.count(&id, &part, "same");
            assert_eq!(journal.count(&id, &part, "same"), count);
            count
        }));
    }
    let mut counts: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    counts.sort_unstable();
    assert_eq!(counts, (1..=32).collect::<Vec<_>>());
    let state = journal.state.lock().unwrap();
    assert_eq!((state.parts.len(), state.calls.len()), (1, 32));
}
