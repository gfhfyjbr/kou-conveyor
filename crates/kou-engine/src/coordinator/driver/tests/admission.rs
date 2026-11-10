use super::*;

#[tokio::test]
async fn deferred_admission_cannot_starve_controls_or_operation_updates() {
    let (input, updates, mut channels) = channels(QueueLimits {
        events: 1,
        bytes: 1 << 20,
        event_bytes: 1 << 19,
    })
    .unwrap();
    input
        .submit(external("one", "pending", "after_tools"))
        .await
        .unwrap();
    let first = channels.inputs.recv().await.unwrap();
    let send = input.clone();
    let blocked =
        tokio::spawn(async move { send.submit(external("two", "pending", "after_tools")).await });
    tokio::task::yield_now().await;
    assert!(!blocked.is_finished());
    input.submit(control("hard")).await.unwrap();
    updates.submit(spec().operation("op".into())).await.unwrap();
    assert_eq!(channels.inputs.recv().await.unwrap().value.kind, "control");
    assert_eq!(channels.operations.recv().await.unwrap().value.id, "op");
    drop(first);
    tokio::time::timeout(Duration::from_secs(1), blocked)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn dropping_the_driver_channels_wakes_blocked_producers() {
    let (input, _, channels) = channels(QueueLimits {
        events: 1,
        ..QueueLimits::default()
    })
    .unwrap();
    input
        .submit(external("one", "pending", "after_tools"))
        .await
        .unwrap();
    let send = input.clone();
    let blocked =
        tokio::spawn(async move { send.submit(external("two", "pending", "after_tools")).await });
    tokio::task::yield_now().await;
    assert!(!blocked.is_finished());
    drop(channels);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), blocked)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(input.submit(control("hard")).await.is_err());
}

#[tokio::test]
async fn canceling_byte_admission_releases_its_count_permit() {
    let (input, _, mut channels) = channels(QueueLimits {
        events: 2,
        bytes: 1024,
        event_bytes: 1024,
    })
    .unwrap();
    input
        .submit(external("one", &"x".repeat(600), ""))
        .await
        .unwrap();
    let send = input.clone();
    let blocked =
        tokio::spawn(async move { send.submit(external("two", &"x".repeat(600), "")).await });
    tokio::task::yield_now().await;
    assert!(!blocked.is_finished());
    blocked.abort();
    assert!(blocked.await.unwrap_err().is_cancelled());
    tokio::time::timeout(
        Duration::from_secs(1),
        input.submit(external("three", "small", "")),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(channels.inputs.recv().await.unwrap().value.id, "one");
    assert_eq!(channels.inputs.recv().await.unwrap().value.id, "three");
}

#[tokio::test]
async fn committed_admission_keeps_the_lease_until_processing_finishes() {
    let (_, handle, mut channels) = channels(QueueLimits {
        events: 1,
        ..QueueLimits::default()
    })
    .unwrap();
    let sender = handle.clone();
    let first = tokio::spawn(async move {
        sender
            .submit_committed(spec().operation("one".into()))
            .await
    });
    let mut queued = channels.operations.recv().await.unwrap();
    assert!(!first.is_finished());
    let second = tokio::spawn(async move { handle.submit(spec().operation("two".into())).await });
    tokio::task::yield_now().await;
    assert!(!second.is_finished());
    queued.committed.take().unwrap().send(Ok(())).unwrap();
    first.await.unwrap().unwrap();
    assert!(
        !second.is_finished(),
        "in-process update lost its admission lease"
    );
    drop(queued);
    second.await.unwrap().unwrap();
    assert_eq!(channels.operations.recv().await.unwrap().value.id, "two");
}

#[tokio::test]
async fn discarded_committed_update_and_closed_admission_never_report_success() {
    for receive_first in [false, true] {
        let (_, handle, mut channels) = channels(QueueLimits::default()).unwrap();
        let sender = handle.clone();
        let task =
            tokio::spawn(
                async move { sender.submit_committed(spec().operation("op".into())).await },
            );
        if receive_first {
            drop(channels.operations.recv().await.unwrap());
        } else {
            tokio::task::yield_now().await;
            drop(channels);
        }
        assert!(task.await.unwrap().is_err());
        if !receive_first {
            assert!(
                handle
                    .submit_committed(spec().operation("next".into()))
                    .await
                    .is_err()
            );
        }
    }
}

#[tokio::test]
async fn queue_limits_reject_invalid_config_and_oversized_events() {
    for limits in [
        QueueLimits {
            events: 0,
            ..QueueLimits::default()
        },
        QueueLimits {
            bytes: 0,
            ..QueueLimits::default()
        },
        QueueLimits {
            event_bytes: 2,
            bytes: 1,
            ..QueueLimits::default()
        },
    ] {
        assert!(channels(limits).is_err());
    }
    let (input, updates, _channels) = channels(QueueLimits {
        event_bytes: 1,
        ..QueueLimits::default()
    })
    .unwrap();
    assert!(
        input
            .submit(external("one", "large", ""))
            .await
            .unwrap_err()
            .to_string()
            .contains("byte limit")
    );
    assert!(
        updates
            .submit(spec().operation("op".into()))
            .await
            .unwrap_err()
            .to_string()
            .contains("byte limit")
    );
}
