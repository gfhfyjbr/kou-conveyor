use kou_tools::process::{CancelToken, ProcessRequest, run_process};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    time::{Duration, Instant},
};

fn shell(code: &str) -> ProcessRequest {
    let mut request = ProcessRequest::new("/bin/sh");
    request.arguments = vec!["-c".into(), code.into()];
    request.termination_grace = Duration::from_millis(100);
    request
}

#[test]
fn streams_exit_codes_and_shell_text() {
    let result = run_process(
        &shell("printf 'out\n'; printf 'err\n' >&2; exit 7"),
        &CancelToken::default(),
    )
    .unwrap();
    assert_eq!(
        (
            &*result.stdout,
            &*result.stderr,
            result.exit_code,
            result.timed_out
        ),
        ("out\n", "err\n", 7, false)
    );
    assert_eq!(result.text(), "out\n\nStderr:\nerr\n\nExit code: 7");
    assert_eq!(
        run_process(&shell("kill -TERM $$"), &CancelToken::default())
            .unwrap()
            .exit_code,
        143
    );
}

#[test]
fn input_is_nonblocking_and_large_output_is_drained_but_not_retained() {
    let mut request = shell("cat");
    request.input = Some(vec![b'a'; 4 << 20]);
    request.output_limit = 100;
    let result = run_process(&request, &CancelToken::default()).unwrap();
    assert_eq!(result.exit_code, 0);
    assert_eq!(
        result.stdout,
        format!(
            "{}...4194204 bytes truncated...{}",
            "a".repeat(50),
            "a".repeat(50)
        )
    );
    assert!(result.stderr.is_empty());
}

#[test]
fn directory_and_explicit_environment_do_not_leak_parent_values() {
    let root = tempfile::tempdir().unwrap();
    let mut request = shell("printf '%s:%s:' \"$ONLY\" \"$HOME\"; pwd");
    request.directory = root.path().to_owned();
    request.environment = Some(BTreeMap::from([(
        OsString::from("ONLY"),
        OsString::from("test"),
    )]));
    let result = run_process(&request, &CancelToken::default()).unwrap();
    assert!(result.stdout.starts_with("test::"), "{}", result.stdout);
    assert!(
        result
            .stdout
            .trim_end()
            .ends_with(&*root.path().canonicalize().unwrap().to_string_lossy())
    );
}

#[test]
fn timeout_escalates_and_interrupts_a_blocked_input_writer() {
    let mut request = shell("trap '' TERM; printf 'ready\n'; while :; do :; done");
    request.input = Some(vec![b'x'; 4 << 20]);
    request.timeout = Some(Duration::from_millis(100));
    let start = Instant::now();
    let result = run_process(&request, &CancelToken::default()).unwrap();
    assert!(start.elapsed() < Duration::from_secs(3));
    assert!(result.timed_out);
    assert_eq!(result.exit_code, 137);
    assert!(result.stdout.contains("ready"));
}

#[test]
fn cancellation_terminates_the_group_and_reaps_the_leader() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("leader");
    let mut request =
        shell("printf '%s' \"$$\" > leader.new; mv leader.new leader; sleep 30 & wait");
    request.directory = root.path().to_owned();
    let token = CancelToken::default();
    struct Cleanup {
        token: CancelToken,
        job: Option<std::thread::JoinHandle<std::io::Result<kou_tools::process::ProcessResult>>>,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.token.cancel();
            if let Some(job) = self.job.take() {
                let _ = job.join();
            }
        }
    }
    let mut cleanup = Cleanup {
        token: token.clone(),
        job: Some(std::thread::spawn(move || run_process(&request, &token))),
    };
    let deadline = Instant::now() + Duration::from_secs(3);
    while !marker.exists() {
        assert!(Instant::now() < deadline, "child did not start");
        std::thread::yield_now();
    }
    let pid: i32 = fs::read_to_string(&marker).unwrap().parse().unwrap();
    cleanup.token.cancel();
    let result = cleanup.job.take().unwrap().join().unwrap().unwrap();
    assert!(result.canceled && !result.timed_out);
    assert_eq!(
        rustix::process::test_kill_process(rustix::process::Pid::from_raw(pid).unwrap()),
        Err(rustix::io::Errno::SRCH)
    );
}

#[test]
fn leader_exit_cleans_up_descendants_and_never_waits_for_inherited_output_forever() {
    let request = shell("sleep 30 & printf 'leader exited\n'; exit 0");
    let started = Instant::now();
    let result = run_process(&request, &CancelToken::default()).unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(result.stdout, "leader exited\n");
    assert_eq!(result.exit_code, 0);
}

#[test]
fn canceled_start_and_invalid_configuration_never_spawn() {
    let token = CancelToken::default();
    token.cancel();
    assert!(
        run_process(&shell("exit 0"), &token)
            .unwrap_err()
            .to_string()
            .contains("canceled")
    );
    assert!(
        run_process(&ProcessRequest::new("sh"), &CancelToken::default())
            .unwrap_err()
            .to_string()
            .contains("absolute")
    );
    assert!(
        run_process(
            &ProcessRequest::new("/no/such/kou-test-executable"),
            &CancelToken::default()
        )
        .is_err()
    );
}

#[test]
fn unrelated_owned_descriptors_are_not_inherited() {
    use std::os::fd::AsRawFd;
    let file = tempfile::NamedTempFile::new().unwrap();
    let fd = file.as_file().as_raw_fd();
    assert!(
        rustix::io::fcntl_getfd(file.as_file())
            .unwrap()
            .contains(rustix::io::FdFlags::CLOEXEC)
    );
    let path = file.path().to_string_lossy().replace('\'', "'\\''");
    // A shell can reuse a closed number for its own redirection bookkeeping.
    // Check the file's identity, not just whether that number is open.
    let script = format!("if [ /dev/fd/{fd} -ef '{path}' ]; then exit 91; else exit 0; fi");
    let result = run_process(&shell(&script), &CancelToken::default()).unwrap();
    assert_eq!(result.exit_code, 0, "{result:?}");
    assert!(result.stderr.is_empty());
}
