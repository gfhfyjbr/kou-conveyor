use super::*;

#[test]
fn notices_use_go_duration_spelling_without_truncating_subseconds() {
    for (nanos, expected) in [
        (0, "0s"),
        (1, "1ns"),
        (999, "999ns"),
        (1000, "1µs"),
        (1501, "1.501µs"),
        (1_500_001, "1.500001ms"),
        (1_200_000_001, "1.200000001s"),
        (60_000_000_000, "1m0s"),
        (3_661_500_000_000, "1h1m1.5s"),
    ] {
        assert_eq!(duration_text(Duration::from_nanos(nanos)), expected);
    }
}

#[tokio::test(start_paused = true)]
async fn invalid_timer_does_not_silently_disable_a_run_budget() {
    let run = Rig::new(Config {
        max_duration: Duration::MAX,
        ..Config::default()
    });
    assert!(
        run.task
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("clock range")
    );
    assert!(run.saved.0.lock().unwrap().items.is_empty());
}
