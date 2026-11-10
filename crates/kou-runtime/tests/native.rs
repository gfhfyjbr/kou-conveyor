use kou_domain::Json;
use kou_runtime::{
    Cancellation, CodeConfig, CodeHost, CommandTool, HostCall, NativeConfig, NativeHost, run_code,
};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs,
    path::Path,
    rc::Rc,
    time::{Duration, Instant},
};

fn config(directory: &Path) -> NativeConfig {
    NativeConfig {
        directory: directory.to_owned(),
        termination_grace: Duration::from_millis(100),
        ..Default::default()
    }
}
fn run(config: NativeConfig, code: &str) -> kou_runtime::Execution {
    let host = NativeHost::new(config).unwrap();
    let vm = CodeConfig {
        tools: host.tool_names(),
        timeout: Duration::from_secs(4),
        ..Default::default()
    };
    let host = Rc::new(RefCell::new(host));
    let result = run_code(host.clone(), &vm, code, Cancellation::default());
    assert!(Rc::try_unwrap(host).is_ok(), "VM retained native host");
    result
}

#[test]
fn vm_calls_real_file_tools_without_changing_global_cwd() {
    let root = tempfile::tempdir().unwrap();
    let cwd = std::env::current_dir().unwrap();
    let result = run(
        config(root.path()),
        "await write('a.txt','one\\ntwo\\n'); await edit('a.txt','two','second'); const numbered=await read('a.txt',{offset:2}); const raw=await readText('a.txt'); return [numbered,raw];",
    );
    assert!(result.error.is_empty(), "{result:?}");
    assert!(result.value.contains("second"));
    assert_eq!(
        fs::read_to_string(root.path().join("a.txt")).unwrap(),
        "one\nsecond\n"
    );
    assert_eq!(result.calls.len(), 4);
    assert_eq!(
        result.calls[0].arguments,
        root.path().join("a.txt").to_string_lossy()
    );
    assert!(result.calls[1].output.starts_with("Edited "));
    assert_eq!(std::env::current_dir().unwrap(), cwd);
}

#[test]
fn bash_value_has_streams_status_and_timeout_without_throwing_on_exit() {
    let root = tempfile::tempdir().unwrap();
    let result = run(
        config(root.path()),
        "const a=await bash(\"printf out; printf err >&2; exit 7\"); const b=await bash('sleep 30',{timeout:0.05}); return [a,b.timedOut,b.exitCode];",
    );
    assert!(result.error.is_empty(), "{result:?}");
    assert!(result.value.contains("\"stdout\": \"out\""));
    assert!(result.value.contains("\"exitCode\": 7"));
    assert_eq!(result.calls[0].exit_code, Some(7));
    assert!(result.calls[1].output.contains("timed out"));
}

#[test]
fn plugins_receive_json_environment_and_call_indices_not_timer_tickets() {
    let root = tempfile::tempdir().unwrap();
    let mut native = config(root.path());
    native.call_id = "outer".into();
    native.environment = Some(BTreeMap::new());
    native.tools.push(CommandTool {
        name: "probe".into(), command: vec!["/bin/sh".into(), "-c".into(), "printf '%s:%s:%s:' \"$KOU_CONVEYOR_TOOL_NAME\" \"$KOU_CONVEYOR_TOOL_CALL_ID\" \"$EXTRA\"; cat".into()],
        environment: BTreeMap::from([("EXTRA".into(), "own".into())]), directory: root.path().to_owned(),
    });
    let result = run(
        native,
        "await sleep(1); const a=await probe({n:1}); await bash('true'); const b=await probe({n:2}); return [a,b];",
    );
    assert!(result.error.is_empty(), "{result:?}");
    assert!(result.value.contains("probe:outer-0:own:"));
    assert!(result.value.contains("probe:outer-2:own:"));
    assert_eq!(result.calls[0].arguments, "{\"n\":1}");
}

#[test]
fn plugin_failure_is_catchable_and_preserves_record_output() {
    let root = tempfile::tempdir().unwrap();
    let mut native = config(root.path());
    native.tools.push(CommandTool {
        name: "fail".into(),
        command: vec![
            "/bin/sh".into(),
            "-c".into(),
            "printf reason; exit 5".into(),
        ],
        environment: BTreeMap::new(),
        directory: root.path().to_owned(),
    });
    let result = run(native, "try {await fail({})} catch(e) {return e.message}");
    assert!(result.error.is_empty(), "{result:?}");
    assert_eq!(result.value, "fail failed:\nreason\nExit code: 5");
    assert_eq!(result.calls[0].output, "reason\nExit code: 5");
    assert_eq!(result.calls[0].exit_code, Some(5));
}

#[test]
fn pending_count_and_argument_admission_release_on_completion() {
    let root = tempfile::tempdir().unwrap();
    let mut host = NativeHost::new(NativeConfig {
        max_pending: 1,
        max_argument_bytes: 30,
        ..config(root.path())
    })
    .unwrap();
    let call = |ticket, args: &str| HostCall {
        ticket,
        index: 0,
        name: "read".into(),
        arguments: Json::parse(args).unwrap(),
        plugin: false,
    };
    assert!(
        host.start(call(1, "[\"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\"]"))
            .unwrap_err()
            .contains("byte budget")
    );
    host.start(call(2, "[\"missing\"]")).unwrap();
    assert!(
        host.start(call(3, "[]"))
            .unwrap_err()
            .contains("queue is full")
    );
    let completed = host.wait(Duration::from_secs(2)).unwrap();
    assert_eq!(completed.ticket, 2);
    assert!(completed.result.is_err());
    host.start(call(4, "[\"missing\"]")).unwrap();
    assert_eq!(host.wait(Duration::from_secs(2)).unwrap().ticket, 4);
    host.cancel();
    assert!(host.start(call(5, "[]")).unwrap_err().contains("canceled"));
}

#[test]
fn vm_return_joins_workers_even_with_a_retained_host_and_full_completion_queue() {
    let root = tempfile::tempdir().unwrap();
    let host = Rc::new(RefCell::new(
        NativeHost::new(NativeConfig {
            workers: 1,
            ..config(root.path())
        })
        .unwrap(),
    ));
    let start = Instant::now();
    let result = run_code(
        host.clone(),
        &CodeConfig::default(),
        "for(let i=0;i<50;i++) bash('sleep 30'); return 'done';",
        Cancellation::default(),
    );
    assert!(result.error.is_empty(), "{result:?}");
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_eq!(result.value, "done");
    assert!(result.calls.iter().all(|call| !call.running));
    assert!(host.borrow_mut().wait(Duration::ZERO).is_none());
}

#[test]
fn native_cancellation_kills_processes_before_returning_and_skips_queued_writes() {
    let root = tempfile::tempdir().unwrap();
    let cancel = Cancellation::default();
    let trigger = cancel.clone();
    let marker = root.path().join("started");
    let watcher = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !marker.exists() {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        trigger.cancel();
    });
    let host = Rc::new(RefCell::new(
        NativeHost::new(NativeConfig {
            workers: 1,
            ..config(root.path())
        })
        .unwrap(),
    ));
    let result = run_code(
        host.clone(),
        &CodeConfig::default(),
        "const p=bash('echo ready > started; sleep 30'); const w=write('must-not-exist','later'); await p; await w;",
        cancel,
    );
    watcher.join().unwrap();
    assert!(result.error.contains("canceled"), "{result:?}");
    assert!(!root.path().join("must-not-exist").exists());
    assert!(host.borrow_mut().wait(Duration::ZERO).is_none());
}
