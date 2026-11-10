use crate::{CodeHost, Completion, HostCall, output::clip};
use kou_domain::Json;
use kou_tools::{
    fileops::{self, EditOptions, ReadOptions},
    process::{CancelToken, ProcessRequest, run_process},
};
use serde::Serialize;
use serde_json::{Value, value::RawValue};
use std::{
    collections::{BTreeMap, VecDeque},
    ffi::OsString,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Clone, Debug)]
pub struct CommandTool {
    pub name: String,
    pub command: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub directory: PathBuf,
}

#[derive(Clone, Debug)]
pub struct NativeConfig {
    pub shell: PathBuf,
    pub directory: PathBuf,
    pub environment: Option<BTreeMap<OsString, OsString>>,
    pub tools: Vec<CommandTool>,
    pub skills: BTreeMap<String, PathBuf>,
    pub call_id: String,
    pub command_timeout: Option<Duration>,
    pub max_output: usize,
    pub workers: usize,
    pub max_pending: usize,
    pub max_argument_bytes: usize,
    pub termination_grace: Duration,
}
impl Default for NativeConfig {
    fn default() -> Self {
        Self {
            shell: "/bin/sh".into(),
            directory: PathBuf::new(),
            environment: None,
            tools: Vec::new(),
            skills: BTreeMap::new(),
            call_id: String::new(),
            command_timeout: Some(Duration::from_secs(600)),
            max_output: 40_000,
            workers: 4,
            max_pending: 64,
            max_argument_bytes: 16 << 20,
            termination_grace: Duration::from_secs(5),
        }
    }
}

struct Finished {
    completion: Completion,
    argument_bytes: usize,
}
#[derive(Default)]
struct Queues {
    jobs: VecDeque<HostCall>,
    done: VecDeque<Finished>,
    admitted: usize,
    argument_bytes: usize,
    canceled: bool,
}
struct Shared {
    queues: Mutex<Queues>,
    changed: Condvar,
    cancel: CancelToken,
    config: NativeConfig,
}

