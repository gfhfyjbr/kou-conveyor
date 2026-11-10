use crate::{
    Error, Result, SessionLog,
    index::{Location, OperationRef},
    replay::Head,
    validate_session_id,
};
use kou_domain::{
    FORMAT_VERSION, Item, ItemData, ItemOperation, ItemRecord, Operation, OperationRecord, Record,
    Session, SessionRecord, Timestamp,
};
use rustix::fs::{FlockOperation, flock};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct SessionStore {
    directory: PathBuf,
}

impl SessionStore {
    pub fn new(directory: impl AsRef<Path>) -> Result<Self> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory.as_ref())?;
        Ok(Self {
            directory: directory.as_ref().canonicalize()?,
        })
    }

    fn lock(&self, id: &str) -> Result<File> {
        validate_session_id(id)?;
        let locks = self.directory.join(".locks");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&locks)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(locks.join(format!("{id}.lock")))?;
        flock(&lock, FlockOperation::NonBlockingLockExclusive).map_err(|error| {
            let error = io::Error::from(error);
            if error.kind() == io::ErrorKind::WouldBlock {
                Error::Busy
            } else {
                Error::Io(error)
            }
        })?;
        Ok(lock)
    }

    pub fn create_new(&self, session: Session) -> Result<SessionWriter> {
        self.create(session, false)
    }

    /// Explicitly replaces an existing log under its writer lock, like Go Create.
    pub fn create_or_replace(&self, session: Session) -> Result<SessionWriter> {
        self.create(session, true)
    }

    fn create(&self, session: Session, replace: bool) -> Result<SessionWriter> {
        let lock = self.lock(&session.id)?;
        let temporary = self.initial_file(&session)?;
        self.publish(&session.id, temporary, lock, replace)
    }

    pub fn fork_new(
        &self,
        session: Session,
        parent_id: &str,
        previous_turn_id: &str,
    ) -> Result<SessionWriter> {
        self.fork(session, parent_id, previous_turn_id, false)
    }

    /// Replaces only the destination, never the parent's authoritative history.
    pub fn fork_or_replace(
        &self,
        session: Session,
        parent_id: &str,
        previous_turn_id: &str,
    ) -> Result<SessionWriter> {
        self.fork(session, parent_id, previous_turn_id, true)
    }

    fn fork(
        &self,
        session: Session,
        parent_id: &str,
        previous_turn_id: &str,
        replace: bool,
    ) -> Result<SessionWriter> {
        validate_session_id(parent_id)?;
        if session.id == parent_id {
            return Err(Error::Invalid("cannot fork a session onto itself".into()));
        }
        let lock = self.lock(&session.id)?;
        let parent = SessionLog::open(self.path(parent_id))?;
        if parent.session().id != parent_id {
            return Err(Error::Invalid("parent session ID mismatch".into()));
        }
        let boundary = parent.fork_boundary(previous_turn_id)?;
        let mut temporary = self.initial_file(&session)?;
        {
            let mut out = BufWriter::with_capacity(64 * 1024, temporary.as_file_mut());
            for sequence in 1..=boundary {
                write_line(&mut out, &Record::Item(parent.inherited_item(sequence)?))?;
            }
            write_line(
                &mut out,
                &Record::Item(ItemRecord {
                    item: Item {
                        sequence: boundary + 1,
                        recorded_at: session.created_at,
                        data: ItemData::Fork(kou_domain::Fork {
                            parent_id: parent_id.into(),
                            previous_turn_id: previous_turn_id.into(),
                        }),
                    },
                    operations: Vec::new(),
                }),
            )?;
            out.flush()?;
        }
        parent.ensure_current()?;
        self.publish(&session.id, temporary, lock, replace)
    }

    fn initial_file(&self, session: &Session) -> Result<tempfile::NamedTempFile> {
        if session.created_at.is_zero() {
            return Err(Error::Invalid("session creation time is zero".into()));
        }
        let mut temporary = tempfile::NamedTempFile::new_in(&self.directory)?;
        write_line(
            temporary.as_file_mut(),
            &Record::Session(SessionRecord {
                version: FORMAT_VERSION,
                session: session.clone(),
            }),
        )?;
        Ok(temporary)
    }

    fn publish(
        &self,
        id: &str,
        temporary: tempfile::NamedTempFile,
        lock: File,
        replace: bool,
    ) -> Result<SessionWriter> {
        temporary.as_file().sync_all()?;
        let path = self.path(id);
        if replace {
            temporary
                .persist(path)
                .map_err(|error| Error::Io(error.error))?;
        } else {
            temporary
                .persist_noclobber(path)
                .map_err(|error| Error::Io(error.error))?;
        }
        File::open(&self.directory)?.sync_all()?;
        self.open_locked(id, lock)
    }

    pub fn open_writer(&self, id: &str) -> Result<SessionWriter> {
        let lock = self.lock(id)?;
        self.open_locked(id, lock)
    }

    fn path(&self, id: &str) -> PathBuf {
        self.directory.join(format!("{id}.session.jsonl"))
    }

    fn open_locked(&self, id: &str, lock: File) -> Result<SessionWriter> {
        let path = self.path(id);
        if !fs::symlink_metadata(&path)?.file_type().is_file() {
            return Err(Error::Invalid("session is not a regular file".into()));
        }
        let log = crate::SessionRecovery::open_for_writer(&path)?;
        if log.session().id != id {
            return Err(Error::Invalid("session ID mismatch".into()));
        }
        let state = log.into_write_state()?;
        let file = OpenOptions::new().read(true).append(true).open(&path)?;
        let version = Stamp::of(&file.metadata()?);
        if version != Stamp::of(&state.metadata) {
            return Err(Error::Changed);
        }
        Ok(SessionWriter {
            file,
            path,
            _lock: lock,
            version,
            session: state.session,
            head: state.head,
            sequence: state.sequence,
            committed: state.committed,
            poisoned: false,
        })
    }
}

