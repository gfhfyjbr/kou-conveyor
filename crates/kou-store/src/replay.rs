use crate::{
    Error, Result,
    index::{Location, OperationRef},
};
use kou_domain::{ItemData, ItemOperation, ItemRecord, Operation, ToolCallStatus};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    io::{self, Write},
};

#[derive(Clone, Copy, Debug)]
pub(crate) enum Status {
    Ready,
    Awaiting,
    Canceling,
    Completed,
    Failed,
    Canceled,
}

impl Status {
    fn parse(s: &str) -> Result<Self> {
        match s {
            "ready" => Ok(Self::Ready),
            "awaiting" => Ok(Self::Awaiting),
            "canceling" => Ok(Self::Canceling),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "canceled" => Ok(Self::Canceled),
            _ => Err(Error::Invalid("unsupported operation status".into())),
        }
    }
    pub(crate) fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Canceled)
    }
}

#[derive(Debug)]
pub(crate) struct OperationSlot {
    pub(crate) kind: Box<str>,
    pub(crate) version: u32,
    pub(crate) status: Status,
    pub(crate) source: OperationRef,
    pub(crate) fingerprint: Option<[u8; 32]>,
    pub(crate) max_output_length: i64,
}

#[derive(Default)]
pub(crate) struct Head {
    pub(crate) latest_turn: String,
    turns: HashSet<String>,
    owned: HashSet<String>,
    responded: HashSet<String>,
    calls: HashSet<(String, String)>,
    pub(crate) operations: Vec<OperationSlot>,
    pub(crate) operation_ids: HashMap<String, usize>,
    pub(crate) pending: HashSet<String>,
    pub(crate) external_input_ids: Vec<String>,
}

pub(crate) fn fingerprint(operation: &Operation) -> Result<[u8; 32]> {
    struct HashWriter(Sha256);
    impl Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter(Sha256::new());
    kou_domain::write_compact_json(&mut writer, operation)
        .map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(writer.0.finalize().into())
}

impl Head {
    pub(crate) fn estimated_resident_bytes(&self) -> usize {
        fn strings(values: &HashSet<String>) -> usize {
            values.capacity() * (size_of::<String>() + 1) * 2
                + values.iter().map(String::capacity).sum::<usize>()
        }
        self.latest_turn.capacity()
            + strings(&self.turns)
            + strings(&self.owned)
            + strings(&self.responded)
            + strings(&self.pending)
            + self.calls.capacity() * (size_of::<(String, String)>() + 1) * 2
            + self
                .calls
                .iter()
                .map(|(turn, call)| turn.capacity() + call.capacity())
                .sum::<usize>()
            + self.operation_ids.capacity() * (size_of::<(String, usize)>() + 1) * 2
            + self
                .operation_ids
                .keys()
                .map(String::capacity)
                .sum::<usize>()
            + self.operations.capacity() * size_of::<OperationSlot>()
            + self
                .operations
                .iter()
                .map(|slot| slot.kind.len())
                .sum::<usize>()
            + self.external_input_ids.capacity() * size_of::<String>()
            + self
                .external_input_ids
                .iter()
                .map(String::capacity)
                .sum::<usize>()
    }

    fn reset_owned(&mut self) {
        self.owned.clear();
        self.responded.clear();
        self.calls.clear();
        self.operations.clear();
        self.operation_ids.clear();
        self.pending.clear();
    }

    pub(crate) fn inherit(&mut self, record: &ItemRecord) {
        match &record.item.data {
            ItemData::Input(v) if v.kind == "external" => {
                self.external_input_ids.push(v.id.clone())
            }
            ItemData::Turn(v) => {
                self.turns.insert(v.id.clone());
                self.owned.insert(v.id.clone());
                self.latest_turn.clone_from(&v.id);
            }
            ItemData::ModelResponse(v) => {
                self.responded.insert(v.turn_id.clone());
            }
            ItemData::ToolCallStatus(v) => {
                self.calls.insert((v.turn_id.clone(), v.call_id.clone()));
            }
            ItemData::Fork(_) => self.reset_owned(),
            _ => {}
        }
    }