/// Per-execution native capabilities. Jobs and completed results have count
/// bounds; admitted arguments also have a byte budget. Filesystem operations
/// may still have bounded transient decode/edit buffers. This is not an RSS
/// hard limit or a sandbox. Dropping the host joins every worker.
pub struct NativeHost {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}
impl NativeHost {
    pub fn new(config: NativeConfig) -> Result<Self, String> {
        if !(1..=8).contains(&config.workers)
            || config.max_pending == 0
            || config.max_argument_bytes == 0
            || config.max_output == 0
            || config.max_output > 1_000_000
            || !config.shell.is_absolute()
        {
            return Err("invalid native host limits or shell path".into());
        }
        let shared = Arc::new(Shared {
            queues: Mutex::new(Queues::default()),
            changed: Condvar::new(),
            cancel: CancelToken::default(),
            config,
        });
        let mut host = Self {
            shared,
            workers: Vec::new(),
        };
        for n in 0..host.shared.config.workers {
            let shared = host.shared.clone();
            host.workers.push(
                thread::Builder::new()
                    .name(format!("kou-tool-{n}"))
                    .spawn(move || worker(shared))
                    .map_err(|err| err.to_string())?,
            );
        }
        Ok(host)
    }
    pub fn tool_names(&self) -> Vec<String> {
        self.shared
            .config
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect()
    }
    pub fn shutdown(&mut self) {
        self.cancel();
    }
}
impl Drop for NativeHost {
    fn drop(&mut self) {
        self.shutdown();
    }
}
impl CodeHost for NativeHost {
    fn start(&mut self, call: HostCall) -> Result<(), String> {
        let bytes = call.arguments.as_str().map_or(0, str::len);
        let mut queues = lock(&self.shared.queues);
        if queues.canceled {
            return Err("context canceled".into());
        }
        if queues.admitted >= self.shared.config.max_pending {
            return Err("native host queue is full".into());
        }
        if bytes
            > self
                .shared
                .config
                .max_argument_bytes
                .saturating_sub(queues.argument_bytes)
        {
            return Err("native host arguments exceed the byte budget".into());
        }
        queues.admitted += 1;
        queues.argument_bytes += bytes;
        queues.jobs.push_back(call);
        self.shared.changed.notify_all();
        Ok(())
    }
    fn record_arguments(&self, name: &str, arguments: &Json, plugin: bool) -> Option<String> {
        if plugin {
            return None;
        }
        let args: Vec<&RawValue> = serde_json::from_str(arguments.as_str()?).ok()?;
        let first: String = serde_json::from_str(args.first()?.get()).ok()?;
        Some(
            if matches!(name, "read" | "readText" | "write" | "edit" | "viewImage") {
                resolve(&self.shared.config.directory, &first)
                    .to_string_lossy()
                    .into_owned()
            } else {
                first
            },
        )
    }
    fn wait(&mut self, timeout: Duration) -> Option<Completion> {
        let queues = lock(&self.shared.queues);
        let (mut queues, _) = self
            .shared
            .changed
            .wait_timeout_while(queues, timeout, |q| q.done.is_empty() && !q.canceled)
            .unwrap_or_else(|e| e.into_inner());
        let finished = queues.done.pop_front()?;
        queues.admitted -= 1;
        queues.argument_bytes -= finished.argument_bytes;
        self.shared.changed.notify_all();
        Some(finished.completion)
    }
    fn cancel(&mut self) {
        self.shared.cancel.cancel();
        let mut queues = lock(&self.shared.queues);
        queues.canceled = true;
        queues.jobs.clear();
        queues.done.clear();
        queues.admitted = 0;
        queues.argument_bytes = 0;
        self.shared.changed.notify_all();
        drop(queues);
        // run_code's cleanup must not leave native work alive just because an
        // external Rc still owns the host after the VM has returned.
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
fn worker(shared: Arc<Shared>) {
    loop {
        let job = {
            let queues = lock(&shared.queues);
            let mut queues = shared
                .changed
                .wait_while(queues, |q| q.jobs.is_empty() && !q.canceled)
                .unwrap_or_else(|e| e.into_inner());
            if queues.canceled {
                return;
            }
            queues.jobs.pop_front().expect("queued job")
        };
        let ticket = job.ticket;
        let argument_bytes = job.arguments.as_str().map_or(0, str::len);
        let completion = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            execute(&shared.config, &shared.cancel, job)
        }))
        .unwrap_or_else(|_| failed(ticket, "internal error in a native tool".into()));
        let queues = lock(&shared.queues);
        let mut queues = shared
            .changed
            .wait_while(queues, |q| {
                q.done.len() >= shared.config.workers && !q.canceled
            })
            .unwrap_or_else(|e| e.into_inner());
        if queues.canceled {
            return;
        }
        queues.done.push_back(Finished {
            completion,
            argument_bytes,
        });
        shared.changed.notify_all();
    }
}

fn failed(ticket: u64, error: String) -> Completion {
    Completion {
        ticket,
        result: Err(error),
        output: String::new(),
        exit_code: None,
    }
}
fn encode(value: &impl Serialize) -> Result<Json, String> {
    let encoded = serde_json::to_string(value).map_err(|e| e.to_string())?;
    if encoded.len() > 16 << 20 {
        return Err("native host result exceeds 16 MiB".into());
    }
    Json::parse(&encoded).map_err(|e| e.to_string())
}
fn text(ticket: u64, value: Result<String, String>) -> Completion {
    match value {
        Ok(output) => Completion {
            ticket,
            result: encode(&output),
            output: clip(&output, 16_000),
            exit_code: None,
        },
        Err(error) => failed(ticket, error),
    }
}
fn resolve(directory: &Path, path: &str) -> PathBuf {
    if path.is_empty() || directory.as_os_str().is_empty() || Path::new(path).is_absolute() {
        path.into()
    } else {
        directory.join(path)
    }
}
fn argument(args: &[Value], n: usize) -> Result<&str, String> {
    args.get(n)
        .and_then(Value::as_str)
        .ok_or_else(|| "invalid native tool arguments".into())
}
fn number(args: &[Value], n: usize, key: &str) -> Option<f64> {
    let value = args.get(n)?.get(key)?;
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|n| n.is_finite())
}
fn plural(n: usize) -> String {
    format!("{n} {}", if n == 1 { "line" } else { "lines" })
}

