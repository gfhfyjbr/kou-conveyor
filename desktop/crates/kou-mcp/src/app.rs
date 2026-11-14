//! Requests to the app, one connection each, as `kou_ipc::call` makes them,
//! except that another thread can close the connection of a call in flight:
//! that is how a cancelled `wait_for_panes` lets go of the app.

use kou_ipc::{Request, Response};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Why a call brought no answer.
#[derive(Debug)]
pub(crate) enum CallError {
    /// Nothing listens on the socket.
    NotRunning(PathBuf),
    /// The call was cancelled while it waited.
    Cancelled,
    /// Anything else, said for the model.
    Failed(String),
}

/// The cancellation of one tool call, shared between the thread that runs it
/// and the reader that hears `notifications/cancelled`.
#[derive(Debug, Default)]
pub(crate) struct Cancel {
    cancelled: AtomicBool,
    /// The connection of the request in flight, to shut down on cancel.
    #[cfg(unix)]
    open: Mutex<Option<std::os::unix::net::UnixStream>>,
}

impl Cancel {
    /// Stop the call: the flag is seen between requests, and the connection
    /// in flight is shut down, which ends a blocked read at once and tells the
    /// app the caller has gone.
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        #[cfg(unix)]
        if let Some(stream) = lock(&self.open).take() {
            drop(stream.shutdown(std::net::Shutdown::Both));
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Keep a handle on `stream` for [`Cancel::cancel`]. False when the call
    /// was cancelled before it got this far.
    #[cfg(unix)]
    fn hold(&self, stream: &std::os::unix::net::UnixStream) -> bool {
        let mut open = lock(&self.open);
        // Checked under the lock, so a cancel either sees the stream or is seen here.
        if self.is_cancelled() {
            return false;
        }
        *open = stream.try_clone().ok();
        true
    }

    #[cfg(unix)]
    fn release(&self) {
        lock(&self.open).take();
    }
}

#[cfg(unix)]
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Send one request and wait for its answer, for at most `timeout`.
pub(crate) fn call(
    socket: &Path,
    request: &Request,
    timeout: Duration,
    cancel: &Cancel,
) -> Result<Response, CallError> {
    if cancel.is_cancelled() {
        return Err(CallError::Cancelled);
    }
    imp::call(socket, request, timeout, cancel)
}

#[cfg(unix)]
mod imp {
    use super::{CallError, Cancel};
    use kou_ipc::{Request, Response};
    use std::io::{self, BufRead as _, BufReader, Write as _};
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::time::Duration;

    pub(super) fn call(
        socket: &Path,
        request: &Request,
        timeout: Duration,
        cancel: &Cancel,
    ) -> Result<Response, CallError> {
        let stream = UnixStream::connect(socket).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
                CallError::NotRunning(socket.to_path_buf())
            }
            _ => CallError::Failed(format!(
                "could not reach kou desktop at {}: {error}",
                socket.display()
            )),
        })?;
        if !cancel.hold(&stream) {
            return Err(CallError::Cancelled);
        }
        let answer = exchange(&stream, request, timeout);
        cancel.release();
        // A shut-down connection reads as an early end or an error: either way
        // it was the cancel, not the app.
        if cancel.is_cancelled() {
            return Err(CallError::Cancelled);
        }
        answer
    }

    fn exchange(
        stream: &UnixStream,
        request: &Request,
        timeout: Duration,
    ) -> Result<Response, CallError> {
        let failed = |what: &str, error: io::Error| match error.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
                CallError::Failed("kou desktop did not answer in time".into())
            }
            _ => CallError::Failed(format!("{what} kou desktop failed: {error}")),
        };
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .and_then(|()| stream.set_read_timeout(Some(timeout.max(Duration::from_millis(1)))))
            .map_err(|error| failed("talking to", error))?;
        let mut line = serde_json::to_vec(request)
            .map_err(|error| CallError::Failed(format!("could not encode the request: {error}")))?;
        line.push(b'\n');
        let mut writer = stream;
        writer
            .write_all(&line)
            .and_then(|()| writer.flush())
            .map_err(|error| failed("writing to", error))?;
        let mut answer = Vec::new();
        BufReader::new(stream)
            .read_until(b'\n', &mut answer)
            .map_err(|error| failed("reading from", error))?;
        if answer.iter().all(u8::is_ascii_whitespace) {
            return Err(CallError::Failed(
                "kou desktop closed the connection without answering".into(),
            ));
        }
        serde_json::from_slice(&answer).map_err(|error| {
            CallError::Failed(format!(
                "kou desktop answered with something unreadable: {error}"
            ))
        })
    }
}

#[cfg(not(unix))]
mod imp {
    //! No transport of our own off Unix: the app's socket is Unix-only so far,
    //! and `kou_ipc::call` says so. A call cannot be interrupted here.

    use super::{CallError, Cancel};
    use kou_ipc::{NotRunning, Request, Response};
    use std::path::Path;
    use std::time::Duration;

    pub(super) fn call(
        socket: &Path,
        request: &Request,
        timeout: Duration,
        _cancel: &Cancel,
    ) -> Result<Response, CallError> {
        kou_ipc::call(socket, request, Some(timeout)).map_err(|error| {
            match error.downcast_ref::<NotRunning>() {
                Some(not_running) => CallError::NotRunning(not_running.socket.clone()),
                None => CallError::Failed(format!("{error:#}")),
            }
        })
    }
}
