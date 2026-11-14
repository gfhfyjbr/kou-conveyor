//! What the live tests share: the real Go runner, and a local stand-in for
//! the model that speaks the Responses API's stream. No test here reaches a
//! real provider: the runner is pointed at `127.0.0.1` and given a key of no
//! account.

#![allow(dead_code)]

use kou_thread::{Backend, Thread, Update};
use serde_json::{Value, json};
use std::{
    io::{BufRead as _, BufReader, Read as _, Write as _},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

/// The repository this crate lives in.
pub fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("the repository")
}

/// A runner built from the repository's sources as they are now, or `None`
/// with the reason when there is no Go toolchain to build it with.
/// `$KOU_RUNNER` names one to use instead.
pub fn fresh_runner() -> Option<PathBuf> {
    static RUNNER: OnceLock<Option<PathBuf>> = OnceLock::new();
    RUNNER
        .get_or_init(|| {
            if let Some(path) = std::env::var_os("KOU_RUNNER").filter(|path| !path.is_empty()) {
                return Some(PathBuf::from(path));
            }
            let target = Path::new(env!("CARGO_TARGET_TMPDIR"))
                .join("kou-native-runner")
                .join("kou-conveyor-runner");
            let built = Command::new("go")
                .args(["build", "-o"])
                .arg(&target)
                .arg("./cmd/kou-conveyor-runner")
                .current_dir(repository())
                .output();
            match built {
                Ok(output) if output.status.success() => Some(target),
                Ok(output) => {
                    eprintln!(
                        "the runner did not build: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                    None
                }
                Err(error) => {
                    eprintln!("no Go toolchain to build the runner with: {error}");
                    None
                }
            }
        })
        .clone()
}

/// The runner the repository keeps in `bin/`, which may be an older build.
pub fn built_runner() -> Option<PathBuf> {
    Some(repository().join("bin").join("kou-conveyor-runner")).filter(|path| path.is_file())
}

/// One piece of a streamed response.
pub enum Piece {
    /// Text of the answer, as a delta of the output item at this index.
    Text(usize, &'static str),
    /// Reasoning, as a delta of the output item at this index.
    Reasoning(usize, &'static str),
    /// A pause, so that what came before goes out as a line of its own.
    Pause,
}

/// What the stand-in answers one request with.
pub struct Answer {
    /// What streams before the response is complete.
    pub pieces: Vec<Piece>,
    /// The output items of the complete response.
    pub output: Value,
    /// An HTTP status other than 200 answers with this body instead.
    pub status: u16,
}

impl Answer {
    /// A complete response with these output items.
    pub fn of(pieces: Vec<Piece>, output: Value) -> Self {
        Self {
            pieces,
            output,
            status: 200,
        }
    }

    /// A refusal of the request.
    pub fn refused(status: u16) -> Self {
        Self {
            pieces: Vec::new(),
            output: Value::Null,
            status,
        }
    }
}

/// An assistant message, as the Responses API states it.
pub fn message(id: &str, text: &str) -> Value {
    json!({"id": id, "type": "message", "role": "assistant", "status": "completed",
        "content": [{"type": "output_text", "text": text, "annotations": [], "logprobs": []}]})
}

/// A reasoning item with a summary.
pub fn reasoning(id: &str, summary: &str) -> Value {
    json!({"id": id, "type": "reasoning", "status": "completed",
        "summary": [{"type": "summary_text", "text": summary}]})
}

/// A call of a tool.
pub fn call(call_id: &str, name: &str, arguments: Value) -> Value {
    json!({"id": format!("fc-{call_id}"), "type": "function_call", "call_id": call_id,
        "name": name, "arguments": arguments.to_string(), "status": "completed"})
}

/// A local stand-in for a provider of the Responses API.
pub struct Provider {
    /// The base URL to hand the runner.
    pub base_url: String,
    /// The bodies of the requests for a response, in order.
    pub requests: Arc<Mutex<Vec<Value>>>,
}

impl Provider {
    /// Serves until the process ends. `answer` sees each request's body and
    /// how many came before it.
    pub fn start(answer: impl Fn(&Value, usize) -> Answer + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let base_url = format!("http://{}/v1", listener.local_addr().expect("an address"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let answer = Arc::new(answer);
        {
            let requests = requests.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let (requests, answer) = (requests.clone(), answer.clone());
                    std::thread::spawn(move || {
                        let _ = serve(stream, &requests, &*answer);
                    });
                }
            });
        }
        Self { base_url, requests }
    }
}

fn serve(
    stream: TcpStream,
    requests: &Mutex<Vec<Value>>,
    answer: &(dyn Fn(&Value, usize) -> Answer + Send + Sync),
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut stream = stream;
    loop {
        let mut request_line = String::new();
        if reader.read_line(&mut request_line)? == 0 {
            return Ok(());
        }
        let mut length = 0;
        let mut chunked = false;
        loop {
            let mut header = String::new();
            reader.read_line(&mut header)?;
            let header = header.trim().to_ascii_lowercase();
            if header.is_empty() {
                break;
            }
            if let Some(value) = header.strip_prefix("content-length:") {
                length = value.trim().parse().unwrap_or(0);
            }
            chunked |= header == "transfer-encoding: chunked";
        }
        let mut body = Vec::new();
        if chunked {
            loop {
                let mut size = String::new();
                reader.read_line(&mut size)?;
                let size = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
                let start = body.len();
                body.resize(start + size + 2, 0);
                reader.read_exact(&mut body[start..])?;
                body.truncate(start + size);
                if size == 0 {
                    break;
                }
            }
        } else {
            body.resize(length, 0);
            reader.read_exact(&mut body)?;
        }
        if request_line.starts_with("GET ") && request_line.contains("/models") {
            let models = r#"{"object":"list","data":[{"id":"stub-model","owned_by":"kou-test","created":1}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{models}",
                models.len()
            )?;
            continue;
        }
        if !request_line.starts_with("POST ") || !request_line.contains("/responses") {
            write!(
                stream,
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n"
            )?;
            continue;
        }
        let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let before = {
            let mut requests = requests.lock().expect("the requests");
            requests.push(body.clone());
            requests.len() - 1
        };
        let answer = answer(&body, before);
        if answer.status != 200 {
            let error = r#"{"error":{"code":"invalid_api_key","message":"the stand-in refuses this request","type":"authentication_error"}}"#;
            write!(
                stream,
                "HTTP/1.1 {} Refused\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{error}",
                answer.status,
                error.len()
            )?;
            continue;
        }
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n"
        )?;
        let mut event = |event: Value| -> std::io::Result<()> {
            write!(stream, "data: {event}\n\n")?;
            stream.flush()
        };
        for piece in &answer.pieces {
            match piece {
                Piece::Text(index, text) => event(json!({
                    "type": "response.output_text.delta", "output_index": index, "delta": text
                }))?,
                Piece::Reasoning(index, text) => event(json!({
                    "type": "response.reasoning_summary_text.delta", "output_index": index,
                    "summary_index": 0, "delta": text
                }))?,
                // Longer than the runner gathers the pieces of one line for.
                Piece::Pause => std::thread::sleep(Duration::from_millis(120)),
            }
        }
        event(json!({"type": "response.completed", "response": {
            "id": format!("resp-{before}"), "object": "response", "status": "completed",
            "output": answer.output,
            "usage": {"input_tokens": 100 + before, "input_tokens_details": {"cached_tokens": 0},
                "output_tokens": 7, "output_tokens_details": {"reasoning_tokens": 2},
                "total_tokens": 107 + before}
        }}))?;
        return Ok(());
    }
}