fn execute(config: &NativeConfig, cancel: &CancelToken, call: HostCall) -> Completion {
    if cancel.is_canceled() {
        return failed(call.ticket, "context canceled".into());
    }
    if call.plugin {
        return plugin(config, cancel, call);
    }
    let args: Vec<Value> = match call.arguments.decode() {
        Ok(value) => value,
        Err(e) => return failed(call.ticket, e.to_string()),
    };
    let action = || -> Result<String, String> {
        let first = argument(&args, 0)?;
        let path = resolve(&config.directory, first);
        match call.name.as_str() {
            "read" => fileops::read_interruptible(
                &path,
                ReadOptions {
                    offset: number(&args, 1, "offset").unwrap_or(0.0) as i64,
                    limit: number(&args, 1, "limit").unwrap_or(0.0) as i64,
                    ..Default::default()
                },
                || cancel.is_canceled(),
            )
            .map(|v| v.text)
            .map_err(|e| e.to_string()),
            "readText" => read_text(&path),
            "write" => {
                let value =
                    fileops::write(&path, argument(&args, 1)?).map_err(|e| e.to_string())?;
                Ok(if value.created {
                    format!(
                        "Wrote {}: {}, {} bytes.",
                        path.display(),
                        plural(value.lines),
                        value.bytes
                    )
                } else {
                    format!(
                        "Replaced {} (it had {}): now {}, {} bytes.",
                        path.display(),
                        plural(value.previous_lines),
                        plural(value.lines),
                        value.bytes
                    )
                })
            }
            "edit" => {
                let replace_all = args
                    .get(3)
                    .and_then(|v| v.get("replaceAll"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let value = fileops::edit(
                    &path,
                    argument(&args, 1)?,
                    argument(&args, 2)?,
                    EditOptions { replace_all },
                )
                .map_err(|e| edit_error(e.to_string()))?;
                let replaced = if value.replacements == 1 {
                    format!("replaced 1 occurrence at line {}", value.line)
                } else {
                    format!(
                        "replaced {} occurrences, the first at line {}",
                        value.replacements, value.line
                    )
                };
                Ok(format!(
                    "Edited {}: {replaced}.\n{}",
                    path.display(),
                    value.snippet
                ))
            }
            "skill" => {
                let path = config.skills.get(first).ok_or_else(|| {
                    format!(
                        "skill {first:?} is not registered; the skills are: {}",
                        config.skills.keys().cloned().collect::<Vec<_>>().join(", ")
                    )
                })?;
                read_text(path)
            }
            "applyPatch" => kou_tools::patch::apply_patch(first, &config.directory)
                .map(|value| value.summary())
                .map_err(|error| error.to_string()),
            "transcriptSearch" => Err("no transcript is available".into()),
            "viewImage" => Err("viewImage is not available in this run".into()),
            _ => Err(format!(
                "{} is not available in this native host yet",
                call.name
            )),
        }
    };
    if call.name == "bash" {
        let command = match argument(&args, 0) {
            Ok(v) => v,
            Err(e) => return failed(call.ticket, e),
        };
        let timeout = match number(&args, 1, "timeout") {
            Some(0.0) => None,
            Some(n) if n > 0.0 => match Duration::try_from_secs_f64(n) {
                Ok(n) => Some(n),
                Err(_) => return failed(call.ticket, "invalid command timeout".into()),
            },
            _ => config.command_timeout,
        };
        let limit = number(&args, 1, "maxOutputLength")
            .filter(|n| *n > 0.0)
            .map_or(config.max_output, |n| n.min(1_000_000.0) as usize);
        return command_completion(
            call.ticket,
            config,
            cancel,
            &config.shell,
            &config.directory,
            config.environment.clone(),
            command,
            timeout,
            limit,
            None,
        );
    }
    text(call.ticket, action())
}

fn read_text(path: &Path) -> Result<String, String> {
    const CAP: u64 = 4 << 20;
    let info = std::fs::metadata(path).map_err(|e| path_error("stat", path, e))?;
    if info.len() > CAP {
        return Err(format!(
            "{} is {} bytes; readText reads up to {CAP}",
            path.display(),
            info.len()
        ));
    }
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(CAP + 1).read_to_end(&mut bytes))
        .map_err(|e| path_error("read", path, e))?;
    if bytes.len() as u64 > CAP {
        return Err(format!(
            "{} grew past the readText limit of {CAP}",
            path.display()
        ));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
fn path_error(operation: &str, path: &Path, error: std::io::Error) -> String {
    use std::io::ErrorKind;
    let message = match error.kind() {
        ErrorKind::NotFound => "no such file or directory".to_owned(),
        ErrorKind::PermissionDenied => "permission denied".to_owned(),
        ErrorKind::IsADirectory => "is a directory".to_owned(),
        ErrorKind::NotADirectory => "not a directory".to_owned(),
        _ => error.to_string(),
    };
    format!("{operation} {}: {message}", path.display())
}
fn edit_error(mut message: String) -> String {
    for (from, to) in [
        ("old_string", "oldString"),
        ("new_string", "newString"),
        ("set replace_all", "pass { replaceAll: true }"),
        ("Write creates", "write() creates"),
        ("as Read shows", "as read() shows"),
        ("; use Bash", "; use bash()"),
    ] {
        message = message.replace(from, to);
    }
    message
}
fn shell_word(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}
fn plugin(config: &NativeConfig, cancel: &CancelToken, call: HostCall) -> Completion {
    let Some(definition) = config.tools.iter().find(|tool| tool.name == call.name) else {
        return failed(
            call.ticket,
            format!("tool {:?} is not registered", call.name),
        );
    };
    let arguments = call.arguments.as_str().unwrap_or("{}");
    let script = format!(
        "exec{} <<'KOU_CONVEYOR_TOOL_ARGUMENTS'\n{}\nKOU_CONVEYOR_TOOL_ARGUMENTS\n",
        definition
            .command
            .iter()
            .map(|arg| format!(" {}", shell_word(arg)))
            .collect::<String>(),
        arguments.replace('\n', " ")
    );
    let mut environment: BTreeMap<OsString, OsString> = config
        .environment
        .clone()
        .unwrap_or_else(|| std::env::vars_os().collect());
    environment.extend(
        definition
            .environment
            .iter()
            .map(|(key, value)| (key.into(), value.into())),
    );
    environment.insert(
        "KOU_CONVEYOR_TOOL_NAME".into(),
        definition.name.clone().into(),
    );
    if !config.call_id.is_empty() {
        environment.insert(
            "KOU_CONVEYOR_TOOL_CALL_ID".into(),
            format!("{}-{}", config.call_id, call.index).into(),
        );
    }
    let directory = if definition.directory.as_os_str().is_empty() {
        &config.directory
    } else {
        &definition.directory
    };
    command_completion(
        call.ticket,
        config,
        cancel,
        Path::new("/bin/sh"),
        directory,
        Some(environment),
        &script,
        config.command_timeout,
        config.max_output,
        Some(&definition.name),
    )
}

#[allow(clippy::too_many_arguments)]
fn command_completion(
    ticket: u64,
    config: &NativeConfig,
    cancel: &CancelToken,
    shell: &Path,
    directory: &Path,
    environment: Option<BTreeMap<OsString, OsString>>,
    command: &str,
    timeout: Option<Duration>,
    limit: usize,
    plugin: Option<&str>,
) -> Completion {
    let mut request = ProcessRequest::new(shell);
    request.arguments = vec!["-c".into(), command.into()];
    request.directory = directory.into();
    request.environment = environment;
    request.timeout = timeout;
    request.output_limit = limit;
    request.termination_grace = config.termination_grace;
    match run_process(&request, cancel) {
        Err(e) => failed(ticket, e.to_string()),
        Ok(value) if value.canceled => failed(ticket, "context canceled".into()),
        Ok(value) => {
            let output = value.text();
            let result = if let Some(name) = plugin {
                if value.exit_code != 0 || value.timed_out {
                    Err(format!("{name} failed:\n{}", clip(output.trim(), 4000)))
                } else {
                    encode(&output)
                }
            } else {
                encode(&value)
            };
            Completion {
                ticket,
                result,
                output: clip(&output, 16_000),
                exit_code: Some(value.exit_code),
            }
        }
    }
}
