//! Shared replay/checkpoint state and the single-owner asynchronous coordinator.
//! Replay only translates recorded results; live dispatch is behind journaled ports.
use crate::context::{Builder, Error, Result};
use kou_domain::{CallStatus, LlmData, LlmItem, Operation, Timestamp, ToolCall, ToolResultOutput};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
mod channels;
mod checkpoint;
mod driver;
mod inspection;
mod journal;
mod model;
mod replay;
pub use channels::{Channels, InputHandle, OperationHandle, QueueLimits, channels};
pub use driver::{
    Compaction, Config, Coordinator, Hooks, LiveTools, OperationSpec, Operations, Submission,
    VERIFICATION_MARKER, Verifier,
};
pub use journal::{BoxFuture, FileJournal, Journal};
pub use model::{CancellationToken, Model, ModelError, ModelFailure, ModelResult};
#[cfg(test)]
mod tests;

/// The runner supplies its actual pure result translators, separately from the
/// capabilities that execute operations. Restore must never execute tool calls.
pub trait ReplayTools: Send + Sync {
    fn available(&self, name: &str) -> bool;
    fn result(
        &self,
        call: &ToolCall,
        status: &CallStatus,
        operations: &[Arc<Operation>],
    ) -> Result<Vec<ToolResultOutput>>;
    fn running(&self, _call: &ToolCall, _operations: &[Arc<Operation>]) -> String {
        String::new()
    }
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct CallKey {
    turn: String,
    call: String,
}
struct ActiveCall {
    item: Arc<LlmItem>,
    status: Option<CallStatus>,
    operations: BTreeSet<String>,
}
impl ActiveCall {
    fn call(&self) -> &ToolCall {
        let LlmData::ToolCall(call) = &self.item.data else {
            unreachable!()
        };
        call
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct RecordedStop {
    at: Timestamp,
    reason: String,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct ReplayHistory {
    work_since_verification: bool,
    verifications: i64,
    stopped: Option<RecordedStop>,
}
struct State {
    current_turn: String,
    current_turn_type: String,
    turn_models: BTreeMap<String, String>,
    compaction_turns: BTreeSet<String>,
    calls: BTreeMap<CallKey, ActiveCall>,
    operations: BTreeMap<String, Arc<Operation>>,
    verifying: Option<String>,
    available_inputs: i64,
    delivered_inputs: i64,
    current_turn_inputs: i64,
    compacted: bool,
    compaction_asked: bool,
    turns_since_compaction: i64,
    compaction_failures: i64,
    rapid_refills: i64,
    reminded: bool,
    work_since_verification: bool,
    verifications: i64,
    heartbeats: i64,
    file_changes: i64,
    stopped: Option<RecordedStop>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            current_turn: String::new(),
            current_turn_type: String::new(),
            turn_models: BTreeMap::new(),
            compaction_turns: BTreeSet::new(),
            calls: BTreeMap::new(),
            operations: BTreeMap::new(),
            verifying: None,
            available_inputs: 0,
            delivered_inputs: 0,
            current_turn_inputs: 0,
            compacted: false,
            compaction_asked: false,
            turns_since_compaction: -1,
            compaction_failures: 0,
            rapid_refills: 0,
            reminded: false,
            work_since_verification: false,
            verifications: 0,
            heartbeats: 0,
            file_changes: 0,
            stopped: None,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Progress {
    pub pending_inputs: i64,
    pub file_changes: i64,
    pub verifications: i64,
    pub heartbeats: i64,
    pub work_since_verification: bool,
}

pub struct History {
    pub context: Builder,
    tools: Arc<dyn ReplayTools>,
    state: State,
    history: ReplayHistory,
}
impl History {
    pub fn new(context: Builder, tools: Arc<dyn ReplayTools>) -> Self {
        Self {
            context,
            tools,
            state: State::default(),
            history: ReplayHistory::default(),
        }
    }
    pub fn verifying_operation(&self) -> Option<&Operation> {
        self.state
            .verifying
            .as_ref()
            .and_then(|id| self.state.operations.get(id))
            .map(Arc::as_ref)
    }
    pub fn progress(&self) -> Progress {
        Progress {
            pending_inputs: self.state.available_inputs - self.state.delivered_inputs,
            file_changes: self.state.file_changes,
            verifications: self.state.verifications,
            heartbeats: self.state.heartbeats,
            work_since_verification: self.state.work_since_verification,
        }
    }
    pub fn operations(&self) -> impl Iterator<Item = &Operation> {
        self.state.operations.values().map(Arc::as_ref)
    }
    pub fn apply_operation(&mut self, operation: Operation) {
        self.state
            .operations
            .insert(operation.id.clone(), Arc::new(operation));
    }
}
fn call_key(turn: &str, call: &str) -> CallKey {
    CallKey {
        turn: turn.into(),
        call: call.into(),
    }
}
fn changes_files(name: &str) -> bool {
    let names = ["Edit", "Write", "apply_patch"];
    names.contains(&name)
        || names
            .iter()
            .filter(|suffix| {
                name.match_indices('_')
                    .any(|(at, _)| at > 0 && name.get(at + 1..) == Some(*suffix))
            })
            .count()
            == 1
}
