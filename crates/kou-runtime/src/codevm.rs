use crate::output::{BoundedOutput, clip};
use kou_domain::Json;
use rquickjs::{Context, Ctx, Exception, Function, Object, Promise, Runtime, Value, function::Opt};
use serde::Serialize;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release)
    }
    pub fn is_canceled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Clone, Debug)]
pub struct CodeConfig {
    pub tools: Vec<String>,
    pub timeout: Duration,
    pub max_calls: usize,
    pub max_pending: usize,
    pub max_pending_bytes: usize,
    pub max_output: usize,
    pub memory_limit: usize,
    pub stack_limit: usize,
}
impl Default for CodeConfig {
    fn default() -> Self {
        Self {
            tools: Vec::new(),
            timeout: Duration::from_secs(1800),
            max_calls: 200,
            max_pending: 256,
            max_pending_bytes: 16 * 1024 * 1024,
            max_output: 40000,
            memory_limit: 128 * 1024 * 1024,
            stack_limit: 512 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HostCall {
    pub ticket: u64,
    pub index: usize,
    pub name: String,
    pub arguments: Json,
    pub plugin: bool,
}
#[derive(Debug)]
pub struct Completion {
    pub ticket: u64,
    pub result: Result<Json, String>,
    pub output: String,
    pub exit_code: Option<i32>,
}

/// start must enqueue without blocking; wait must respect its timeout. The host
/// owns native work and must cancel that work when cancel is called. No JS value
/// is ever sent to a worker. This is a capability boundary, not an OS sandbox.
pub trait CodeHost {
    fn start(&mut self, call: HostCall) -> Result<(), String>;
    // Optional legacy display label; it must not perform I/O or run user code.
    fn record_arguments(&self, _name: &str, _arguments: &Json, _plugin: bool) -> Option<String> {
        None
    }
    fn wait(&mut self, timeout: Duration) -> Option<Completion>;
    fn cancel(&mut self);
}

#[derive(Clone, Debug, Serialize)]
pub struct Call {
    pub name: String,
    pub arguments: String,
    pub output: String,
    pub error: String,
    pub exit_code: Option<i32>,
    pub running: bool,
}
#[derive(Default, Debug, Serialize)]
pub struct Execution {
    pub logs: String,
    pub value: String,
    pub error: String,
    pub calls: Vec<Call>,
}
struct Pending<'js> {
    resolve: Function<'js>,
    reject: Function<'js>,
    index: Option<usize>,
    wake: Option<Instant>,
    argument_bytes: usize,
}
struct State<'js> {
    pending: BTreeMap<u64, Pending<'js>>,
    pending_bytes: usize,
    next: u64,
    calls: Vec<Call>,
    logs: BoundedOutput,
}

struct RootCleanup<'js>(Rc<RefCell<State<'js>>>);
impl Drop for RootCleanup<'_> {
    fn drop(&mut self) {
        self.0.borrow_mut().pending.clear()
    }
}

/// Construct the owned host on the VM's worker thread. Only owned host messages,
/// not the Rc host or QuickJS handles, may cross into other workers.
pub fn run_code<H: CodeHost + 'static>(
    host: Rc<RefCell<H>>,
    config: &CodeConfig,
    code: &str,
    cancel: Cancellation,
) -> Execution {
    struct Cancel<H: CodeHost>(Rc<RefCell<H>>);
    impl<H: CodeHost> Drop for Cancel<H> {
        fn drop(&mut self) {
            self.0.borrow_mut().cancel()
        }
    }
    let _guard = Cancel(host.clone());
    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        execute(host, config, code, cancel)
    }));
    match run {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => Execution {
            error,
            ..Execution::default()
        },
        Err(_) => Execution {
            error: "internal error in the code runtime".into(),
            ..Execution::default()
        },
    }
}

