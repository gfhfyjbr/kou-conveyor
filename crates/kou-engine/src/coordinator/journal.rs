use super::*;
use kou_domain::{ItemData, Json};
use kou_store::SessionWriter;
use std::{future::Future, pin::Pin, thread::JoinHandle};
use tokio::sync::{mpsc, oneshot};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Journal: Send {
    fn session_id(&self) -> &str;
    fn append(&mut self, data: ItemData, at: Timestamp) -> BoxFuture<'_, Result<u64>>;
    fn save_operation(&mut self, operation: Operation) -> BoxFuture<'_, Result<()>>;
    fn supports_checkpoints(&self) -> bool {
        false
    }
    fn checkpoint(&mut self, state: Json) -> BoxFuture<'_, Result<()>>;
}
enum Command {
    Append(ItemData, Timestamp, oneshot::Sender<Result<u64>>),
    Operation(Operation, oneshot::Sender<Result<()>>),
    Checkpoint(Json, oneshot::Sender<Result<()>>),
}

/// One dedicated writer owns the lock and fsyncs. Dropping a waiting future does
/// not abandon the write thread: Drop closes its queue and joins it. The store's
/// poisoned-writer contract remains authoritative after an uncertain I/O error.
pub struct FileJournal {
    id: String,
    sender: Option<mpsc::Sender<Command>>,
    worker: Option<JoinHandle<()>>,
}
impl FileJournal {
    pub fn new(mut writer: SessionWriter) -> Result<Self> {
        let id = writer.session().id.clone();
        let (sender, mut receiver) = mpsc::channel(1);
        let worker = std::thread::Builder::new()
            .name("kou-session-writer".into())
            .spawn(move || {
                while let Some(command) = receiver.blocking_recv() {
                    match command {
                        Command::Append(data, at, reply) => {
                            let result = writer
                                .append_item(data, at)
                                .map_err(|error| Error(error.to_string()));
                            let _ = reply.send(result);
                        }
                        Command::Operation(operation, reply) => {
                            let result = writer
                                .save_operation(operation)
                                .map_err(|error| Error(error.to_string()));
                            let _ = reply.send(result);
                        }
                        Command::Checkpoint(state, reply) => {
                            let result = writer
                                .save_checkpoint(state)
                                .map(|_| ())
                                .map_err(|error| Error(error.to_string()));
                            let _ = reply.send(result);
                        }
                    }
                }
            })
            .map_err(|error| Error(format!("start session writer: {error}")))?;
        Ok(Self {
            id,
            sender: Some(sender),
            worker: Some(worker),
        })
    }
    async fn send<T>(&self, command: Command, response: oneshot::Receiver<Result<T>>) -> Result<T> {
        self.sender
            .as_ref()
            .expect("journal is live")
            .send(command)
            .await
            .map_err(|_| Error("session writer closed".into()))?;
        response
            .await
            .map_err(|_| Error("session writer closed".into()))?
    }
}
impl Journal for FileJournal {
    fn session_id(&self) -> &str {
        &self.id
    }
    fn append(&mut self, data: ItemData, at: Timestamp) -> BoxFuture<'_, Result<u64>> {
        Box::pin(async move {
            let (reply, response) = oneshot::channel();
            self.send(Command::Append(data, at, reply), response).await
        })
    }
    fn save_operation(&mut self, operation: Operation) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let (reply, response) = oneshot::channel();
            self.send(Command::Operation(operation, reply), response)
                .await
        })
    }
    fn supports_checkpoints(&self) -> bool {
        true
    }
    fn checkpoint(&mut self, state: Json) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let (reply, response) = oneshot::channel();
            self.send(Command::Checkpoint(state, reply), response).await
        })
    }
}
impl Drop for FileJournal {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
