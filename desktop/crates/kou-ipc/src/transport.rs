//! The socket itself: a listener on std threads for the app, and a blocking
//! one-shot client for the command line.
//!
//! A connection carries one request line and one response line. The thread
//! that accepted a connection only reads the request; the stream then travels
//! inside a [`Reply`], so a long [`Request::PaneWait`](crate::Request::PaneWait)
//! holds no thread while it waits.

use crate::protocol::{Request, Response, env};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A request line longer than this is refused. Hook payloads carry whole tool
/// results, so the limit is generous.
pub const MAX_REQUEST_BYTES: u64 = 32 * 1024 * 1024;

/// The app is not running: nothing listens on the socket. Found in the error
/// chain of [`call`], so the command line can tell this from other failures.
#[derive(Debug)]
pub struct NotRunning {
    /// The socket that was tried.
    pub socket: PathBuf,
}

impl fmt::Display for NotRunning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "kou-desktop is not running (no one listens on {})",
            self.socket.display()
        )
    }
}

impl std::error::Error for NotRunning {}

/// Where the app listens: `$KOU_SOCKET` when set, otherwise a per-user path
/// (the runtime directory on Linux, Application Support on macOS).
pub fn socket_path() -> PathBuf {
    match std::env::var_os(env::SOCKET) {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => default_socket_path(),
    }
}

fn default_socket_path() -> PathBuf {
    const DIR: &str = "kou-desktop";
    const FILE: &str = "kou.sock";
    // The runtime directory is per-user, cleared on logout and short enough
    // for a socket address; macOS and Windows have none.
    if let Some(runtime) = dirs::runtime_dir() {
        return runtime.join(DIR).join(FILE);
    }
    if cfg!(any(target_os = "macos", windows))
        && let Some(data) = dirs::data_local_dir()
    {
        return data.join(DIR).join(FILE);
    }
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".into());
    std::env::temp_dir()
        .join(format!("{DIR}-{user}"))
        .join(FILE)
}

/// A request the app has to answer.
pub struct Incoming {
    /// What was asked.
    pub request: Request,
    /// Where the answer goes.
    pub reply: Reply,
}

impl fmt::Debug for Incoming {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Incoming")
            .field("request", &self.request)
            .finish_non_exhaustive()
    }
}

/// The open connection of one request. It may be kept for as long as the
/// answer takes (a wait); dropping it unanswered tells the client so.
pub struct Reply {
    stream: Option<imp::Stream>,
}

impl Reply {
    /// Answer the request and close the connection. A client that has gone
    /// away in the meantime is not an error worth reporting.
    pub fn send(mut self, response: Response) {
        if let Some(stream) = self.stream.take() {
            imp::write_response(stream, &response);
        }
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take() {
            imp::write_response(
                stream,
                &Response::Error {
                    message: "the app dropped the request without answering".into(),
                },
            );
        }
    }
}

/// The listening socket. Dropping it stops the accept loop and removes the file.
pub struct Server {
    inner: imp::Server,
}

impl Server {
    /// Listen on `path`, replacing a socket file no one answers on. Requests
    /// arrive on the returned channel in the order they were read.
    ///
    /// Fails when another instance answers on `path`, or when `path` is
    /// something other than a socket.
    pub fn bind(path: &Path) -> anyhow::Result<(Server, async_channel::Receiver<Incoming>)> {
        let (inner, incoming) = imp::Server::bind(path)?;
        Ok((Server { inner }, incoming))
    }

    /// The path the server listens on.
    pub fn path(&self) -> &Path {
        self.inner.path()
    }
}

/// Send one request and wait for its answer. `timeout` bounds the wait for the
/// answer; `None` waits for as long as the app holds the request.
///
/// When nothing listens on `path`, the error chain contains [`NotRunning`].
pub fn call(path: &Path, request: &Request, timeout: Option<Duration>) -> anyhow::Result<Response> {
    imp::call(path, request, timeout)
}

/// Send one request and do not wait for its answer: for a caller that only
/// tells the app something, and must not be held up by an app that is busy.
/// Returns once the request is the app's to read, which it does whether or
/// not the caller is still there.
pub fn tell(path: &Path, request: &Request) -> anyhow::Result<()> {
    imp::tell(path, request)
}