#[derive(PartialEq, Eq)]
struct Stamp {
    device: u64,
    inode: u64,
    bytes: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}
impl Stamp {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            bytes: metadata.len(),
            mtime: (metadata.mtime(), metadata.mtime_nsec()),
            ctime: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

pub struct SessionWriter {
    file: File,
    path: PathBuf,
    _lock: File,
    version: Stamp,
    session: Session,
    head: Head,
    sequence: u64,
    committed: u64,
    poisoned: bool,
}

impl SessionWriter {
    pub fn session(&self) -> &Session {
        &self.session
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    fn current(&self) -> Result<()> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        if Stamp::of(&self.file.metadata()?) != self.version
            || Stamp::of(&fs::metadata(&self.path)?) != self.version
        {
            return Err(Error::Changed);
        }
        Ok(())
    }

    pub fn append_item(&mut self, data: ItemData, recorded_at: Timestamp) -> Result<u64> {
        self.current()?;
        if recorded_at.is_zero() {
            return Err(Error::Invalid("item recorded time is zero".into()));
        }
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("sequence exhausted".into()))?;
        let mut item = Item {
            sequence,
            recorded_at,
            data,
        };
        let mut operations = Vec::new();
        if let ItemData::ToolCallStatus(status) = &mut item.data {
            for operation in std::mem::take(&mut status.operations) {
                operations.push(if self.head.same_operation(&operation)? {
                    ItemOperation::Saved(operation.id)
                } else {
                    ItemOperation::Full(operation)
                });
            }
        }
        let record = ItemRecord { item, operations };
        self.head.check_item(&record)?;
        let record = Record::Item(record);
        let location = self.commit(&record)?;
        let Record::Item(record) = record else {
            unreachable!()
        };
        if let Err(error) = self.head.apply(&record, location) {
            self.poisoned = true;
            return Err(error);
        }
        self.sequence = sequence;
        Ok(sequence)
    }

    /// State is the consumer's existing checkpoint JSON; the store never
    /// invents a new consumer schema under the legacy envelope version.
    pub fn save_checkpoint(&mut self, state: kou_domain::Json) -> Result<()> {
        self.current()?;
        if state.is_missing() {
            return Err(Error::Invalid("checkpoint state must be valid JSON".into()));
        }
        let head =
            self.head
                .make_checkpoint(&self.session, self.sequence, &self.file, self.committed)?;
        let data =
            crate::checkpoint::encode(crate::checkpoint::Data { head, state }, self.committed)?;
        self.current()?;
        self.commit(&Record::Checkpoint(data))?;
        Ok(())
    }

    pub fn save_operation(&mut self, operation: Operation) -> Result<()> {
        self.current()?;
        self.head.check_save(&operation)?;
        let record = Record::Operation(OperationRecord { operation });
        let location = self.commit(&record)?;
        let Record::Operation(value) = record else {
            unreachable!()
        };
        if let Err(error) = self.head.save(
            &value.operation,
            OperationRef::Indexed {
                record: location,
                in_item: None,
            },
        ) {
            self.poisoned = true;
            return Err(error);
        }
        Ok(())
    }

