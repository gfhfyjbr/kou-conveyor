use kou_tools::process::{CancelToken, CapturePaths, ProcessRequest, run_captured_process};
use std::{
    cell::Cell,
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io,
    os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink},
    panic::{AssertUnwindSafe, catch_unwind},
    time::{Duration, Instant},
};

fn fixture() -> (tempfile::TempDir, CapturePaths) {
    let directory = tempfile::tempdir().unwrap();
    let paths = CapturePaths {
        stdout: directory.path().join("out"),
        stderr: directory.path().join("err"),
    };
    for path in [&paths.stdout, &paths.stderr] {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap();
    }
    (directory, paths)
}
fn shell(code: &str) -> ProcessRequest {
    let mut request = ProcessRequest::new("/bin/sh");
    request.arguments = vec!["-c".into(), code.into()];
    request.output_limit = 0;
    request.termination_grace = Duration::from_millis(30);
    request
}
fn group_gone(id: u32) {
    let group = rustix::process::Pid::from_raw(id as i32).unwrap();
    assert_eq!(
        rustix::process::test_kill_process_group(group),
        Err(rustix::io::Errno::SRCH)
    );
}

#[test]
fn captures_are_regular_descriptors_and_full_output_is_not_retained_in_ram() {
    let (_directory, paths) = fixture();
    fs::write(&paths.stdout, "old output must be truncated").unwrap();
    let request = shell(
        "test -f /dev/stdout && test -f /dev/stderr || exit 91; printf 'regular:'; i=0; while [ \"$i\" -lt 20000 ]; do printf '0123456789'; i=$((i+1)); done; printf ':end'; printf 'owned stderr' >&2",
    );
    let started = Cell::new(0);
    let result = run_captured_process(&request, &paths, &CancelToken::default(), |id| {
        started.set(id);
        Ok(())
    })
    .unwrap();
    assert!(started.get() > 0);
    assert_eq!(result.exit_code, 0);
    assert!(!result.timed_out && !result.canceled);
    assert_eq!(result.signal, 0);
    assert_eq!(
        fs::read_to_string(&paths.stdout).unwrap(),
        format!("regular:{}:end", "0123456789".repeat(20000))
    );
    assert_eq!(fs::read_to_string(&paths.stderr).unwrap(), "owned stderr");
    assert_eq!(
        fs::metadata(&paths.stdout).unwrap().permissions().mode() & 0o777,
        0o600
    );
    group_gone(started.get());
}

#[test]
fn invalid_second_capture_and_aliases_never_truncate_the_first() {
    for kind in [
        "same",
        "hardlink",
        "symlink",
        "missing",
        "directory",
        "fifo",
    ] {
        let (directory, mut paths) = fixture();
        fs::write(&paths.stdout, "keep stdout").unwrap();
        fs::remove_file(&paths.stderr).unwrap();
        match kind {
            "same" => paths.stderr = paths.stdout.clone(),
            "hardlink" => fs::hard_link(&paths.stdout, &paths.stderr).unwrap(),
            "symlink" => symlink(&paths.stdout, &paths.stderr).unwrap(),
            "missing" => {}
            "directory" => paths.stderr = directory.path().to_owned(),
            "fifo" => assert!(
                std::process::Command::new("/usr/bin/mkfifo")
                    .arg(&paths.stderr)
                    .status()
                    .unwrap()
                    .success()
            ),
            _ => unreachable!(),
        }
        let result = run_captured_process(
            &shell("printf must-not-run"),
            &paths,
            &CancelToken::default(),
            |_| panic!("invalid capture spawned"),
        );
        assert!(result.is_err(), "{kind}");
        assert_eq!(
            fs::read_to_string(&paths.stdout).unwrap(),
            "keep stdout",
            "{kind}"
        );
    }
}

#[test]
fn failed_or_panicking_start_callbacks_kill_and_reap_the_owned_group() {
    for panics in [false, true] {
        let (_directory, paths) = fixture();
        let started = Cell::new(0);
        let result = catch_unwind(AssertUnwindSafe(|| {
            run_captured_process(
                &shell("/bin/sleep 5"),
                &paths,
                &CancelToken::default(),
                |id| {
                    started.set(id);
                    assert!(!panics, "owned start callback panic");
                    Err(io::Error::other("owned start callback failed"))
                },
            )
        }));
        if panics {
            assert!(result.is_err());
        } else {
            assert_eq!(
                result.unwrap().unwrap_err().to_string(),
                "owned start callback failed"
            );
        }
        assert!(started.get() > 0);
        group_gone(started.get());
    }
}