/// Whether a request follows the result of a tool call of the turn it
/// belongs to: one that came after the user's last message.
pub fn has_tool_result(request: &Value) -> bool {
    let Some(input) = request["input"].as_array() else {
        return false;
    };
    input
        .iter()
        .rev()
        .take_while(|item| item["role"] != "user")
        .any(|item| item["type"] == "function_call_output")
}

/// The text of the last message of the user's in a request.
pub fn last_user_text(request: &Value) -> String {
    let Some(input) = request["input"].as_array() else {
        return String::new();
    };
    input
        .iter()
        .rev()
        .find(|item| item["role"] == "user")
        .map(|item| match &item["content"] {
            Value::String(text) => text.clone(),
            content => content.to_string(),
        })
        .unwrap_or_default()
}

/// Folds a backend's updates into a thread, keeping them for inspection.
pub struct Follower {
    pub backend: Backend,
    pub thread: Thread,
    pub updates: Vec<Update>,
}

impl Follower {
    pub fn new(backend: Backend) -> Self {
        Self {
            backend,
            thread: Thread::new(),
            updates: Vec::new(),
        }
    }

    /// Takes updates in until one satisfies `done`, which is kept too.
    pub fn until(&mut self, what: &str, done: impl Fn(&Update) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let update = match self.backend.updates.try_recv() {
                Ok(update) => update,
                Err(async_channel::TryRecvError::Empty) => {
                    assert!(
                        Instant::now() < deadline,
                        "no {what} in time; the updates so far: {:#?}",
                        self.updates
                    );
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(async_channel::TryRecvError::Closed) => {
                    panic!("the backend closed before {what}: {:#?}", self.updates)
                }
            };
            self.thread.apply(update.clone());
            let found = done(&update);
            self.updates.push(update);
            if found {
                return;
            }
        }
    }

