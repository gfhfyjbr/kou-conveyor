use super::*;
use kou_domain::{Input, Json};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};

#[derive(Clone, Copy, Debug)]
pub struct QueueLimits {
    pub events: usize,
    pub bytes: usize,
    pub event_bytes: usize,
}
impl Default for QueueLimits {
    fn default() -> Self {
        Self {
            events: 256,
            bytes: 32 << 20,
            event_bytes: 16 << 20,
        }
    }
}
struct Admission {
    count: Arc<Semaphore>,
    bytes: Arc<Semaphore>,
    event_bytes: usize,
}
impl Admission {
    fn new(limits: QueueLimits) -> Result<Arc<Self>> {
        if !(1..=4096).contains(&limits.events)
            || limits.bytes == 0
            || limits.bytes > u32::MAX as usize
            || limits.event_bytes == 0
            || limits.event_bytes > limits.bytes
        {
            return Err(Error("invalid coordinator queue limits".into()));
        }
        Ok(Arc::new(Self {
            count: Arc::new(Semaphore::new(limits.events)),
            bytes: Arc::new(Semaphore::new(limits.bytes)),
            event_bytes: limits.event_bytes,
        }))
    }
    async fn admit<T>(&self, value: T, size: usize) -> Result<Queued<T>> {
        if size > self.event_bytes {
            return Err(Error("coordinator event exceeds its byte limit".into()));
        }
        let count = self.count.clone().acquire_owned().await.map_err(closed)?;
        let bytes = self
            .bytes
            .clone()
            .acquire_many_owned(size as u32)
            .await
            .map_err(closed)?;
        Ok(Queued {
            value,
            _count: count,
            _bytes: bytes,
        })
    }
    fn close(&self) {
        self.count.close();
        self.bytes.close();
    }
}
fn closed(_: impl std::fmt::Display) -> Error {
    Error("coordinator input is closed".into())
}
pub(super) struct Queued<T> {
    pub(super) value: T,
    _count: OwnedSemaphorePermit,
    _bytes: OwnedSemaphorePermit,
}
#[derive(Clone)]
pub struct InputHandle {
    sender: mpsc::Sender<Queued<Input>>,
    ordinary: Arc<Admission>,
    controls: Arc<Admission>,
}
impl InputHandle {
    /// Admission is not a durable acknowledgement. In particular, after-tools
    /// input stays with its sender until the session log records its ID.
    pub async fn submit(&self, input: Input) -> Result<()> {
        input.validate().map_err(Error)?;
        let size = std::mem::size_of::<Input>()
            + input.id.len()
            + input.kind.len()
            + input.delivery.len()
            + json_bytes(&input.payload);
        let admission = if input.kind == "control" {
            &self.controls
        } else {
            &self.ordinary
        };
        let queued = admission.admit(input, size).await?;
        self.sender.send(queued).await.map_err(closed)
    }
}
#[derive(Clone)]
pub struct OperationHandle {
    sender: mpsc::Sender<Queued<Operation>>,
    admission: Arc<Admission>,
}
impl OperationHandle {
    pub async fn submit(&self, operation: Operation) -> Result<()> {
        operation.validate().map_err(Error)?;
        let size = std::mem::size_of::<Operation>()
            + operation.id.len()
            + operation.kind.len()
            + operation.status.len()
            + json_bytes(&operation.state)
            + json_bytes(&operation.idempotency);
        let queued = self.admission.admit(operation, size).await?;
        self.sender.send(queued).await.map_err(closed)
    }
}
fn json_bytes(value: &Json) -> usize {
    value.as_str().map_or(0, str::len)
}
pub struct Channels {
    pub(super) inputs: mpsc::Receiver<Queued<Input>>,
    pub(super) operations: mpsc::Receiver<Queued<Operation>>,
    admissions: [Arc<Admission>; 3],
}
impl Drop for Channels {
    fn drop(&mut self) {
        for admission in &self.admissions {
            admission.close();
        }
    }
}
/// Operation updates and controls have independent budgets: deferred prompts
/// must not consume the admission needed to complete/cancel the tools they await.
pub fn channels(limits: QueueLimits) -> Result<(InputHandle, OperationHandle, Channels)> {
    let ordinary = Admission::new(limits)?;
    let updates = Admission::new(limits)?;
    let controls = Admission::new(QueueLimits {
        events: 16,
        bytes: 1 << 20,
        event_bytes: 64 << 10,
    })?;
    let (input_tx, inputs) = mpsc::channel(limits.events + 16);
    let (operation_tx, operations) = mpsc::channel(limits.events);
    Ok((
        InputHandle {
            sender: input_tx,
            ordinary: ordinary.clone(),
            controls: controls.clone(),
        },
        OperationHandle {
            sender: operation_tx,
            admission: updates.clone(),
        },
        Channels {
            inputs,
            operations,
            admissions: [ordinary, controls, updates],
        },
    ))
}