#[cfg(unix)]
mod imp {
    use super::{Incoming, MAX_REQUEST_BYTES, NotRunning, Reply};
    use crate::protocol::{Request, Response};
    use anyhow::{Context as _, bail};
    use std::fs;
    use std::io::{self, BufRead as _, BufReader, Read as _, Write as _};
    use std::net::Shutdown;
    use std::os::unix::fs::{DirBuilderExt as _, FileTypeExt as _, PermissionsExt as _};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    pub(super) type Stream = UnixStream;

    /// How long the server waits for a connected client to send its request line.
    const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(10);

    pub(super) struct Server {
        path: PathBuf,
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl Server {
        pub(super) fn bind(
            path: &Path,
        ) -> anyhow::Result<(Server, async_channel::Receiver<Incoming>)> {
            clear_stale(path)?;
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
                && !parent.exists()
            {
                // Only this user may reach the socket: it starts processes.
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            let listener = UnixListener::bind(path)
                .with_context(|| format!("listening on {}", path.display()))?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                .with_context(|| format!("restricting {}", path.display()))?;

            let (sender, receiver) = async_channel::unbounded();
            let stop = Arc::new(AtomicBool::new(false));
            let thread = thread::Builder::new()
                .name("kou-ipc-accept".into())
                .spawn({
                    let stop = stop.clone();
                    move || accept_loop(listener, sender, stop)
                })
                .context("starting the accept thread")?;
            Ok((
                Server {
                    path: path.to_path_buf(),
                    stop,
                    thread: Some(thread),
                },
                receiver,
            ))
        }

        pub(super) fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            // `accept` has no timeout; a connection of our own wakes it.
            drop(UnixStream::connect(&self.path));
            if let Some(thread) = self.thread.take() {
                drop(thread.join());
            }
            drop(fs::remove_file(&self.path));
        }
    }