    pub(crate) fn check_item(&self, record: &ItemRecord) -> Result<()> {
        if !matches!(record.item.data, ItemData::ToolCallStatus(_)) && !record.operations.is_empty()
        {
            return Err(Error::Invalid(
                "non-status item initializes operations".into(),
            ));
        }
        match &record.item.data {
            ItemData::Input(v) => v.validate().map_err(Error::Invalid),
            ItemData::Turn(v) => {
                if v.id.is_empty()
                    || v.previous_turn_id != self.latest_turn
                    || self.turns.contains(&v.id)
                {
                    return Err(Error::Invalid(
                        "empty/duplicate turn ID or incorrect previous turn".into(),
                    ));
                }
                Ok(())
            }
            ItemData::ModelResponse(v) => {
                if !self.owned.contains(&v.turn_id) || self.responded.contains(&v.turn_id) {
                    return Err(Error::Invalid(
                        "model response has no owned turn or repeats a response".into(),
                    ));
                }
                Ok(())
            }
            ItemData::ToolCallStatus(v) => self.check_status(v, &record.operations),
            ItemData::Fork(_) => Err(Error::Invalid("unexpected owned fork".into())),
        }
    }

    fn check_status(&self, status: &ToolCallStatus, values: &[ItemOperation]) -> Result<()> {
        if !self.owned.contains(&status.turn_id) || status.call_id.is_empty() {
            return Err(Error::Invalid(
                "tool status has no owned turn or call ID".into(),
            ));
        }
        let repeated = self
            .calls
            .contains(&(status.turn_id.clone(), status.call_id.clone()));
        let mut initialized = HashSet::new();
        for value in values {
            match value {
                ItemOperation::Saved(id) => {
                    if !repeated || !self.operation_ids.contains_key(id) {
                        return Err(Error::Invalid(
                            "status references an unsaved operation".into(),
                        ));
                    }
                }
                ItemOperation::Full(op) if !repeated => {
                    op.validate().map_err(Error::Invalid)?;
                    if self.operation_ids.contains_key(&op.id) || !initialized.insert(op.id.clone())
                    {
                        return Err(Error::Invalid("duplicate initialized operation".into()));
                    }
                }
                _ => {}
            }
        }
        if !repeated {
            validate_status(status, &initialized).map_err(|error| {
                Error::Invalid(format!(
                    "append tool-call status for turn {:?} and call {:?}: {error}",
                    status.turn_id, status.call_id
                ))
            })?;
        }
        Ok(())
    }

    pub(crate) fn apply(
        &mut self,
        record: &ItemRecord,
        location: Location,
    ) -> Result<Vec<OperationRef>> {
        self.check_item(record)?;
        match &record.item.data {
            ItemData::Input(v) if v.kind == "external" => {
                self.external_input_ids.push(v.id.clone())
            }
            ItemData::Turn(v) => {
                self.turns.insert(v.id.clone());
                self.owned.insert(v.id.clone());
                self.latest_turn.clone_from(&v.id);
            }
            ItemData::ModelResponse(v) => {
                self.responded.insert(v.turn_id.clone());
            }
            ItemData::ToolCallStatus(v) => return self.status(v, &record.operations, location),
            _ => {}
        }
        Ok(Vec::new())
    }

    fn status(
        &mut self,
        status: &ToolCallStatus,
        values: &[ItemOperation],
        location: Location,
    ) -> Result<Vec<OperationRef>> {
        let key = (status.turn_id.clone(), status.call_id.clone());
        let repeated = self.calls.contains(&key);
        let mut resolved = Vec::with_capacity(values.len());
        for (index, value) in values.iter().enumerate() {
            let (id, terminal, source) = match value {
                ItemOperation::Saved(id) => {
                    let slot = &self.operations[self.operation_ids[id]];
                    (id, slot.status.terminal(), slot.source.clone())
                }
                ItemOperation::Full(op) => {
                    let source = OperationRef::Indexed {
                        record: location,
                        in_item: Some(index),
                    };
                    if !repeated {
                        self.operation_ids
                            .insert(op.id.clone(), self.operations.len());
                        self.operations.push(OperationSlot {
                            kind: op.kind.clone().into_boxed_str(),
                            version: op.version,
                            status: Status::parse(&op.status)?,
                            source: source.clone(),
                            fingerprint: Some(fingerprint(op)?),
                            max_output_length: op.max_output_length,
                        });
                    }
                    (&op.id, op.terminal(), source)
                }
            };
            if terminal {
                self.pending.remove(id);
            } else {
                self.pending.insert(id.clone());
            }
            resolved.push(source);
        }
        self.calls.insert(key);
        Ok(resolved)
    }

