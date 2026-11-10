use crate::{
    Error, Result, SessionLog, checkpoint,
    index::{
        DEFAULT_RECORD_LIMIT, Location, OperationRef, ResumeCheckpoint, ResumeState, Scanner,
        SourceVersion, WriteState,
    },
    replay::Head,
};
use kou_domain::{FORMAT_VERSION, ItemData, Record, Session, Snapshot};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
};

/// Restores a consumer checkpoint and its replay tail without indexing old bodies.
/// Archive reads use SessionLog instead; a short recovery tail is never an archive.
pub struct SessionRecovery {
    path: PathBuf,
    file: File,
    version: SourceVersion,
    session: Session,
    head: Head,
    sequence: u64,
    committed: u64,
    checkpoint: Option<ResumeCheckpoint>,
}

impl SessionRecovery {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_inner(path.as_ref(), true)
    }

    pub(crate) fn open_for_writer(path: &Path) -> Result<Self> {
        Self::open_inner(path, false)
    }

    fn open_inner(path: &Path, keep_items: bool) -> Result<Self> {
        let path = path.to_owned();
        let file = File::open(&path)?;
        let version = SourceVersion::of(&file.metadata()?);
        let size = file.metadata()?.len();
        let committed = find_previous(&file, size, b"\n")?
            .ok_or_else(|| Error::Invalid("session log has no committed records".into()))?
            + 1;
        let mut first = Scanner::at(&file, 0, committed, DEFAULT_RECORD_LIMIT);
        let line = first
            .next()?
            .ok_or_else(|| Error::Invalid("session header is missing".into()))?;
        let Record::Session(header) =
            Record::decode(line).map_err(|source| Error::Decode { line: 1, source })?
        else {
            return Err(Error::Invalid("session header must come first".into()));
        };
        crate::validate_session_id(&header.session.id)?;
        if header.version != FORMAT_VERSION || header.session.created_at.is_zero() {
            return Err(Error::Invalid(
                "unsupported session version or zero creation time".into(),
            ));
        }
        let session = header.session;
        let mut before = committed;
        while let Some(marker) =
            find_previous(&file, before, b"\n{\"type\":\"checkpoint\",\"data\":")?
        {
            let offset = marker + 1;
            before = marker;
            let mut candidate = Scanner::at(&file, offset, committed, DEFAULT_RECORD_LIMIT);
            let Ok(Some(line)) = candidate.next() else {
                continue;
            };
            let Ok(Record::Checkpoint(raw)) = Record::decode(line) else {
                continue;
            };
            let Some(data) = checkpoint::decode(raw, offset) else {
                continue;
            };
            let after = data.head.sequence;
            let Ok(head) = Head::restore_checkpoint(data.head, &session, offset) else {
                continue;
            };
            let tail = candidate.offset;
            let mut recovery = Self {
                path: path.clone(),
                file,
                version,
                session,
                head,
                sequence: after,
                committed,
                checkpoint: Some(ResumeCheckpoint {
                    state: data.state,
                    after,
                    items: Vec::new(),
                }),
            };
            if recovery.replay_tail(tail, keep_items)? {
                recovery.ensure_current()?;
                return Ok(recovery);
            }
            // A fork makes the entire earlier prefix inherited, as full replay
            // determines from the last fork. Never resume it as owned history.
            return Self::full(path);
        }
        Self::full(path)
    }

    fn full(path: PathBuf) -> Result<Self> {
        let state = SessionLog::open(&path)?.into_write_state()?;
        let file = File::open(&path)?;
        let version = SourceVersion::of(&state.metadata);
        let value = Self {
            path,
            file,
            version,
            session: state.session,
            head: state.head,
            sequence: state.sequence,
            committed: state.committed,
            checkpoint: None,
        };
        value.ensure_current()?;
        Ok(value)
    }

    fn replay_tail(&mut self, offset: u64, keep_items: bool) -> Result<bool> {
        let mut scanner = Scanner::at(&self.file, offset, self.committed, DEFAULT_RECORD_LIMIT);
        loop {
            let offset = scanner.offset;
            let Some(line) = scanner.next()? else { break };
            let location = Location::new(offset, line.len(), Sha256::digest(line).into());
            match Record::decode(line).map_err(|source| Error::Decode {
                line: offset,
                source,
            })? {
                Record::Item(mut record) => {
                    if matches!(record.item.data, ItemData::Fork(_)) {
                        return Ok(false);
                    }
                    if self.sequence.checked_add(1) != Some(record.item.sequence)
                        || record.item.recorded_at.is_zero()
                    {
                        return Err(Error::Invalid(
                            "non-contiguous suffix sequence or zero time".into(),
                        ));
                    }
                    let sources = self.head.apply(&record, location)?;
                    self.sequence = record.item.sequence;
                    if keep_items {
                        if let ItemData::ToolCallStatus(status) = &mut record.item.data {
                            status.operations.clear();
                            for source in sources {
                                status
                                    .operations
                                    .push(source.load(&self.file, self.committed)?);
                            }
                        }
                        self.checkpoint
                            .as_mut()
                            .expect("checkpoint replay")
                            .items
                            .push(record.item);
                    }
                }
                Record::Operation(value) => self.head.save(
                    &value.operation,
                    OperationRef::Indexed {
                        record: location,
                        in_item: None,
                    },
                )?,
                Record::Checkpoint(_) => {}
                Record::Session(_) => {
                    return Err(Error::Invalid("session header repeated in suffix".into()));
                }
            }
        }
        Ok(true)
    }

    pub fn session(&self) -> &Session {
        &self.session
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn checkpoint_after(&self) -> Option<u64> {
        self.checkpoint.as_ref().map(|c| c.after)
    }

    fn ensure_current(&self) -> Result<()> {
        if self.version != SourceVersion::of(&self.file.metadata()?)
            || self.version != SourceVersion::of(&fs::metadata(&self.path)?)
        {
            return Err(Error::Changed);
        }
        Ok(())
    }

    pub fn into_resume(self) -> Result<ResumeState> {
        self.ensure_current()?;
        let operations = self.head.resume_operations(&self.file, self.committed)?;
        self.ensure_current()?;
        Ok(ResumeState {
            snapshot: Snapshot {
                session: self.session,
            },
            operations,
            external_input_ids: self.head.external_input_ids,
            checkpoint: self.checkpoint,
        })
    }

    pub(crate) fn into_write_state(self) -> Result<WriteState> {
        self.ensure_current()?;
        let metadata = self.file.metadata()?;
        if SourceVersion::of(&metadata) != self.version {
            return Err(Error::Changed);
        }
        Ok(WriteState {
            session: self.session,
            head: self.head,
            sequence: self.sequence,
            committed: self.committed,
            metadata,
        })
    }
}

