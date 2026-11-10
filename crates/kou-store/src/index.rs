use crate::{Error, Result, replay::Head, validate_session_id};
use kou_domain::{
    FORMAT_VERSION, Item, ItemData, ItemOperation, Operation, Record, Session, Snapshot,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, Metadata},
    io::{BufRead, BufReader, Read},
    os::unix::fs::{FileExt, MetadataExt},
    path::{Path, PathBuf},
};

pub(crate) const DEFAULT_RECORD_LIMIT: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Location {
    offset: u64,
    length: usize,
    digest: [u8; 32],
}

impl Location {
    pub(crate) fn offset(self) -> u64 {
        self.offset
    }

    pub(crate) fn new(offset: u64, length: usize, digest: [u8; 32]) -> Self {
        Self {
            offset,
            length,
            digest,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum OperationRef {
    Indexed {
        record: Location,
        in_item: Option<usize>,
    },
    Checkpoint {
        offset: u64,
        operation: std::sync::Arc<Operation>,
        archived: bool,
    },
}

impl OperationRef {
    pub(crate) fn offset(&self) -> u64 {
        match self {
            Self::Indexed { record, .. } => record.offset(),
            Self::Checkpoint { offset, .. } => *offset,
        }
    }

    pub(crate) fn source_bytes(&self) -> usize {
        match self {
            Self::Indexed { record, .. } => record.length,
            Self::Checkpoint { .. } => DEFAULT_RECORD_LIMIT,
        }
    }

    pub(crate) fn load(&self, file: &File, committed: u64) -> Result<Operation> {
        match self {
            Self::Indexed { record, in_item } => {
                let mut bytes = vec![0; record.length];
                file.read_exact_at(&mut bytes, record.offset)?;
                if <[u8; 32]>::from(Sha256::digest(&bytes)) != record.digest {
                    return Err(Error::Changed);
                }
                match (
                    Record::decode(&bytes).map_err(|source| Error::Decode {
                        line: record.offset,
                        source,
                    })?,
                    in_item,
                ) {
                    (Record::Operation(value), None) => Ok(value.operation),
                    (Record::Item(value), Some(index)) => {
                        match value.operations.into_iter().nth(*index) {
                            Some(ItemOperation::Full(operation)) => Ok(operation),
                            _ => Err(Error::Changed),
                        }
                    }
                    _ => Err(Error::Changed),
                }
            }
            Self::Checkpoint {
                offset,
                operation,
                archived: false,
            } => {
                let _ = offset;
                Ok((**operation).clone())
            }
            Self::Checkpoint {
                offset,
                operation,
                archived: true,
            } => {
                if *offset == 0 || *offset >= committed {
                    return Err(Error::Invalid("invalid archived operation offset".into()));
                }
                let mut before = [0];
                file.read_exact_at(&mut before, offset - 1)?;
                if before != *b"\n" {
                    return Err(Error::Invalid(
                        "archived operation is not at a record boundary".into(),
                    ));
                }
                let mut scanner = Scanner::at(file, *offset, committed, DEFAULT_RECORD_LIMIT);
                let line = scanner
                    .next()?
                    .ok_or_else(|| Error::Invalid("archived operation is uncommitted".into()))?;
                let value = match Record::decode(line).map_err(|source| Error::Decode {
                    line: *offset,
                    source,
                })? {
                    Record::Operation(value) => Some(value.operation),
                    Record::Item(value) => {
                        value.operations.into_iter().find_map(|value| match value {
                            ItemOperation::Full(value) if value.id == operation.id => Some(value),
                            _ => None,
                        })
                    }
                    _ => None,
                }
                .ok_or_else(|| Error::Invalid("archived operation record is missing".into()))?;
                if value.id != operation.id
                    || value.kind != operation.kind
                    || value.version != operation.version
                    || value.status != operation.status
                {
                    return Err(Error::Invalid(
                        "archived operation differs from checkpoint".into(),
                    ));
                }
                Ok(value)
            }
        }
    }
}

struct ItemLocation {
    record: Location,
    operations: Vec<OperationRef>,
    inherited: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceVersion {
    device: u64,
    inode: u64,
    size: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl SourceVersion {
    pub(crate) fn of(m: &Metadata) -> Self {
        Self {
            device: m.dev(),
            inode: m.ino(),
            size: m.len(),
            mtime: (m.mtime(), m.mtime_nsec()),
            ctime: (m.ctime(), m.ctime_nsec()),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ItemPage {
    pub items: Vec<Item>,
    pub next_after: u64,
    pub more: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ResumeState {
    pub snapshot: Snapshot,
    pub operations: Vec<Operation>,
    #[serde(rename = "ExternalInputIDs")]
    pub external_input_ids: Vec<String>,
    pub checkpoint: Option<ResumeCheckpoint>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ResumeCheckpoint {
    pub state: kou_domain::Json,
    pub after: u64,
    pub items: Vec<Item>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub struct LogSummary<'a> {
    pub session: &'a Session,
    pub generation: &'a str,
    pub committed_bytes: u64,
    pub ignored_tail_bytes: u64,
    pub records: usize,
    pub items: usize,
    pub operations: usize,
    pub retained_index_bytes: usize,
}

pub struct SessionLog {
    file: File,
    path: PathBuf,
    version: SourceVersion,
    session: Session,
    generation: String,
    committed_bytes: u64,
    records: Vec<Location>,
    items: Vec<ItemLocation>,
    head: Head,
}

pub(crate) struct WriteState {
    pub(crate) head: Head,
    pub(crate) session: Session,
    pub(crate) sequence: u64,
    pub(crate) committed: u64,
    pub(crate) metadata: Metadata,
}

impl SessionLog {
    pub(crate) fn into_write_state(self) -> Result<WriteState> {
        self.ensure_current()?;
        let metadata = self.file.metadata()?;
        if SourceVersion::of(&metadata) != self.version {
            return Err(Error::Changed);
        }
        Ok(WriteState {
            head: self.head,
            session: self.session,
            sequence: self.items.len() as u64,
            committed: self.committed_bytes,
            metadata,
        })
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_record_limit(path, DEFAULT_RECORD_LIMIT)
    }

    pub fn open_with_record_limit(path: impl AsRef<Path>, max_record_bytes: usize) -> Result<Self> {
        if max_record_bytes == 0 {
            return Err(Error::RecordLimit(0));
        }
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(Error::Invalid("session is not a regular file".into()));
        }
        let version = SourceVersion::of(&metadata);
        let reader = BufReader::with_capacity(64 * 1024, file.try_clone()?.take(version.size));
        let mut scanner = Scanner {
            reader,
            max: max_record_bytes,
            offset: 0,
            bytes: Vec::new(),
        };
        let mut records = Vec::new();
        let mut session = None;
        let mut item_count = 0u64;
        let mut inherited_through = 0u64;
        let mut digest = Sha256::new();
        let mut committed_bytes = 0;
        while let Some(line) = scanner.next()? {
            let line_number = records.len() as u64 + 1;
            if records.is_empty() {
                let Record::Session(header) =
                    Record::decode(line).map_err(|source| Error::Decode {
                        line: line_number,
                        source,
                    })?
                else {
                    return Err(Error::Invalid("session record must be first".into()));
                };
                if header.version != FORMAT_VERSION {
                    return Err(Error::Invalid(format!(
                        "unsupported session format version {}",
                        header.version
                    )));
                }
                validate_session_id(&header.session.id)?;
                if header.session.created_at.is_zero() {
                    return Err(Error::Invalid("session creation time is zero".into()));
                }
                session = Some(header.session);
            } else {
                match crate::peek::marker(line)? {
                    crate::peek::Marker::Session => {
                        return Err(Error::Invalid("session record must be first".into()));
                    }
                    crate::peek::Marker::Item { fork } => {
                        item_count += 1;
                        if fork {
                            inherited_through = item_count;
                        }
                    }
                    _ => {}
                }
            }
            let location = Location {
                offset: committed_bytes,
                length: line.len(),
                digest: Sha256::digest(line).into(),
            };
            digest.update(line);
            digest.update(b"\n");
            committed_bytes += line.len() as u64 + 1;
            records.push(location);
        }
        // The discovery scratch may be as large as one record. Do not retain
        // it while replay allocates its own decode buffers.
        drop(scanner);
        let session =
            session.ok_or_else(|| Error::Invalid("session log has no committed records".into()))?;
        let mut log = Self {
            file,
            path,
            version,
            session,
            generation: format!("{:x}", digest.finalize()),
            committed_bytes,
            records,
            items: Vec::new(),
            head: Head::default(),
        };
        log.ensure_current()?;
        log.replay(inherited_through)?;
        log.ensure_current()?;
        Ok(log)
    }

    fn replay(&mut self, inherited_through: u64) -> Result<()> {
        for location in self.records.iter().copied() {
            match self.read_record_unchecked(location)? {
                Record::Item(value) => {
                    if value.item.sequence != self.items.len() as u64 + 1
                        || value.item.recorded_at.is_zero()
                    {
                        return Err(Error::Invalid(
                            "non-contiguous item sequence or zero recorded time".into(),
                        ));
                    }
                    if !matches!(value.item.data, ItemData::ToolCallStatus(_))
                        && !value.operations.is_empty()
                    {
                        return Err(Error::Invalid(
                            "non-status item initializes operations".into(),
                        ));
                    }
                    let inherited = value.item.sequence <= inherited_through;
                    let operations = if inherited {
                        if !value.operations.is_empty() {
                            return Err(Error::Invalid(
                                "inherited item initializes operations".into(),
                            ));
                        }
                        self.head.inherit(&value);
                        Vec::new()
                    } else {
                        self.head.apply(&value, location)?
                    };
                    self.items.push(ItemLocation {
                        record: location,
                        inherited,
                        operations,
                    });
                }
                Record::Operation(value) => {
                    if (self.items.len() as u64) < inherited_through {
                        return Err(Error::Invalid(
                            "operation record appears in inherited history".into(),
                        ));
                    }
                    self.head.save(
                        &value.operation,
                        OperationRef::Indexed {
                            record: location,
                            in_item: None,
                        },
                    )?;
                }
                Record::Session(_) | Record::Checkpoint(_) => {}
            }
        }
        Ok(())
    }

    pub(crate) fn fork_boundary(&self, previous_turn_id: &str) -> Result<u64> {
        self.ensure_current()?;
        let mut boundary = None;
        for (index, source) in self.items.iter().enumerate() {
            let Record::Item(record) = self.read_record_unchecked(source.record)? else {
                return Err(Error::Changed);
            };
            if boundary.is_some() && matches!(record.item.data, ItemData::Turn(_)) {
                break;
            }
            let turn_id = match &record.item.data {
                ItemData::Turn(value) => Some(value.id.as_str()),
                ItemData::ModelResponse(value) => Some(value.turn_id.as_str()),
                ItemData::ToolCallStatus(value) => Some(value.turn_id.as_str()),
                _ => None,
            };
            if turn_id == Some(previous_turn_id) {
                boundary = Some(index as u64 + 1);
            }
        }
        self.ensure_current()?;
        boundary.ok_or_else(|| Error::Invalid("fork turn was not found".into()))
    }

    pub(crate) fn inherited_item(&self, sequence: u64) -> Result<kou_domain::ItemRecord> {
        self.ensure_current()?;
        let index = usize::try_from(sequence)
            .ok()
            .and_then(|n| n.checked_sub(1))
            .ok_or_else(|| Error::Invalid("invalid item sequence".into()))?;
        let source = self
            .items
            .get(index)
            .ok_or_else(|| Error::Invalid("item sequence out of range".into()))?;
        let Record::Item(mut record) = self.read_record_unchecked(source.record)? else {
            return Err(Error::Changed);
        };
        record.operations.clear();
        if let ItemData::ToolCallStatus(status) = &mut record.item.data {
            status.operations.clear();
        }
        Ok(record)
    }

    pub fn session(&self) -> &Session {
        &self.session
    }
    pub fn generation(&self) -> &str {
        &self.generation
    }
    pub fn item_count(&self) -> usize {
        self.items.len()
    }
    pub fn committed_bytes(&self) -> u64 {
        self.committed_bytes
    }

    pub fn summary(&self) -> LogSummary<'_> {
        LogSummary {
            session: &self.session,
            generation: &self.generation,
            committed_bytes: self.committed_bytes,
            ignored_tail_bytes: self.version.size - self.committed_bytes,
            records: self.records.len(),
            items: self.items.len(),
            operations: self.head.operations.len(),
            retained_index_bytes: self.records.capacity() * size_of::<Location>()
                + self.items.capacity() * size_of::<ItemLocation>()
                + self
                    .items
                    .iter()
                    .map(|i| i.operations.capacity() * size_of::<OperationRef>())
                    .sum::<usize>(),
        }
    }

    pub fn ensure_current(&self) -> Result<()> {
        if SourceVersion::of(&self.file.metadata()?) != self.version
            || SourceVersion::of(&std::fs::metadata(&self.path)?) != self.version
        {
            return Err(Error::Changed);
        }
        Ok(())
    }

    fn read_record_unchecked(&self, at: Location) -> Result<Record> {
        let mut bytes = vec![0; at.length];
        self.file.read_exact_at(&mut bytes, at.offset)?;
        if <[u8; 32]>::from(Sha256::digest(&bytes)) != at.digest {
            return Err(Error::Changed);
        }
        Record::decode(&bytes).map_err(|source| Error::Decode {
            line: at.offset,
            source,
        })
    }

    fn operation(&self, source: &OperationRef) -> Result<Operation> {
        source.load(&self.file, self.committed_bytes)
    }

    pub fn estimated_resident_bytes(&self) -> usize {
        self.summary().retained_index_bytes
            + self.head.estimated_resident_bytes()
            + self.path.as_os_str().len()
            + self.session.id.capacity()
            + self.generation.capacity()
    }

    pub fn item_source_bytes(&self, sequence: u64) -> Result<usize> {
        let index = sequence
            .checked_sub(1)
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(|| Error::Invalid("invalid item sequence".into()))?;
        let item = self
            .items
            .get(index)
            .ok_or_else(|| Error::Invalid("item sequence out of range".into()))?;
        Ok(item.operations.iter().fold(item.record.length, |sum, op| {
            sum.saturating_add(op.source_bytes())
        }))
    }

    pub fn item(&self, sequence: u64) -> Result<Item> {
        self.ensure_current()?;
        let index = sequence
            .checked_sub(1)
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(|| Error::Invalid("invalid item sequence".into()))?;
        let location = self
            .items
            .get(index)
            .ok_or_else(|| Error::Invalid("item sequence out of range".into()))?;
        let Record::Item(mut record) = self.read_record_unchecked(location.record)? else {
            return Err(Error::Changed);
        };
        if let ItemData::ToolCallStatus(status) = &mut record.item.data {
            status.operations.clear();
            if !location.inherited {
                for source in &location.operations {
                    status.operations.push(self.operation(source)?);
                }
            }
        }
        self.ensure_current()?;
        Ok(record.item)
    }

    pub fn items(&self, after: u64, limit: usize) -> Result<ItemPage> {
        if limit == 0 {
            return Err(Error::Invalid("item page limit must be positive".into()));
        }
        self.ensure_current()?;
        let start = usize::try_from(after)
            .unwrap_or(usize::MAX)
            .min(self.items.len());
        let end = start.saturating_add(limit).min(self.items.len());
        let mut items = Vec::with_capacity(end - start);
        for index in start..end {
            items.push(self.item(index as u64 + 1)?);
        }
        let next_after = items.last().map_or(after, |i| i.sequence);
        self.ensure_current()?;
        Ok(ItemPage {
            items,
            next_after,
            more: end < self.items.len(),
        })
    }

    /// Full replay fallback, deliberately without a consumer checkpoint.
    /// SessionRecovery provides the accelerated path and its replay tail.
    pub fn resume_full(&self) -> Result<ResumeState> {
        self.ensure_current()?;
        let operations = self
            .head
            .resume_operations(&self.file, self.committed_bytes)?;
        self.ensure_current()?;
        Ok(ResumeState {
            snapshot: Snapshot {
                session: self.session.clone(),
            },
            operations,
            external_input_ids: self.head.external_input_ids.clone(),
            checkpoint: None,
        })
    }

    pub fn copy_committed(&self, mut out: impl std::io::Write) -> Result<()> {
        self.ensure_current()?;
        let mut offset = 0u64;
        let mut bytes = [0u8; 64 * 1024];
        while offset < self.committed_bytes {
            let count =
                usize::try_from((self.committed_bytes - offset).min(bytes.len() as u64)).unwrap();
            self.file.read_exact_at(&mut bytes[..count], offset)?;
            out.write_all(&bytes[..count])?;
            offset += count as u64;
        }
        self.ensure_current()
    }
}

pub(crate) struct Scanner<R> {
    reader: R,
    max: usize,
    pub(crate) offset: u64,
    bytes: Vec<u8>,
}

pub(crate) struct ReadAt<'a> {
    file: &'a File,
    offset: u64,
    end: u64,
}

impl Read for ReadAt<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let count = usize::try_from((self.end - self.offset).min(out.len() as u64)).unwrap();
        let read = self.file.read_at(&mut out[..count], self.offset)?;
        self.offset += read as u64;
        Ok(read)
    }
}

impl<'a> Scanner<BufReader<ReadAt<'a>>> {
    pub(crate) fn at(file: &'a File, start: u64, end: u64, max: usize) -> Self {
        Self {
            reader: BufReader::with_capacity(
                64 * 1024,
                ReadAt {
                    file,
                    offset: start,
                    end,
                },
            ),
            max,
            offset: start,
            bytes: Vec::new(),
        }
    }
}

impl<R: BufRead> Scanner<R> {
    pub(crate) fn next(&mut self) -> Result<Option<&[u8]>> {
        self.bytes.clear();
        let mut oversized = false;
        loop {
            let part = self.reader.fill_buf()?;
            if part.is_empty() {
                return Ok(None);
            } // The uncommitted final record is ignored, even when malformed.
            let newline = part.iter().position(|b| *b == b'\n');
            let used = newline.map_or(part.len(), |at| at + 1);
            if !oversized && used <= self.max.saturating_sub(self.bytes.len()) {
                self.bytes.extend_from_slice(&part[..used]);
            } else {
                oversized = true;
            }
            self.offset += used as u64;
            self.reader.consume(used);
            if newline.is_some() {
                if oversized {
                    return Err(Error::RecordLimit(self.max));
                }
                return Ok(Some(&self.bytes[..self.bytes.len() - 1]));
            }
        }
    }
}