    /// Sends a prompt as an interface does: shown at once, then sent.
    pub fn prompt(&mut self, text: &str) {
        let blocks = vec![kou_thread::Block::Text(text.to_owned())];
        self.thread.push_user(blocks.clone());
        self.backend
            .commands
            .send_blocking(kou_thread::Command::Prompt(blocks))
            .expect("a backend");
    }

    /// Closes the session and waits for the backend to say it is gone.
    pub fn close(mut self) -> Vec<Update> {
        self.backend
            .commands
            .send_blocking(kou_thread::Command::Close)
            .expect("a backend");
        self.until("the end of the backend", |update| {
            matches!(update, Update::Closed(_))
        });
        self.updates
    }
}

/// A runner started apart from any front-end, as the web cockpit starts
/// them: its input, output and diagnostics are files of the run's directory,
/// it holds the session's lock itself, and it leads a session of its own.
#[cfg(unix)]
pub struct Detached {
    child: std::process::Child,
    dir: PathBuf,
}

#[cfg(unix)]
impl Detached {
    pub fn start(
        runner: &Path,
        project: &Path,
        setup: &kou_native::Setup,
        session: &str,
        prompt: &str,
    ) -> Self {
        use std::os::{fd::AsRawFd as _, unix::process::CommandExt as _};
        let sessions = project.join(".harness").join("sessions");
        let dir = sessions.join(".runs").join("run-of-a-cockpit");
        std::fs::create_dir_all(&dir).expect("the run's directory");
        std::fs::create_dir_all(sessions.join(".locks")).expect("the locks");
        let request = json!({
            "messages": [{"content": prompt, "message_id": "99999999-9999-4999-8999-999999999999"}],
            "session_id": session, "model": "stub-model", "thinking_level": "high",
        });
        std::fs::write(dir.join("in.jsonl"), format!("{request}\n")).expect("the request");
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(sessions.join(".locks").join(format!("{session}.lock")))
            .expect("the session's lock");
        // SAFETY: flock takes a descriptor this process owns.
        assert_eq!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        let open = |name: &str| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join(name))
                .expect("a file of the run")
        };
        let mut command = Command::new(runner);
        command
            .args(["-workspace"])
            .arg(project)
            .arg("-session-directory")
            .arg(&sessions)
            .args([
                "-tool-heartbeat-interval",
                "10m0s",
                "-steer",
                "-stream",
                "-detached",
            ])
            .current_dir(project)
            .env("KOU_CONVEYOR_LLM_PROVIDER", "openai")
            .env(
                "KOU_CONVEYOR_CONFIG",
                setup.settings_file.as_ref().expect("a settings file"),
            )
            .envs(setup.env.iter().map(|(name, value)| (name, value)))
            .stdin(std::fs::File::open(dir.join("in.jsonl")).expect("the input"))
            .stdout(open("out.jsonl"))
            .stderr(open("err.log"));
        let lock_fd = lock.as_raw_fd();
        // SAFETY: only async-signal-safe calls between fork and exec: the
        // session's lock becomes descriptor 3, which the runner keeps, and
        // the runner leads a session of its own.
        unsafe {
            command.pre_exec(move || {
                if lock_fd == 3 {
                    let flags = libc::fcntl(3, libc::F_GETFD);
                    libc::fcntl(3, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
                } else if libc::dup2(lock_fd, 3) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                libc::setsid();
                Ok(())
            });
        }
        let child = command.spawn().expect("the runner");
        // The runner holds the lock now; closing this copy leaves it locked.
        drop(lock);
        let job = json!({
            "pid": child.id(), "session_id": session, "session_dir": sessions,
            "session_size": 0, "workspace": project, "runner": runner,
            "steers": true, "stops": true, "links": true, "started": "2026-01-01T00:00:00Z",
        });
        std::fs::write(dir.join("job.json"), format!("{job}\n")).expect("the run's record");
        Self { child, dir }
    }

    /// Waits until the runner's output holds `text`.
    pub fn wait_for_output(&self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let output = std::fs::read_to_string(self.dir.join("out.jsonl")).unwrap_or_default();
            if output.contains(text) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the run never wrote {text:?}: {output}\n{}",
                std::fs::read_to_string(self.dir.join("err.log")).unwrap_or_default()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Waits for the runner to exit and returns its status.
    pub fn wait(&mut self) -> Option<i32> {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if let Ok(Some(status)) = self.child.try_wait() {
                return status.code();
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the runner did not exit");
    }
}

#[cfg(unix)]
impl Drop for Detached {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            // SAFETY: the group is the runner's own; a test that failed must
            // not leave it running.
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
            let _ = self.child.wait();
        }
    }
}
