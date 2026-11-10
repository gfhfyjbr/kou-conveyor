use super::*;

#[tokio::test(start_paused = true)]
async fn no_dispatch_crosses_a_failed_durable_boundary() {
    for failure in ["turn", "response", "status", "dispatch"] {
        let mut run = Rig::new(Config::default());
        run.saved.0.lock().unwrap().fail = Some(failure);
        run.send("one", "work").await;
        if failure != "turn" {
            reply(run.request().await, calls(&[("call", "Bash")]));
        }
        let error = run.task.await.unwrap().unwrap_err().to_string();
        let saved = run.saved.0.lock().unwrap();
        match failure {
            "turn" => {
                let quoted = error
                    .strip_prefix("store turn ")
                    .unwrap()
                    .strip_suffix(": turn failed")
                    .unwrap();
                let id: String = serde_json::from_str(quoted).unwrap();
                assert!(uuid::Uuid::parse_str(&id).is_ok());
                assert_eq!(error, format!("store turn {id:?}: turn failed"));
            }
            "response" => {
                let id = saved
                    .items
                    .iter()
                    .find_map(|item| match item {
                        ItemData::Turn(turn) => Some(&turn.id),
                        _ => None,
                    })
                    .unwrap();
                assert_eq!(
                    error,
                    format!("store turn {id:?} response: response failed")
                );
            }
            "status" => assert_eq!(error, "store tool call \"call\" status: status failed"),
            "dispatch" => assert_eq!(error, "dispatch failed"),
            _ => unreachable!(),
        }
        assert_eq!(saved.adds.len(), usize::from(failure == "dispatch"));
        assert_eq!(
            saved
                .items
                .iter()
                .filter(|item| matches!(item, ItemData::ToolCallStatus(_)))
                .count(),
            usize::from(failure == "dispatch")
        );
    }
}

#[tokio::test(start_paused = true)]
async fn failed_input_keeps_the_input_id_in_the_durable_error() {
    let run = Rig::new(Config::default());
    run.saved.0.lock().unwrap().fail = Some("input");
    run.send("one", "work").await;
    assert_eq!(
        run.task.await.unwrap().unwrap_err().to_string(),
        "store input \"one\": input failed"
    );
    assert!(run.saved.0.lock().unwrap().items.is_empty());
}

#[tokio::test(start_paused = true)]
async fn result_translation_failure_never_dispatches() {
    let mut run = Rig::new(Config::default());
    run.send("one", "work").await;
    reply(run.request().await, calls(&[("call", "Reject")]));
    assert!(
        run.task
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("result failed")
    );
    let saved = run.saved.0.lock().unwrap();
    assert!(saved.adds.is_empty());
    assert!(
        !saved
            .items
            .iter()
            .any(|item| matches!(item, ItemData::ToolCallStatus(_)))
    );
}

