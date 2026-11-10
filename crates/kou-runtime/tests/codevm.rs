use kou_domain::Json;
use kou_runtime::{
    Cancellation, CodeConfig, CodeHost, Completion, Execution, HostCall, run_code as run_vm,
};
use std::{
    cell::RefCell,
    collections::VecDeque,
    rc::Rc,
    time::{Duration, Instant},
};

fn run_code<H: CodeHost + Default + 'static>(
    host: &mut H,
    config: &CodeConfig,
    code: &str,
    cancel: Cancellation,
) -> Execution {
    let bridge = Rc::new(RefCell::new(std::mem::take(host)));
    let result = run_vm(bridge.clone(), config, code, cancel);
    *host = Rc::try_unwrap(bridge)
        .ok()
        .expect("VM retained host after shutdown")
        .into_inner();
    result
}

#[derive(Default)]
struct Host {
    calls: Vec<HostCall>,
    completions: VecDeque<Completion>,
    canceled: bool,
    fail: bool,
}
impl CodeHost for Host {
    fn start(&mut self, call: HostCall) -> Result<(), String> {
        let value = Json::parse(&serde_json::to_string(&format!("value:{}", call.ticket)).unwrap())
            .unwrap();
        self.completions.push_front(Completion {
            ticket: call.ticket,
            result: if self.fail {
                Err("host failed".into())
            } else {
                Ok(value)
            },
            output: "host output".into(),
            exit_code: Some(0),
        });
        self.calls.push(call);
        Ok(())
    }
    fn wait(&mut self, timeout: Duration) -> Option<Completion> {
        if let Some(value) = self.completions.pop_front() {
            Some(value)
        } else {
            std::thread::park_timeout(timeout);
            None
        }
    }
    fn cancel(&mut self) {
        self.canceled = true;
        self.completions.clear()
    }
}
fn config() -> CodeConfig {
    CodeConfig {
        tools: vec![
            "echo".into(),
            "net.fetch".into(),
            "JSON".into(),
            "bash".into(),
            "__proto__".into(),
        ],
        timeout: Duration::from_millis(500),
        ..CodeConfig::default()
    }
}

#[test]
fn promises_run_in_parallel_and_records_keep_start_order() {
    let mut host = Host::default();
    let result = run_code(
        &mut host,
        &config(),
        "const values = await Promise.all([echo({n:1}), echo({n:2})]); console.log(values); return values.join('/');",
        Cancellation::default(),
    );
    assert!(result.error.is_empty(), "{result:?}");
    assert_eq!(result.value, "value:1/value:2");
    assert_eq!(result.logs, "[\"value:1\",\"value:2\"]\n");
    assert_eq!(host.calls.len(), 2);
    assert_eq!(result.calls[0].arguments.as_str(), "{\"n\":1}");
    assert!(result.calls.iter().all(|call| !call.running));
    assert!(host.canceled);
}

#[test]
fn native_and_plugin_names_are_distinct_and_dangerous_names_do_not_replace_globals() {
    let mut host = Host::default();
    let code = "await bash('true'); await tools.bash({x:1}); await tools['net.fetch']({}); await tools.JSON({}); await tools['__proto__']({}); return typeof JSON.stringify;";
    let result = run_code(&mut host, &config(), code, Cancellation::default());
    assert!(result.error.is_empty(), "{result:?}");
    assert_eq!(result.value, "function");
    assert!(!host.calls[0].plugin);
    assert!(host.calls[1..].iter().all(|call| call.plugin));
}

#[test]
fn rejected_host_calls_are_catchable_and_invalid_arguments_never_start() {
    let mut host = Host {
        fail: true,
        ..Host::default()
    };
    let result = run_code(
        &mut host,
        &config(),
        "try { await echo({}); } catch (error) { console.log(error.message); } return 'caught';",
        Cancellation::default(),
    );
    assert!(result.error.is_empty(), "{result:?}");
    assert_eq!(result.logs, "host failed\n");
    assert_eq!(result.calls[0].error, "host failed");
    for code in [
        "await read(42)",
        "await read('x', {limti:4})",
        "await write('x', {})",
        "await echo([])",
        "await tools['net.fetch']('bad')",
    ] {
        let mut host = Host::default();
        let result = run_code(&mut host, &config(), code, Cancellation::default());
        assert!(!result.error.is_empty(), "{code}: {result:?}");
        assert!(host.calls.is_empty());
        assert!(host.canceled);
    }
}

#[test]
fn limits_cancel_real_pending_work_and_stalled_promises_do_not_hang() {
    let mut host = Host::default();
    let result = run_code(
        &mut host,
        &config(),
        "await new Promise(() => {})",
        Cancellation::default(),
    );
    assert!(result.error.contains("nothing will settle"), "{result:?}");
    let result = run_code(
        &mut host,
        &CodeConfig {
            max_calls: 2,
            ..config()
        },
        "echo({}); echo({}); await echo({});",
        Cancellation::default(),
    );
    assert!(
        result.error.contains("the most a run may make"),
        "{result:?}"
    );
    assert_eq!(result.calls.len(), 2);
    assert!(
        result
            .calls
            .iter()
            .all(|call| call.error.contains("run ended"))
    );
    assert!(host.canceled);
}

