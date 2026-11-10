mod checkpoint;
mod index;
mod peek;
mod recovery;
mod replay;
mod writer;

pub use index::{ItemPage, LogSummary, ResumeCheckpoint, ResumeState, SessionLog};
pub use recovery::SessionRecovery;
pub use writer::{SessionStore, SessionWriter};

use std::{io, path::Path};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("decode record {line}: {source}")]
    Decode {
        line: u64,
        source: serde_json::Error,
    },
    #[error("session changed while reading; reopen the view")]
    Changed,
    #[error("record exceeds the configured {0}-byte limit")]
    RecordLimit(usize),
    #[error("session is already locked by another writer")]
    Busy,
    #[error("writer had an I/O failure; reopen it before continuing")]
    Poisoned,
    #[error("{0}")]
    Io(#[from] io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn validate_session_id(id: &str) -> Result<()> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(Error::Invalid(
            "session ID must contain only ASCII letters, digits, and dashes".into(),
        ));
    }
    Ok(())
}

pub fn open_session(directory: &Path, id: &str) -> Result<SessionLog> {
    validate_session_id(id)?;
    let log = SessionLog::open(directory.join(format!("{id}.session.jsonl")))?;
    if log.session().id != id {
        return Err(Error::Invalid("file contains another session ID".into()));
    }
    Ok(log)
}
