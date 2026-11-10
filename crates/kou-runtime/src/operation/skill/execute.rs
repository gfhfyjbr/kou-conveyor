use super::*;
use kou_tools::process::CancelToken;
use std::{
    fs::OpenOptions,
    io,
    os::unix::fs::{FileExt, OpenOptionsExt},
    path::Path,
};

const MAX_CHUNK: usize = 256 << 10;
const MAX_CHECKPOINTS: usize = 4096;

/// Read admission policy, not a global RSS bound. Go's primitive has no content cap.
#[derive(Clone, Copy, Debug)]
pub struct SkillReadLimits {
    pub content_bytes: usize,
    pub chunk_bytes: usize,
}
impl Default for SkillReadLimits {
    fn default() -> Self {
        Self {
            content_bytes: 1 << 20,
            chunk_bytes: MAX_CHUNK,
        }
    }
}
impl SkillReadLimits {
    pub fn max_checkpoints(self) -> io::Result<usize> {
        if self.content_bytes == 0
            || self.content_bytes > i64::MAX as usize
            || self.chunk_bytes == 0
            || self.chunk_bytes > MAX_CHUNK
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid skill read limits",
            ));
        }
        let checkpoints = self
            .content_bytes
            .div_ceil(self.chunk_bytes)
            .checked_add(2)
            .filter(|count| *count <= MAX_CHECKPOINTS)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "skill read checkpoint limit exceeds 4096",
                )
            })?;
        Ok(checkpoints)
    }
}