    /// Remove a socket file left by an instance that is gone. A file that
    /// answers belongs to a live instance, and a non-socket is not ours to delete.
    fn clear_stale(path: &Path) -> anyhow::Result<()> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", path.display()));
            }
        };
        if !metadata.file_type().is_socket() {
            bail!(
                "{} exists and is not a socket; refusing to replace it",
                path.display()
            );
        }
        if UnixStream::connect(path).is_ok() {
            bail!("another kou-desktop already listens on {}", path.display());
        }
        fs::remove_file(path).with_context(|| format!("removing the stale {}", path.display()))
    }

    fn accept_loop(
        listener: UnixListener,
        sender: async_channel::Sender<Incoming>,
        stop: Arc<AtomicBool>,
    ) {
        for connection in listener.incoming() {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let stream = match connection {
                Ok(stream) => stream,
                Err(error) => {
                    log::warn!("kou-ipc: accept failed: {error}");
                    // A failing listener (out of descriptors) must not spin.
                    thread::sleep(Duration::from_millis(50));
                    continue;
                }
            };
            let sender = sender.clone();
            // Reading waits on the client, so it never happens on the accept
            // thread: one slow client must not delay a hook from another.
            let spawned = thread::Builder::new()
                .name("kou-ipc-request".into())
                .spawn(move || read_request(stream, sender));
            if let Err(error) = spawned {
                log::warn!("kou-ipc: could not start a request thread: {error}");
            }
        }
    }

    fn read_request(stream: UnixStream, sender: async_channel::Sender<Incoming>) {
        drop(stream.set_read_timeout(Some(REQUEST_READ_TIMEOUT)));
        let mut line = Vec::new();
        let read = BufReader::new(&stream)
            .take(MAX_REQUEST_BYTES + 1)
            .read_until(b'\n', &mut line);
        let reply = Reply {
            stream: Some(stream),
        };
        let refuse = |reply: Reply, message: String| reply.send(Response::Error { message });
        match read {
            Err(error) => return refuse(reply, format!("could not read the request: {error}")),
            // A connection that sends nothing is the server waking itself, or a
            // probe for a live instance; there is no one to answer.
            Ok(0) => {
                let mut reply = reply;
                reply.stream = None;
                return;
            }
            Ok(_) => {}
        }
        if line.len() as u64 > MAX_REQUEST_BYTES {
            return refuse(
                reply,
                format!("the request is longer than {MAX_REQUEST_BYTES} bytes"),
            );
        }
        match serde_json::from_slice::<Request>(&line) {
            Ok(request) => {
                // A closed channel drops the reply, which answers with an error.
                drop(sender.send_blocking(Incoming { request, reply }));
            }
            Err(error) => refuse(reply, format!("malformed request: {error}")),
        }
    }

    pub(super) fn write_response(mut stream: UnixStream, response: &Response) {
        drop(stream.set_write_timeout(Some(Duration::from_secs(5))));
        let mut line = match serde_json::to_vec(response) {
            Ok(line) => line,
            Err(error) => {
                log::warn!("kou-ipc: could not encode a response: {error}");
                return;
            }
        };
        line.push(b'\n');
        // The client may have given up already; nothing to do about it.
        if stream.write_all(&line).is_ok() {
            drop(stream.flush());
        }
        drop(stream.shutdown(Shutdown::Both));
    }

    // Connect and hand the request over.
    fn send(path: &Path, request: &Request) -> anyhow::Result<UnixStream> {
        let mut stream = UnixStream::connect(path).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
                anyhow::Error::new(NotRunning {
                    socket: path.to_path_buf(),
                })
            }
            _ => anyhow::Error::new(error).context(format!("connecting to {}", path.display())),
        })?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut line = serde_json::to_vec(request).context("encoding the request")?;
        line.push(b'\n');
        stream.write_all(&line).context("sending the request")?;
        stream.flush().context("sending the request")?;
        Ok(stream)
    }

    pub(super) fn tell(path: &Path, request: &Request) -> anyhow::Result<()> {
        // Closed for writing only: the app still reads what was sent, and
        // its answer goes nowhere.
        let stream = send(path, request)?;
        drop(stream.shutdown(Shutdown::Write));
        Ok(())
    }

    pub(super) fn call(
        path: &Path,
        request: &Request,
        timeout: Option<Duration>,
    ) -> anyhow::Result<Response> {
        let stream = send(path, request)?;
        // A zero timeout is an error in std; treat it as the shortest wait.
        stream.set_read_timeout(timeout.map(|timeout| timeout.max(Duration::from_millis(1))))?;

        let mut answer = String::new();
        BufReader::new(&stream)
            .read_line(&mut answer)
            .map_err(|error| match error.kind() {
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
                    anyhow::anyhow!("the app did not answer in time")
                }
                _ => anyhow::Error::new(error).context("reading the answer"),
            })?;
        if answer.trim().is_empty() {
            bail!("the app closed the connection without answering");
        }
        serde_json::from_str(&answer).context("malformed answer from the app")
    }
}

#[cfg(not(unix))]
mod imp {
    //! Windows has no transport yet: named pipes are the plan. Everything
    //! compiles, and the two entry points say what is missing.

    use super::Incoming;
    use crate::protocol::{Request, Response};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    const UNSUPPORTED: &str =
        "the kou-desktop socket is not supported on this platform yet (Unix sockets only)";

    pub(super) struct Stream;

    pub(super) struct Server {
        path: PathBuf,
    }

    impl Server {
        pub(super) fn bind(
            _path: &Path,
        ) -> anyhow::Result<(Server, async_channel::Receiver<Incoming>)> {
            anyhow::bail!(UNSUPPORTED)
        }

        pub(super) fn path(&self) -> &Path {
            &self.path
        }
    }

    pub(super) fn write_response(_stream: Stream, _response: &Response) {}

    pub(super) fn call(
        _path: &Path,
        _request: &Request,
        _timeout: Option<Duration>,
    ) -> anyhow::Result<Response> {
        anyhow::bail!(UNSUPPORTED)
    }

    pub(super) fn tell(_path: &Path, _request: &Request) -> anyhow::Result<()> {
        anyhow::bail!(UNSUPPORTED)
    }
}
