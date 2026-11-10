use kou_tools::process::{
    CancelToken, CapturePaths, CapturedEvent, ProcessRequest, run_captured_process_events,
};
use std::{
    cell::Cell,
    fs, io,
    os::unix::fs::OpenOptionsExt,
    panic::{AssertUnwindSafe, catch_unwind},
    time::{Duration, Instant},
};

fn fixture() -> (tempfile::TempDir, CapturePaths, ProcessRequest) {
    let directory = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(directory.path()).unwrap();
    let paths = CapturePaths {
        stdout: root.join("out"),
        stderr: root.join("err"),
    };
    for path in [&paths.stdout, &paths.stderr] {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap();
    }
    let mut request = ProcessRequest::new("/bin/sh");
    request.directory = root;
    request.arguments = vec!["-c".into(), "trap 'if [ -f timeout-ack ]; then printf ack >&2; else printf missing-ack >&2; fi; exit 0' TERM; printf ready; while :; do :; done".into()];
    request.timeout = Some(Duration::from_millis(30));
    request.termination_grace = Duration::from_millis(30);
    (directory, paths, request)
}
fn ready(paths: &CapturePaths) {
    let end = Instant::now() + Duration::from_secs(3);
    while fs::read(&paths.stdout).unwrap() != b"ready" {
        assert!(Instant::now() < end, "fixture readiness deadline");
        std::thread::yield_now();
    }
}
fn gone(pid: u32) {
    assert_eq!(
        rustix::process::test_kill_process_group(
            rustix::process::Pid::from_raw(pid as i32).unwrap()
        ),
        Err(rustix::io::Errno::SRCH)
    );
}
#[test]
fn timeout_notification_precedes_term_and_can_publish_an_owned_checkpoint() {
    let (_directory, paths, request) = fixture();
    let mut events = Vec::new();
    let result = run_captured_process_events(&request, &paths, &CancelToken::default(), |event| {
        match event {
            CapturedEvent::Started { .. } => ready(&paths),
            CapturedEvent::TimedOut { .. } => {
                assert!(fs::read(&paths.stderr)?.is_empty());
                fs::write(request.directory.join("timeout-ack"), "durable-control")?;
            }
        }
        events.push(event);
        Ok(())
    })
    .unwrap();
    let [
        CapturedEvent::Started { pid },
        CapturedEvent::TimedOut { pid: timed },
    ] = events.as_slice()
    else {
        panic!("{events:?}")
    };
    assert_eq!(pid, timed);
    assert!(result.timed_out && !result.canceled);
    assert_eq!(result.exit_code, 0);
    assert_eq!(fs::read(paths.stderr).unwrap(), b"ack");
    gone(*pid);
}
#[test]
fn failed_or_panicking_timeout_callbacks_kill_and_reap_the_invocation() {
    for panic in [false, true] {
        let (_directory, paths, request) = fixture();
        let pid = Cell::new(0);
        let result = catch_unwind(AssertUnwindSafe(|| {
            run_captured_process_events(&request, &paths, &CancelToken::default(), |event| {
                match event {
                    CapturedEvent::Started { pid: id } => {
                        pid.set(id);
                        ready(&paths);
                        Ok(())
                    }
                    CapturedEvent::TimedOut { .. } if panic => panic!("timeout publisher panic"),
                    CapturedEvent::TimedOut { .. } => {
                        Err(io::Error::other("timeout publisher refused"))
                    }
                }
            })
        }));
        if panic {
            assert!(result.is_err());
        } else {
            assert_eq!(
                result.unwrap().unwrap_err().to_string(),
                "timeout publisher refused"
            );
        }
        gone(pid.get());
        assert!(fs::read(paths.stderr).unwrap().is_empty());
    }
}
#[test]
fn canceled_invocation_does_not_emit_a_timeout_event() {
    let (_directory, paths, mut request) = fixture();
    request.timeout = None;
    let cancel = CancelToken::default();
    let mut events = Vec::new();
    let result = run_captured_process_events(&request, &paths, &cancel, |event| {
        assert!(matches!(event, CapturedEvent::Started { .. }));
        ready(&paths);
        events.push(event);
        cancel.cancel();
        Ok(())
    })
    .unwrap();
    assert!(result.canceled && !result.timed_out);
    let [CapturedEvent::Started { pid }] = events.as_slice() else {
        panic!("{events:?}")
    };
    gone(*pid);
}
#[test]
fn impossible_timer_configuration_never_mutates_captures_or_spawns() {
    for grace in [false, true] {
        let (_directory, paths, mut request) = fixture();
        fs::write(&paths.stdout, "keep-out").unwrap();
        fs::write(&paths.stderr, "keep-err").unwrap();
        if grace {
            request.termination_grace = Duration::MAX;
        } else {
            request.timeout = Some(Duration::MAX);
        }
        assert_eq!(
            run_captured_process_events(&request, &paths, &CancelToken::default(), |_| panic!(
                "must not spawn"
            ))
            .unwrap_err()
            .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(fs::read(paths.stdout).unwrap(), b"keep-out");
        assert_eq!(fs::read(paths.stderr).unwrap(), b"keep-err");
    }
}

#[test]
fn slow_timeout_callback_does_not_consume_the_physical_term_grace() {
    let (_directory, paths, mut request) = fixture();
    request.arguments[1] = "trap '/bin/sleep 0.05; printf graceful >&2; exit 0' TERM; printf ready; while :; do :; done".into();
    request.termination_grace = Duration::from_millis(200);
    let pid = Cell::new(0);
    let result = run_captured_process_events(&request, &paths, &CancelToken::default(), |event| {
        match event {
            CapturedEvent::Started { pid: id } => {
                pid.set(id);
                ready(&paths);
            }
            CapturedEvent::TimedOut { .. } => {
                let (sender, receiver) = std::sync::mpsc::channel::<()>();
                assert_eq!(
                    receiver.recv_timeout(Duration::from_millis(300)),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                );
                drop(sender);
            }
        }
        Ok(())
    })
    .unwrap();
    assert!(result.timed_out);
    assert_eq!((result.exit_code, result.signal), (0, 0));
    assert_eq!(fs::read(paths.stderr).unwrap(), b"graceful");
    gone(pid.get());
}