/// Runs on the caller's worker. Publication must finish durably before returning
/// Ok; callback refusal stops all later reads/publications. This function owns no threads.
pub fn run_skill(
    current: Operation,
    cancel: &CancelToken,
    limits: SkillReadLimits,
    mut publish: impl FnMut(Operation) -> io::Result<()>,
) -> io::Result<()> {
    let max_checkpoints = limits.max_checkpoints()?;
    let mut reader = SkillReader::new(current).map_err(io::Error::other)?;
    if !matches!(
        reader.current.status.as_str(),
        "ready" | "awaiting" | "canceling"
    ) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot run terminal or unsupported skill operation",
        ));
    }
    if reader.state.content.len() > limits.content_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "recovered skill content exceeds runtime byte limit",
        ));
    }
    let mut checkpoints = 0;
    let mut publish = |step: SkillStep| -> io::Result<()> {
        if checkpoints >= max_checkpoints {
            return Err(io::Error::other(
                "skill executor exceeded its checkpoint limit",
            ));
        }
        checkpoints += 1;
        let current = step
            .operation
            .ok_or_else(|| io::Error::other("skill transition has no checkpoint"))?;
        publish(current)
    };
    if cancel.is_canceled() {
        return canceled(&mut reader, &mut publish);
    }
    let initial = reader.handle(None).map_err(io::Error::other)?;
    let terminal = initial.operation.as_ref().is_some_and(Operation::terminal);
    publish(initial)?;
    if terminal {
        return Ok(());
    }
    if cancel.is_canceled() {
        return canceled(&mut reader, &mut publish);
    }
    let path = std::path::PathBuf::from(&reader.state.path);
    // NONBLOCK prevents an unexpected FIFO from stalling a worker before fstat.
    // Follow symlinks, as the Go skill reader does, but accept regular files only.
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC).bits() as _)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) => {
            return failed(
                &mut reader,
                native_error("open", &path, error),
                &mut publish,
            );
        }
    };
    let info = match file.metadata() {
        Ok(info) => info,
        Err(error) => {
            return failed(
                &mut reader,
                native_error("stat", &path, error),
                &mut publish,
            );
        }
    };
    if !info.is_file() {
        return failed(
            &mut reader,
            "skill file is not a regular file".into(),
            &mut publish,
        );
    }
    if info.len() > limits.content_bytes as u64 {
        return failed(
            &mut reader,
            "skill file exceeds runtime content byte limit".into(),
            &mut publish,
        );
    }
    let size = info.len() as i64;
    loop {
        if cancel.is_canceled() {
            return canceled(&mut reader, &mut publish);
        }
        let offset = reader.state.content.len();
        let window = limits.chunk_bytes.min(limits.content_bytes - offset);
        let mut chunk = Vec::with_capacity(window);
        let mut scratch = [0u8; 8192];
        let mut eof = false;
        while chunk.len() < window {
            if cancel.is_canceled() {
                output(&mut reader, chunk, &mut publish)?;
                return canceled(&mut reader, &mut publish);
            }
            let count = scratch.len().min(window - chunk.len());
            match file.read_at(&mut scratch[..count], (offset + chunk.len()) as u64) {
                Ok(0) => {
                    eof = true;
                    break;
                }
                Ok(n) => chunk.extend_from_slice(&scratch[..n]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    output(&mut reader, chunk, &mut publish)?;
                    return failed(
                        &mut reader,
                        native_error("read", &path, error),
                        &mut publish,
                    );
                }
            }
        }
        output(&mut reader, chunk, &mut publish)?;
        if cancel.is_canceled() {
            return canceled(&mut reader, &mut publish);
        }
        if window == 0 {
            let mut extra = [0u8; 1];
            match file.read_at(&mut extra, offset as u64) {
                Ok(0) => eof = true,
                Ok(_) => {
                    return failed(
                        &mut reader,
                        "skill file exceeds runtime content byte limit".into(),
                        &mut publish,
                    );
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    return failed(
                        &mut reader,
                        native_error("read", &path, error),
                        &mut publish,
                    );
                }
            }
        }
        if eof {
            let step = reader
                .handle(Some(event(
                    &reader,
                    "io.read_completed",
                    SkillEventResult::Completed { size },
                )))
                .map_err(io::Error::other)?;
            return publish(step);
        }
    }
}
fn output(
    reader: &mut SkillReader,
    data: Vec<u8>,
    publish: &mut impl FnMut(SkillStep) -> io::Result<()>,
) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    let offset = reader.state.content.len() as i64;
    if !reader.state.content.is_empty() {
        // Preserve a content-byte cap instead of geometric Vec growth beyond it.
        reader
            .state
            .content
            .try_reserve_exact(data.len())
            .map_err(io::Error::other)?;
    }
    let step = reader
        .handle(Some(event(
            reader,
            "io.read_output",
            SkillEventResult::Output { offset, data },
        )))
        .map_err(io::Error::other)?;
    publish(step)
}
fn event(reader: &SkillReader, kind: &str, result: SkillEventResult) -> SkillEvent {
    SkillEvent {
        kind: kind.into(),
        source: reader.current.id.clone(),
        correlation_id: CORRELATION.into(),
        result,
    }
}
fn canceled(
    reader: &mut SkillReader,
    publish: &mut impl FnMut(SkillStep) -> io::Result<()>,
) -> io::Result<()> {
    reader.current.status = "canceling".into();
    let step = reader.handle(None).map_err(io::Error::other)?;
    publish(step)
}
fn failed(
    reader: &mut SkillReader,
    error: String,
    publish: &mut impl FnMut(SkillStep) -> io::Result<()>,
) -> io::Result<()> {
    let step = reader
        .handle(Some(event(
            reader,
            "primitive.failed",
            SkillEventResult::Failure(error),
        )))
        .map_err(io::Error::other)?;
    publish(step)
}
fn native_error(action: &str, path: &Path, error: io::Error) -> String {
    let reason = if error.kind() == io::ErrorKind::InvalidInput
        && path.as_os_str().as_encoded_bytes().contains(&0)
    {
        "invalid argument".into()
    } else {
        let message = error.to_string();
        let reason = error
            .raw_os_error()
            .and_then(|code| message.strip_suffix(&format!(" (os error {code})")))
            .unwrap_or(&message);
        reason.to_ascii_lowercase()
    };
    format!("{action} {}: {reason}", path.display())
}