fn find_previous(file: &File, before: u64, needle: &[u8]) -> Result<Option<u64>> {
    let mut end = before;
    let mut bytes = vec![0; 64 * 1024 + needle.len() - 1];
    while end != 0 {
        let start = end.saturating_sub(64 * 1024);
        let read_end = (end + needle.len() as u64 - 1).min(before);
        let count = usize::try_from(read_end - start).expect("bounded block");
        file.read_exact_at(&mut bytes[..count], start)?;
        if let Some(index) = memchr::memmem::rfind(&bytes[..count], needle) {
            return Ok(Some(start + index as u64));
        }
        end = start;
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn reverse_marker_search_handles_every_block_edge() {
        let marker = b"\n{\"type\":\"checkpoint\",\"data\":";
        for shift in 0..marker.len() + 2 {
            let mut file = tempfile::tempfile().unwrap();
            let prefix = 64 * 1024 - shift;
            let mut bytes = vec![b'x'; prefix];
            bytes.extend_from_slice(marker);
            bytes.extend_from_slice(&vec![b'y'; 64 * 1024]);
            file.write_all(&bytes).unwrap();
            assert_eq!(
                find_previous(&file, bytes.len() as u64, marker).unwrap(),
                Some(prefix as u64)
            );
            assert_eq!(find_previous(&file, prefix as u64, marker).unwrap(), None);
        }
    }
}