    fn commit(&mut self, record: &Record) -> Result<Location> {
        self.current()?;
        let result = self.write_record(record);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn write_record(&mut self, record: &Record) -> Result<Location> {
        // Only an uncommitted tail is removed. No mmap reader can observe a
        // shorter committed prefix, and every complete append is fsynced.
        self.file.set_len(self.committed)?;
        let mut buffer = BufWriter::with_capacity(64 * 1024, &self.file);
        let (length, digest) = {
            let mut output = HashWriter {
                writer: &mut buffer,
                digest: Sha256::new(),
                length: 0,
            };
            kou_domain::write_compact_json(&mut output, record).map_err(json_error)?;
            (output.length, <[u8; 32]>::from(output.digest.finalize()))
        };
        buffer.write_all(b"\n")?;
        buffer.flush()?;
        self.file.sync_all()?;
        let location = Location::new(self.committed, length, digest);
        self.committed = self
            .committed
            .checked_add(length as u64 + 1)
            .ok_or_else(|| Error::Invalid("session offset exhausted".into()))?;
        self.version = Stamp::of(&self.file.metadata()?);
        Ok(location)
    }
}

fn write_line(mut out: impl Write, record: &Record) -> Result<()> {
    kou_domain::write_compact_json(&mut out, record).map_err(json_error)?;
    out.write_all(b"\n")?;
    Ok(())
}

fn json_error(error: serde_json::Error) -> Error {
    if let Some(kind) = error.io_error_kind() {
        Error::Io(io::Error::new(kind, error))
    } else {
        Error::Invalid(error.to_string())
    }
}

struct HashWriter<'a, W> {
    writer: &'a mut W,
    digest: Sha256,
    length: usize,
}
impl<W: Write> Write for HashWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.writer.write_all(bytes)?;
        self.digest.update(bytes);
        self.length += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kou_domain::{Input, Json};

    #[test]
    fn rejected_initial_status_preserves_go_diagnostics_and_durable_head() {
        use kou_domain::{CallStatus, ToolCallStatus, Turn};
        let directory = tempfile::tempdir().unwrap();
        let store = SessionStore::new(directory.path()).unwrap();
        let at = Timestamp::parse("2026-11-10T02:00:00Z").unwrap();
        let mut writer = store
            .create_new(Session {
                id: "status-errors".into(),
                created_at: at,
            })
            .unwrap();
        writer
            .append_item(
                ItemData::Turn(Turn {
                    id: "turn".into(),
                    previous_turn_id: String::new(),
                    kind: "regular".into(),
                    model: String::new(),
                }),
                at,
            )
            .unwrap();
        let operation = |id: &str| Operation {
            id: id.into(),
            kind: "value".into(),
            version: 1,
            status: "ready".into(),
            ..Operation::default()
        };
        let status = |error: &str, waiting: &[&str]| CallStatus {
            error: error.into(),
            waiting_for: Some(waiting.iter().map(|id| (*id).into()).collect()),
            ..CallStatus::default()
        };
        let before = fs::read(&writer.path).unwrap();
        for (status, operations, diagnostic) in [
            (
                status("", &[]),
                vec![],
                "successful status must initialize at least one operation",
            ),
            (
                status("rejected", &["one"]),
                vec![],
                "error status cannot wait for operations",
            ),
            (
                status("rejected", &[]),
                vec![operation("one")],
                "error status cannot initialize operations",
            ),
            (
                status("", &[]),
                vec![operation("one")],
                "status waits for 0 operations, initialized 1",
            ),
            (
                status("", &["missing"]),
                vec![operation("one")],
                "status waits for operation \"missing\" that was not initialized",
            ),
            (
                status("", &["one", "one"]),
                vec![operation("one"), operation("two")],
                "status repeats operation \"one\"",
            ),
        ] {
            let error = writer
                .append_item(
                    ItemData::ToolCallStatus(ToolCallStatus {
                        turn_id: "turn".into(),
                        call_id: "call".into(),
                        status,
                        operations,
                    }),
                    at,
                )
                .unwrap_err();
            assert_eq!(
                error.to_string(),
                format!(
                    "append tool-call status for turn \"turn\" and call \"call\": {diagnostic}"
                )
            );
            assert_eq!(writer.sequence(), 1);
            assert_eq!(fs::read(&writer.path).unwrap(), before);
        }
        writer
            .append_item(
                ItemData::ToolCallStatus(ToolCallStatus {
                    turn_id: "turn".into(),
                    call_id: "call".into(),
                    status: status("", &["one"]),
                    operations: vec![operation("one")],
                }),
                at,
            )
            .unwrap();
        assert_eq!(writer.sequence(), 2);
        assert_eq!(SessionLog::open(&writer.path).unwrap().item_count(), 2);
    }

    #[test]
    fn an_io_failure_poisons_the_writer_instead_of_silently_retrying() {
        let directory = tempfile::tempdir().unwrap();
        let store = SessionStore::new(directory.path()).unwrap();
        let at = Timestamp::parse("2026-09-01T12:00:00Z").unwrap();
        let mut writer = store
            .create_new(Session {
                id: "failure".into(),
                created_at: at,
            })
            .unwrap();
        let before = fs::read(&writer.path).unwrap();
        // A read-only descriptor has identical metadata but cannot commit.
        // Inject it privately instead of modifying permissions on a user file.
        writer.file = File::open(&writer.path).unwrap();
        let input = || {
            ItemData::Input(Input {
                id: "input".into(),
                kind: "external".into(),
                payload: Json::parse(r#"{"message":"test"}"#).unwrap(),
                delivery: String::new(),
            })
        };
        assert!(matches!(writer.append_item(input(), at), Err(Error::Io(_))));
        assert!(matches!(
            writer.append_item(input(), at),
            Err(Error::Poisoned)
        ));
        assert_eq!(fs::read(&writer.path).unwrap(), before);
        assert_eq!(writer.sequence(), 0);
    }
}