    pub(crate) fn check_save(&self, op: &Operation) -> Result<()> {
        op.validate().map_err(Error::Invalid)?;
        let index = self
            .operation_ids
            .get(&op.id)
            .ok_or_else(|| Error::Invalid("operation was not initialized".into()))?;
        let slot = &self.operations[*index];
        if slot.kind.as_ref() != op.kind || slot.version != op.version {
            return Err(Error::Invalid(
                "operation type/version cannot change".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn save(&mut self, op: &Operation, source: OperationRef) -> Result<()> {
        self.check_save(op)?;
        let hash = fingerprint(op)?;
        let slot = &mut self.operations[self.operation_ids[&op.id]];
        slot.status = Status::parse(&op.status)?;
        slot.source = source;
        slot.fingerprint = Some(hash);
        slot.max_output_length = op.max_output_length;
        Ok(())
    }

    pub(crate) fn same_operation(&self, op: &Operation) -> Result<bool> {
        let Some(index) = self.operation_ids.get(&op.id) else {
            return Ok(false);
        };
        Ok(self.operations[*index].fingerprint == Some(fingerprint(op)?))
    }

    pub(crate) fn make_checkpoint(
        &self,
        session: &kou_domain::Session,
        sequence: u64,
        file: &std::fs::File,
        committed: u64,
    ) -> Result<crate::checkpoint::CheckpointHead> {
        use crate::checkpoint::{Call, CheckpointHead, CheckpointOperation};
        let mut ids: Vec<_> = self.operation_ids.iter().collect();
        ids.sort_unstable_by_key(|(_, index)| **index);
        let mut operations = Vec::with_capacity(ids.len());
        for (id, index) in ids {
            let slot = &self.operations[*index];
            let pending = self.pending.contains(id);
            let archived = slot.status.terminal() && !pending;
            let operation = if archived {
                Operation {
                    id: id.clone(),
                    kind: slot.kind.to_string(),
                    version: slot.version,
                    status: match slot.status {
                        Status::Ready => "ready",
                        Status::Awaiting => "awaiting",
                        Status::Canceling => "canceling",
                        Status::Completed => "completed",
                        Status::Failed => "failed",
                        Status::Canceled => "canceled",
                    }
                    .into(),
                    max_output_length: slot.max_output_length,
                    ..Operation::default()
                }
            } else {
                slot.source.load(file, committed)?
            };
            operations.push(CheckpointOperation {
                operation,
                offset: slot.source.offset() + 1,
                archived,
                pending,
            });
        }
        fn sorted(values: &HashSet<String>) -> Vec<String> {
            let mut values: Vec<_> = values.iter().cloned().collect();
            values.sort_unstable();
            values
        }
        let mut calls: Vec<_> = self
            .calls
            .iter()
            .map(|(turn, call)| Call {
                turn: turn.clone(),
                call: call.clone(),
            })
            .collect();
        calls.sort_unstable_by(|a, b| (&a.turn, &a.call).cmp(&(&b.turn, &b.call)));
        Ok(CheckpointHead {
            snapshot: kou_domain::Snapshot {
                session: session.clone(),
            },
            sequence,
            latest_turn: self.latest_turn.clone(),
            turns: sorted(&self.turns),
            owned_turns: sorted(&self.owned),
            responded_turns: sorted(&self.responded),
            calls,
            operations,
            external_input_ids: self.external_input_ids.clone(),
        })
    }

    pub(crate) fn restore_checkpoint(
        value: crate::checkpoint::CheckpointHead,
        session: &kou_domain::Session,
        offset: u64,
    ) -> Result<Self> {
        if value.snapshot.session != *session {
            return Err(Error::Invalid("checkpoint session mismatch".into()));
        }
        fn ids(values: Vec<String>) -> Result<HashSet<String>> {
            let count = values.len();
            let ids: HashSet<_> = values.into_iter().collect();
            if ids.len() != count || ids.contains("") {
                return Err(Error::Invalid("duplicate or empty checkpoint turn".into()));
            }
            Ok(ids)
        }
        let mut head = Self {
            latest_turn: value.latest_turn,
            turns: ids(value.turns)?,
            owned: ids(value.owned_turns)?,
            responded: ids(value.responded_turns)?,
            external_input_ids: value.external_input_ids,
            ..Self::default()
        };
        if !head.owned.is_subset(&head.turns)
            || !head.responded.is_subset(&head.owned)
            || (!head.latest_turn.is_empty() && !head.turns.contains(&head.latest_turn))
        {
            return Err(Error::Invalid("checkpoint turn ownership mismatch".into()));
        }
        for call in value.calls {
            if call.call.is_empty()
                || !head.owned.contains(&call.turn)
                || !head.calls.insert((call.turn, call.call))
            {
                return Err(Error::Invalid(
                    "invalid or duplicate checkpoint call".into(),
                ));
            }
        }
        for entry in value.operations {
            let op = entry.operation;
            op.validate().map_err(Error::Invalid)?;
            if entry.offset == 0
                || entry.offset > offset
                || (entry.archived && (!op.terminal() || entry.pending))
                || head.operation_ids.contains_key(&op.id)
            {
                return Err(Error::Invalid("invalid checkpoint operation".into()));
            }
            if entry.pending {
                head.pending.insert(op.id.clone());
            }
            head.operation_ids
                .insert(op.id.clone(), head.operations.len());
            head.operations.push(OperationSlot {
                kind: op.kind.clone().into_boxed_str(),
                version: op.version,
                status: Status::parse(&op.status)?,
                max_output_length: op.max_output_length,
                fingerprint: if entry.archived {
                    None
                } else {
                    Some(fingerprint(&op)?)
                },
                source: OperationRef::Checkpoint {
                    offset: entry.offset - 1,
                    operation: std::sync::Arc::new(op),
                    archived: entry.archived,
                },
            });
        }
        Ok(head)
    }

    pub(crate) fn resume_operations(
        &self,
        file: &std::fs::File,
        committed: u64,
    ) -> Result<Vec<Operation>> {
        let mut selected: Vec<_> = self
            .operation_ids
            .iter()
            .filter_map(|(id, index)| {
                let slot = &self.operations[*index];
                (!slot.status.terminal() || self.pending.contains(id)).then_some(*index)
            })
            .collect();
        selected.sort_unstable();
        selected
            .into_iter()
            .map(|index| self.operations[index].source.load(file, committed))
            .collect()
    }
}

fn validate_status(status: &ToolCallStatus, initialized: &HashSet<String>) -> Result<()> {
    let waiting = status.status.waiting();
    if !status.status.error.is_empty() {
        if !waiting.is_empty() {
            return Err(Error::Invalid(
                "error status cannot wait for operations".into(),
            ));
        }
        if !initialized.is_empty() {
            return Err(Error::Invalid(
                "error status cannot initialize operations".into(),
            ));
        }
        return Ok(());
    }
    if initialized.is_empty() {
        return Err(Error::Invalid(
            "successful status must initialize at least one operation".into(),
        ));
    }
    if waiting.len() != initialized.len() {
        return Err(Error::Invalid(format!(
            "status waits for {} operations, initialized {}",
            waiting.len(),
            initialized.len()
        )));
    }
    let mut seen = HashSet::new();
    for id in waiting {
        if !seen.insert(id) {
            return Err(Error::Invalid(format!("status repeats operation {id:?}")));
        }
        if !initialized.contains(id) {
            return Err(Error::Invalid(format!(
                "status waits for operation {id:?} that was not initialized"
            )));
        }
    }
    Ok(())
}