#[test]
fn timeout_and_cancellation_close_stdin_and_reap_the_leader() {
    for cancellation in [false, true] {
        let (_directory, paths) = fixture();
        let mut request = shell("trap '' TERM; /bin/sleep 5");
        request.timeout = if cancellation {
            None
        } else {
            Some(Duration::from_millis(30))
        };
        request.input = Some(vec![b'x'; 2 << 20]);
        let cancel = CancelToken::default();
        let started = Cell::new(0);
        let before = Instant::now();
        let result = run_captured_process(&request, &paths, &cancel, |id| {
            started.set(id);
            if cancellation {
                cancel.cancel();
            }
            Ok(())
        })
        .unwrap();
        assert!(before.elapsed() < Duration::from_secs(3));
        assert_eq!(result.canceled, cancellation);
        assert_eq!(result.timed_out, !cancellation);
        assert_eq!(result.exit_code, -1);
        assert!(matches!(result.signal, 9 | 15));
        group_gone(started.get());
    }
}

#[test]
fn normal_leader_exit_cleans_up_descendants_before_capture_completion() {
    let (_directory, paths) = fixture();
    let started = Cell::new(0);
    let before = Instant::now();
    let result = run_captured_process(
        &shell("/bin/sleep 30 & printf leader; printf stderr >&2; exit 3"),
        &paths,
        &CancelToken::default(),
        |id| {
            started.set(id);
            Ok(())
        },
    )
    .unwrap();
    assert!(before.elapsed() < Duration::from_secs(3));
    assert_eq!((result.exit_code, result.signal), (3, 0));
    assert!(!result.timed_out && !result.canceled);
    assert_eq!(fs::read(&paths.stdout).unwrap(), b"leader");
    assert_eq!(fs::read(&paths.stderr).unwrap(), b"stderr");
    group_gone(started.get());
}

#[test]
fn capture_sources_and_unrelated_descriptors_are_closed_on_exec() {
    use std::os::fd::AsRawFd;
    let (directory, paths) = fixture();
    let unrelated = tempfile::NamedTempFile::new().unwrap();
    let fd = unrelated.as_file().as_raw_fd();
    let path = unrelated.path().to_string_lossy().replace('\'', "'\\''");
    let mut request = shell(&format!(
        "if [ /dev/fd/{fd} -ef '{path}' ]; then exit 91; fi; for f in /dev/fd/*; do case \"$f\" in /dev/fd/1|/dev/fd/2) continue;; esac; if [ \"$f\" -ef out ] || [ \"$f\" -ef err ]; then exit 92; fi; done; printf cloexec"
    ));
    request.directory = directory.path().to_owned();
    let result =
        run_captured_process(&request, &paths, &CancelToken::default(), |_| Ok(())).unwrap();
    assert_eq!(result.exit_code, 0, "{result:?}");
    assert_eq!(fs::read(&paths.stdout).unwrap(), b"cloexec");
    assert!(fs::read(&paths.stderr).unwrap().is_empty());
}

#[test]
fn captured_input_explicit_environment_and_directory_match_the_request() {
    let (directory, paths) = fixture();
    let mut request =
        shell("printf '%s:%s\\n' \"$OWNED_CAPTURE\" \"${HOME-unset}\"; /bin/cat; /bin/pwd >&2");
    request.directory = directory.path().to_owned();
    request.environment = Some(BTreeMap::from([("OWNED_CAPTURE".into(), "yes".into())]));
    request.input = Some(b"owned stdin".to_vec());
    let result =
        run_captured_process(&request, &paths, &CancelToken::default(), |_| Ok(())).unwrap();
    assert_eq!(result.exit_code, 0);
    assert_eq!(
        fs::read_to_string(&paths.stdout).unwrap(),
        "yes:unset\nowned stdin"
    );
    assert_eq!(
        fs::read_to_string(&paths.stderr).unwrap(),
        format!(
            "{}\n",
            fs::canonicalize(directory.path()).unwrap().display()
        )
    );
}

#[test]
fn already_canceled_start_does_not_touch_capture_files() {
    let (_directory, paths) = fixture();
    fs::write(&paths.stdout, "preserve").unwrap();
    let cancel = CancelToken::default();
    cancel.cancel();
    let result = run_captured_process(&shell("exit 99"), &paths, &cancel, |_| {
        panic!("canceled start spawned")
    });
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
    assert_eq!(fs::read_to_string(&paths.stdout).unwrap(), "preserve");
}