#[test]
fn interrupts_cpu_loops_microtask_loops_and_result_formatters() {
    for code in [
        "for (;;) {}",
        "for (;;) { await Promise.resolve(1) }",
        "return {toJSON() { for (;;) {} }}",
        "throw {toJSON() { for (;;) {} }}",
    ] {
        let started = Instant::now();
        let mut host = Host::default();
        let result = run_code(
            &mut host,
            &CodeConfig {
                timeout: Duration::from_millis(30),
                ..config()
            },
            code,
            Cancellation::default(),
        );
        assert!(result.error.contains("stopped"), "{code}: {result:?}");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(host.canceled);
    }
}

#[test]
fn parent_cancellation_sleep_and_stable_formatters() {
    let cancellation = Cancellation::default();
    cancellation.cancel();
    let mut host = Host::default();
    assert!(
        run_code(&mut host, &config(), "for (;;) {}", cancellation)
            .error
            .contains("canceled")
    );
    let mut host = Host::default();
    let result = run_code(
        &mut host,
        &config(),
        "await sleep(1); JSON.stringify = () => 'wrong'; console.log(new Set([1,2]), new Map([['a',3]])); return {a:1};",
        Cancellation::default(),
    );
    assert!(result.error.is_empty(), "{result:?}");
    assert_eq!(result.value, "{\n  \"a\": 1\n}");
    assert_eq!(result.logs, "Set [1,2] Map [[\"a\",3]]\n");
    assert!(result.calls.is_empty());
}

#[test]
fn bounded_unicode_output_and_oom_do_not_crash_the_process() {
    let mut host = Host::default();
    let result = run_code(
        &mut host,
        &CodeConfig {
            max_output: 120,
            ..config()
        },
        "for(let i=0;i<1000;i++) console.log('好'.repeat(10)); return '終'.repeat(1000);",
        Cancellation::default(),
    );
    assert!(result.error.is_empty(), "{result:?}");
    assert!(result.logs.len() < 200 && result.value.len() < 200);
    assert!(result.value.ends_with('終'));
    assert!(!result.logs.contains('\u{fffd}') && !result.value.contains('\u{fffd}'));
    let result = run_code(
        &mut host,
        &CodeConfig {
            memory_limit: 2 * 1024 * 1024,
            ..config()
        },
        "const a = []; for(;;) a.push(new Array(10000).fill('value'));",
        Cancellation::default(),
    );
    assert!(!result.error.is_empty());
    assert!(host.canceled);
}

#[test]
fn aggregate_argument_bytes_are_bounded_and_released_after_completion() {
    let mut host = Host::default();
    let cfg = CodeConfig {
        max_pending_bytes: 20,
        ..config()
    };
    let result = run_code(
        &mut host,
        &cfg,
        "echo({text:'1234567890'}); echo({});",
        Cancellation::default(),
    );
    assert!(result.error.contains("byte budget"), "{result:?}");
    assert_eq!(host.calls.len(), 0);
    let result = run_code(
        &mut host,
        &cfg,
        "for(let i=0;i<100;i++) await echo({x:1}); return 'ok';",
        Cancellation::default(),
    );
    assert!(result.error.is_empty(), "{result:?}");
    assert_eq!(result.calls.len(), 100);
    let result = run_code(
        &mut host,
        &config(),
        "await echo({text:'x'.repeat(40000)}); return 'ok';",
        Cancellation::default(),
    );
    assert!(result.error.is_empty(), "{result:?}");
    assert!(result.calls[0].arguments.len() < 16100);
    assert!(result.calls[0].arguments.contains("bytes left out"));
}

#[test]
fn host_start_panics_do_not_cross_ffi() {
    #[derive(Default)]
    struct Broken;
    impl CodeHost for Broken {
        fn start(&mut self, _: HostCall) -> Result<(), String> {
            panic!("host panic")
        }
        fn wait(&mut self, _: Duration) -> Option<Completion> {
            None
        }
        fn cancel(&mut self) {}
    }
    let result = run_code(
        &mut Broken,
        &config(),
        "try { await echo({}) } catch(e) { return e.message }",
        Cancellation::default(),
    );
    assert!(result.error.is_empty(), "{result:?}");
    assert!(result.value.contains("internal error"));
}

#[test]
fn cancellation_during_a_live_sleep_releases_all_roots() {
    let cancel = Cancellation::default();
    let parent = cancel.clone();
    let thread = std::thread::spawn(move || {
        std::thread::park_timeout(Duration::from_millis(10));
        parent.cancel();
    });
    let mut host = Host::default();
    let result = run_code(&mut host, &config(), "await sleep(100000)", cancel);
    thread.join().unwrap();
    assert!(result.error.contains("canceled"), "{result:?}");
    assert!(host.canceled);
}

#[test]
fn host_wait_panics_clear_vm_roots_before_unwinding() {
    #[derive(Default)]
    struct BrokenWait;
    impl CodeHost for BrokenWait {
        fn start(&mut self, _: HostCall) -> Result<(), String> {
            Ok(())
        }
        fn wait(&mut self, _: Duration) -> Option<Completion> {
            panic!("wait panic")
        }
        fn cancel(&mut self) {}
    }
    let result = run_code(
        &mut BrokenWait,
        &config(),
        "await echo({})",
        Cancellation::default(),
    );
    assert!(result.error.contains("internal error"), "{result:?}");
}