fn execute<H: CodeHost + 'static>(
    host: Rc<RefCell<H>>,
    config: &CodeConfig,
    code: &str,
    cancel: Cancellation,
) -> Result<Execution, String> {
    if config.max_calls == 0
        || config.max_pending == 0
        || config.memory_limit == 0
        || config.stack_limit == 0
        || code.len() > 1024 * 1024
    {
        return Err("invalid Code VM limits or code exceeds 1 MiB".into());
    }
    let started = Instant::now();
    let deadline = started
        .checked_add(config.timeout)
        .ok_or("invalid Code VM timeout")?;
    let runtime = Runtime::new().map_err(|e| e.to_string())?;
    runtime.set_memory_limit(config.memory_limit);
    runtime.set_max_stack_size(config.stack_limit);
    let interrupt = cancel.clone();
    runtime.set_interrupt_handler(Some(Box::new(move || {
        interrupt.is_canceled() || Instant::now() >= deadline
    })));
    let context = Context::full(&runtime).map_err(|e| e.to_string())?;
    context.with(|ctx| {
        let state = Rc::new(RefCell::new(State { pending: BTreeMap::new(), pending_bytes: 0, next: 0, calls: Vec::new(), logs: BoundedOutput::new(config.max_output) }));
        let _cleanup = RootCleanup(state.clone());
        let run = (|| -> Result<String, String> {
            let log_state = state.clone();
            let log = Function::new(ctx.clone(), move |line: String| { log_state.borrow_mut().logs.write(line.as_bytes()) }).map_err(|e| e.to_string())?;
            let native = native_call(ctx.clone(), state.clone(), host.clone(), config.clone()).map_err(|e| e.to_string())?;
            let setup: Function = ctx.eval(include_str!("bindings.js")).map_err(|e| describe(&ctx, e))?;
            let bindings: Object = setup.call((native, log, config.tools.clone())).map_err(|e| describe(&ctx, e))?;
            let format: Function = bindings.get("format").map_err(|e| e.to_string())?;
            let thrown: Function = bindings.get("thrown").map_err(|e| e.to_string())?;
            let promise: Promise = ctx.eval(format!("(async () => {{{code}\n}})()")).map_err(|e| describe(&ctx, e))?;
            loop {
                if cancel.is_canceled() { return Err("the run was stopped: it was canceled".into()) }
                if Instant::now() >= deadline { return Err("the run was stopped: it ran longer than its timeout".into()) }
                // Drive one job at a time so a self-rescheduling microtask cannot
                // postpone host completions, timers or cancellation forever.
                let mut jobs_pending = ctx.execute_pending_job();
                if let Some(result) = promise.result::<Value>() {
                    let value = match result {
                        Ok(value) => value,
                        Err(rquickjs::Error::Exception) => {
                            return Err(thrown.call((ctx.catch(),)).unwrap_or_else(|e| describe(&ctx, e)));
                        }
                        Err(e) => return Err(describe(&ctx, e)),
                    };
                    let text = if value.is_undefined() { String::new() } else { format.call((value, "  ")).map_err(|e| describe(&ctx, e))? };
                    return Ok(text);
                }
                let now = Instant::now();
                let due: Vec<_> = state.borrow().pending.iter().filter_map(|(id, p)| p.wake.filter(|wake| *wake <= now).map(|_| *id)).collect();
                if !due.is_empty() { jobs_pending = true; }
                for id in due {
                    let pending = state.borrow_mut().pending.remove(&id).expect("due timer");
                    pending.resolve.call::<_, ()>((Value::new_undefined(ctx.clone()),)).map_err(|e| describe(&ctx, e))?;
                }
                if state.borrow().pending.is_empty() && !jobs_pending {
                    return Err("the code waits for a promise that nothing will settle: none of the calls it made is still running".into());
                }
                let wait = if jobs_pending { Duration::ZERO } else {
                    state.borrow().pending.values().filter_map(|p| p.wake).min().unwrap_or(deadline).saturating_duration_since(Instant::now()).min(Duration::from_millis(20)).min(deadline.saturating_duration_since(Instant::now()))
                };
                let completion = host.borrow_mut().wait(wait);
                if let Some(completion) = completion {
                    let Some(pending) = state.borrow_mut().pending.remove(&completion.ticket) else { continue };
                    let Some(index) = pending.index else { return Err("host completed a VM timer".into()) };
                    let result = match completion.result {
                        Ok(value) if value.as_str().map_or(0, str::len) > 16 * 1024 * 1024 => Err("host result exceeds 16 MiB".to_owned()),
                        Err(error) if error.len() > 16 * 1024 * 1024 => Err("host error exceeds 16 MiB".to_owned()),
                        result => result,
                    };
                    let error = result.as_ref().err().cloned();
                    let mut state = state.borrow_mut();
                    state.pending_bytes -= pending.argument_bytes;
                    let call = &mut state.calls[index];
                    call.running = false;
                    call.output = clip(&completion.output, 16_000);
                    call.error = clip(error.as_deref().unwrap_or(""), 16_000);
                    call.exit_code = completion.exit_code;
                    drop(state);
                    match result {
                        Ok(value) => {
                            let value = ctx.json_parse(value.as_str().unwrap_or("null")).map_err(|e| describe(&ctx, e))?;
                            pending.resolve.call::<_, ()>((value,)).map_err(|e| describe(&ctx, e))?;
                        }
                        Err(message) => {
                            let error = host_exception(ctx.clone(), &message).map_err(|e| describe(&ctx, e))?;
                            pending.reject.call::<_, ()>((error,)).map_err(|e| describe(&ctx, e))?;
                        }
                    }
                }
            }
        })();
        let mut state = state.borrow_mut();
        // Rust closures stored in JS functions are not traced by the JS GC.
        // Drop every rooted resolver before the context/runtime can go away.
        state.pending.clear();
        let mut result = Execution { logs: state.logs.render(), calls: std::mem::take(&mut state.calls), ..Execution::default() };
        match run { Ok(value) => result.value = clip(&value, config.max_output), Err(error) => result.error = clip(&error, config.max_output) }
        if cancel.is_canceled() { result.error = "the run was stopped: it was canceled".into(); result.value.clear(); }
        else if Instant::now() >= deadline { result.error = "the run was stopped: it ran longer than its timeout".into(); result.value.clear(); }
        for call in &mut result.calls {
            if call.running { call.running = false; call.error = "the run ended before it finished".into(); }
        }
        Ok(result)
    })
}