#[tokio::test(start_paused = true)]
async fn input_ids_are_deduplicated_without_canceling_the_request() {
    let mut run = Rig::new(Config::default());
    run.send("same", "first").await;
    let first = run.request().await;
    run.send("same", "duplicate").await;
    settle().await;
    assert!(!first.cancellation.is_cancelled());
    assert!(run.requests.try_recv().is_err());
    assert_eq!(run.count("input"), 1);
    run.abort().await;
    assert!(first.cancellation.is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn immediate_and_unavailable_calls_request_an_answer_without_dispatch() {
    for name in ["Immediate", "Missing"] {
        let mut run = Rig::new(Config::default());
        run.send("one", "work").await;
        reply(run.request().await, calls(&[("call", name)]));
        let next = run.request().await;
        let output = result(&next.request, "call").unwrap();
        assert!(if name == "Missing" {
            output.contains("not available")
        } else {
            output == "immediate"
        });
        assert!(run.saved.0.lock().unwrap().adds.is_empty());
        run.abort().await;
    }
}

#[tokio::test(start_paused = true)]
async fn hard_stop_cancels_all_operations_and_waits_for_terminal_updates() {
    let mut run = Rig::new(Config::default());
    run.send("one", "work").await;
    reply(run.request().await, calls(&[("a", "Bash"), ("b", "Bash")]));
    settle().await;
    run.control("hard").await;
    settle().await;
    assert_eq!(run.saved.0.lock().unwrap().cancels.len(), 2);
    assert!(!run.task.is_finished());
    run.complete(0, "canceling").await;
    run.complete(1, "canceled").await;
    settle().await;
    assert!(!run.task.is_finished());
    run.send("late", "do not start another model").await;
    settle().await;
    assert!(run.requests.try_recv().is_err());
    run.complete(0, "canceled").await;
    run.task.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn hard_stop_rejects_new_nonterminal_operations() {
    let mut run = Rig::new(Config::default());
    run.send("one", "work").await;
    reply(run.request().await, calls(&[("a", "Bash")]));
    settle().await;
    run.control("hard").await;
    settle().await;
    run.updates
        .submit(spec().operation("unintroduced".into()))
        .await
        .unwrap();
    assert!(
        run.task
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("new nonterminal operation")
    );
}

#[tokio::test(start_paused = true)]
async fn cancel_errors_do_not_skip_other_operations() {
    let mut run = Rig::new(Config::default());
    run.send("one", "work").await;
    reply(run.request().await, calls(&[("a", "Bash"), ("b", "Bash")]));
    settle().await;
    run.saved.0.lock().unwrap().fail = Some("cancel");
    run.control("hard").await;
    let error = run.task.await.unwrap().unwrap_err().to_string();
    assert_eq!(error.lines().count(), 2);
    assert_eq!(run.saved.0.lock().unwrap().cancels.len(), 2);
}

#[tokio::test(start_paused = true)]
async fn when_idle_finishes_the_current_request_without_a_new_turn() {
    let mut run = Rig::new(Config::default());
    run.send("one", "work").await;
    let current = run.request().await;
    run.control("when_idle").await;
    settle().await;
    assert!(!current.cancellation.is_cancelled());
    reply(current, text("done"));
    run.task.await.unwrap().unwrap();
    assert_eq!(
        run.saved
            .0
            .lock()
            .unwrap()
            .trace
            .iter()
            .filter(|kind| **kind == "model")
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn grace_accumulates_completions_and_keeps_partial_results_staged() {
    let mut run = Rig::new(Config {
        tool_grace: Duration::from_millis(50),
        ..Config::default()
    });
    run.send("one", "work").await;
    reply(run.request().await, calls(&[("a", "Bash"), ("b", "Bash")]));
    settle().await;
    run.complete(0, "completed").await;
    settle().await;
    assert!(run.requests.try_recv().is_err());
    run.complete(1, "completed").await;
    let next = run.request().await;
    assert_eq!(result(&next.request, "a").as_deref(), Some("completed"));
    assert_eq!(result(&next.request, "b").as_deref(), Some("completed"));
    run.abort().await;
}

#[tokio::test(start_paused = true)]
async fn grace_expiry_wakes_only_after_some_input_is_pending() {
    let mut run = Rig::new(Config {
        tool_grace: Duration::from_millis(25),
        ..Config::default()
    });
    run.send("one", "work").await;
    reply(run.request().await, calls(&[("a", "Bash"), ("b", "Bash")]));
    settle().await;
    run.complete(0, "completed").await;
    let next = run.request().await;
    assert_eq!(result(&next.request, "a").as_deref(), Some("completed"));
    assert!(result(&next.request, "b").unwrap().contains("running"));
    assert!(run.saved.0.lock().unwrap().cancels.is_empty());
    run.abort().await;
}

#[tokio::test(start_paused = true)]
async fn after_tools_is_recorded_after_the_results_it_waits_for() {
    let mut run = Rig::new(Config::default());
    run.send("one", "work").await;
    let current = run.request().await;
    run.input
        .submit(external("deferred", "follow up", "after_tools"))
        .await
        .unwrap();
    settle().await;
    assert_eq!(run.count("input"), 1);
    reply(current, calls(&[("a", "Bash")]));
    settle().await;
    assert_eq!(run.count("input"), 1);
    run.complete(0, "completed").await;
    let next = run.request().await;
    assert!(has_text(&next.request, "follow up"));
    assert_eq!(result(&next.request, "a").as_deref(), Some("completed"));
    {
        let saved = run.saved.0.lock().unwrap();
        let input = saved
            .items
            .iter()
            .position(|item| matches!(item, ItemData::Input(input) if input.id == "deferred"))
            .unwrap();
        assert!(matches!(
            &saved.items[input - 1],
            ItemData::ToolCallStatus(_)
        ));
    }
    run.abort().await;
}

#[tokio::test(start_paused = true)]
async fn steering_flushes_deferred_input_but_hard_stop_does_not() {
    for hard in [false, true] {
        let mut run = Rig::new(Config::default());
        run.send("one", "work").await;
        let current = run.request().await;
        run.input
            .submit(external("deferred", "follow up", "after_tools"))
            .await
            .unwrap();
        settle().await;
        if hard {
            run.control("hard").await;
            run.task.await.unwrap().unwrap();
            assert!(current.cancellation.is_cancelled());
            assert!(
                !run.saved
                    .0
                    .lock()
                    .unwrap()
                    .items
                    .iter()
                    .any(|item| matches!(item, ItemData::Input(input) if input.id == "deferred"))
            );
        } else {
            run.send("steer", "change course").await;
            let next = run.request().await;
            assert!(current.cancellation.is_cancelled());
            assert!(has_text(&next.request, "follow up"));
            assert!(has_text(&next.request, "change course"));
            run.abort().await;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn after_tools_wait_limit_delivers_without_canceling_the_tool() {
    let mut run = Rig::new(Config {
        tool_wait_limit: Duration::from_millis(30),
        ..Config::default()
    });
    run.send("one", "work").await;
    reply(run.request().await, calls(&[("a", "Bash")]));
    settle().await;
    run.input
        .submit(external("deferred", "follow up", "after_tools"))
        .await
        .unwrap();
    let next = run.request().await;
    assert!(has_text(&next.request, "follow up"));
    assert!(result(&next.request, "a").unwrap().contains("running"));
    assert!(run.saved.0.lock().unwrap().cancels.is_empty());
    run.abort().await;
}

#[tokio::test(start_paused = true)]
async fn failed_completion_write_never_delivers_the_result() {
    let mut run = Rig::new(Config::default());
    run.send("one", "work").await;
    reply(run.request().await, calls(&[("a", "Bash")]));
    settle().await;
    run.saved.0.lock().unwrap().fail = Some("update");
    let id = run.saved.0.lock().unwrap().adds[0].id.clone();
    run.complete(0, "completed").await;
    assert_eq!(
        run.task.await.unwrap().unwrap_err().to_string(),
        format!("store operation {id:?}: update failed")
    );
    assert!(run.requests.try_recv().is_err());
}

#[tokio::test(start_paused = true)]
async fn continuation_refusal_and_failed_response_recovery_have_finite_bounds() {
    for (stop, responses, failure) in [
        ("max_output_tokens", 4, false),
        ("refused", 2, false),
        ("", 3, true),
    ] {
        let mut run = Rig::new(Config::default());
        run.send("one", "work").await;
        for _ in 0..responses {
            let mut response = text("partial");
            response.stop = stop.into();
            if failure {
                response.failure = Some(kou_domain::Failure {
                    code: "broken".into(),
                    message: "try again".into(),
                });
            }
            reply(run.request().await, response);
        }
        if failure {
            assert!(
                run.task
                    .await
                    .unwrap()
                    .unwrap_err()
                    .to_string()
                    .contains("3 times in a row")
            );
        } else {
            settle().await;
            assert!(run.requests.try_recv().is_err());
            assert_eq!(run.count("response"), responses);
            run.abort().await;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn no_recovery_does_not_continue_a_cut_answer() {
    let mut run = Rig::new(Config {
        no_recovery: true,
        ..Config::default()
    });
    run.send("one", "work").await;
    let mut response = text("partial");
    response.stop = "max_output_tokens".into();
    reply(run.request().await, response);
    settle().await;
    assert!(run.requests.try_recv().is_err());
    run.abort().await;
}

#[tokio::test(start_paused = true)]
async fn quota_backoff_blocks_steering_and_stops_after_two_waits() {
    let mut run = Rig::new(Config::default());
    run.send("one", "work").await;
    for index in 0..3 {
        let request = run.request().await;
        assert!(
            request
                .reply
                .send(Err(ModelError {
                    message: "quota".into(),
                    kind: ModelFailure::RateLimited(Duration::from_millis(40))
                }))
                .is_ok()
        );
        settle().await;
        if index < 2 {
            run.send(&format!("steer-{index}"), "steering during quota")
                .await;
            settle().await;
            assert!(run.requests.try_recv().is_err());
        }
    }
    assert!(
        run.task
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("quota")
    );
}

#[tokio::test(start_paused = true)]
async fn requested_compaction_finishes_before_delivering_new_input() {
    let mut run = Rig::new(Config::default());
    run.send("one", "work").await;
    reply(run.request().await, text("answer"));
    settle().await;
    run.control("compact").await;
    let compact = run.request().await;
    run.send("two", "new input").await;
    settle().await;
    assert!(!compact.cancellation.is_cancelled());
    assert!(run.requests.try_recv().is_err());
    reply(compact, text("<summary>kept task</summary>"));
    let next = run.request().await;
    assert!(has_text(&next.request, "kept task"));
    assert!(has_text(&next.request, "new input"));
    run.abort().await;
}

#[tokio::test(start_paused = true)]
async fn automatic_compaction_is_followed_by_the_answer_not_another_summary() {
    let mut run = Rig::new(Config {
        auto_compact_tokens: 1000,
        context_window: 10_000,
        ..Config::default()
    });
    run.send("one", "work").await;
    let mut response = text("answer");
    response.usage.input_tokens = 5000;
    reply(run.request().await, response);
    settle().await;
    run.send("two", "new work").await;
    reply(run.request().await, text("<summary>kept task</summary>"));
    let next = run.request().await;
    assert!(has_text(&next.request, "new work"));
    let turns: Vec<_> = run
        .saved
        .0
        .lock()
        .unwrap()
        .items
        .iter()
        .filter_map(|item| match item {
            ItemData::Turn(turn) => Some(turn.kind.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(turns, ["regular", "compaction", "regular"]);
    run.abort().await;
}

#[tokio::test(start_paused = true)]
async fn overflow_retries_the_same_compaction_turn_at_most_three_times() {
    let mut run = Rig::new(Config {
        auto_compact_tokens: 1_000_000,
        context_window: 10_000,
        ..Config::default()
    });
    run.send("one", "work").await;
    reply(run.request().await, text("answer"));
    settle().await;
    run.send("two", "new work").await;
    for _ in 0..5 {
        let request = run.request().await;
        assert!(
            request
                .reply
                .send(Err(ModelError {
                    message: "too large".into(),
                    kind: ModelFailure::ContextOverflow {
                        tokens: 20_000,
                        limit: 10_000
                    }
                }))
                .is_ok()
        );
    }
    assert!(
        run.task
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("too large")
    );
    let turns: Vec<_> = run
        .saved
        .0
        .lock()
        .unwrap()
        .items
        .iter()
        .filter_map(|item| match item {
            ItemData::Turn(turn) => Some(turn.kind.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(turns, ["regular", "regular", "compaction"]);
}

#[tokio::test(start_paused = true)]
async fn turn_budget_gets_one_final_report_then_cancels_new_work() {
    let mut run = Rig::new(Config {
        max_turns: 1,
        ..Config::default()
    });
    run.send("one", "work").await;
    reply(run.request().await, text("first answer"));
    let report = run.request().await;
    assert!(has_text(&report.request, "write the final report"));
    reply(report, text("final report"));
    run.task.await.unwrap().unwrap();
    assert!(run.saved.0.lock().unwrap().cancels.is_empty());
}

struct Check;
impl Verifier for Check {
    fn spec(&self) -> Option<OperationSpec> {
        Some(spec())
    }
    fn report(&self, operation: &Operation) -> (String, bool) {
        ("check failed".into(), operation.status == "completed")
    }
}
#[tokio::test(start_paused = true)]
async fn failed_verification_is_journaled_as_harness_work_before_retrying() {
    let mut run = Rig::custom(Config::default(), |driver| {
        driver.with_verifier(Arc::new(Check))
    });
    run.send("one", "work").await;
    reply(run.request().await, calls(&[("write", "Write")]));
    settle().await;
    run.complete(0, "completed").await;
    reply(run.request().await, text("done"));
    settle().await;
    assert_eq!(run.saved.0.lock().unwrap().adds.len(), 2);
    assert_eq!(
        run.saved.0.lock().unwrap().adds[1].idempotency.as_str(),
        Some(VERIFICATION_MARKER)
    );
    run.control("when_idle").await;
    settle().await;
    assert!(!run.task.is_finished());
    run.complete(1, "failed").await;
    let retry = run.request().await;
    assert!(has_text(
        &retry.request,
        "[harness verification] check failed"
    ));
    reply(retry, text("reported"));
    run.task.await.unwrap().unwrap();
}