fn native_call<'js, H: CodeHost + 'static>(
    ctx: Ctx<'js>,
    state: Rc<RefCell<State<'js>>>,
    host: Rc<RefCell<H>>,
    config: CodeConfig,
) -> rquickjs::Result<Function<'js>> {
    Function::new(
        ctx,
        move |ctx: Ctx<'js>,
              name: String,
              arguments: String,
              plugin: Opt<bool>|
              -> rquickjs::Result<Promise<'js>> {
            let (promise, resolve, reject) = Promise::new(&ctx)?;
            let mut current = state.borrow_mut();
            if current.pending.len() >= config.max_pending {
                return Err(Exception::throw_message(
                    &ctx,
                    "too many pending Code VM calls",
                ));
            }
            if !name.is_empty() && current.calls.len() >= config.max_calls {
                drop(current);
                reject.call::<_, ()>((host_exception(
                    ctx,
                    &format!(
                        "the run made {} tool calls, the most a run may make",
                        config.max_calls
                    ),
                )?,))?;
                return Ok(promise);
            }
            if arguments.len() > 4 * 1024 * 1024 {
                return Err(Exception::throw_message(
                    &ctx,
                    "Code VM arguments exceed 4 MiB",
                ));
            }
            if !name.is_empty()
                && arguments.len()
                    > config
                        .max_pending_bytes
                        .saturating_sub(current.pending_bytes)
            {
                return Err(Exception::throw_message(
                    &ctx,
                    "pending Code VM arguments exceed the byte budget",
                ));
            }
            let args = Json::parse(&arguments)
                .map_err(|_| Exception::throw_type(&ctx, "arguments must be valid JSON"))?;
            current.next += 1;
            let ticket = current.next;
            if name.is_empty() {
                let delay: Vec<Option<f64>> = args.decode().unwrap_or_default();
                let millis = delay
                    .first()
                    .and_then(|n| *n)
                    .filter(|n| n.is_finite() && *n >= 0.0)
                    .unwrap_or(0.0);
                let delay =
                    Duration::from_secs_f64((millis / 1000.0).min(config.timeout.as_secs_f64()));
                current.pending.insert(
                    ticket,
                    Pending {
                        resolve,
                        reject,
                        index: None,
                        wake: Some(Instant::now() + delay),
                        argument_bytes: 0,
                    },
                );
            } else {
                let index = current.calls.len();
                let label = host
                    .borrow()
                    .record_arguments(&name, &args, plugin.0.unwrap_or(false));
                current.calls.push(Call {
                    name: name.clone(),
                    arguments: clip(label.as_deref().unwrap_or(&arguments), 16_000),
                    output: String::new(),
                    error: String::new(),
                    exit_code: None,
                    running: true,
                });
                current.pending_bytes += arguments.len();
                current.pending.insert(
                    ticket,
                    Pending {
                        resolve,
                        reject,
                        index: Some(index),
                        wake: None,
                        argument_bytes: arguments.len(),
                    },
                );
                drop(current);
                let start = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    host.borrow_mut().start(HostCall {
                        ticket,
                        index,
                        name,
                        arguments: args,
                        plugin: plugin.0.unwrap_or(false),
                    })
                }));
                let error = match start {
                    Ok(Ok(())) => None,
                    Ok(Err(e)) => Some(e),
                    Err(_) => Some("internal error starting a host call".into()),
                };
                if let Some(error) = error {
                    let pending = state
                        .borrow_mut()
                        .pending
                        .remove(&ticket)
                        .expect("pending call");
                    let mut current = state.borrow_mut();
                    current.pending_bytes -= pending.argument_bytes;
                    current.calls[index].running = false;
                    current.calls[index].error = clip(&error, 16_000);
                    drop(current);
                    let exception = host_exception(ctx.clone(), &error)?;
                    pending.reject.call::<_, ()>((exception,))?;
                }
            }
            Ok(promise)
        },
    )
}

fn host_exception<'js>(ctx: Ctx<'js>, message: &str) -> rquickjs::Result<Exception<'js>> {
    let error = Exception::from_message(ctx, message)?;
    error.as_object().set("name", "GoError")?;
    Ok(error)
}

fn describe(ctx: &Ctx<'_>, error: rquickjs::Error) -> String {
    match rquickjs::CaughtError::from_error(ctx, error) {
        rquickjs::CaughtError::Exception(value) => value
            .message()
            .unwrap_or_else(|| "JavaScript exception".into()),
        rquickjs::CaughtError::Value(value) => format!("uncaught {value:?}"),
        rquickjs::CaughtError::Error(error) => error.to_string(),
    }
}
