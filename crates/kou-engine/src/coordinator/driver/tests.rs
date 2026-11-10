use super::*;
use kou_domain::{LlmItem, Message, Response};
use kou_provider::SharedRequest;
use std::sync::Mutex;
use tokio::sync::{mpsc, oneshot};

mod admission;
mod cases;
mod clock;
mod http;
mod operations;
mod storage;

#[derive(Default)]
struct Saved {
    items: Vec<ItemData>,
    updates: Vec<Operation>,
    adds: Vec<Operation>,
    cancels: Vec<(String, String)>,
    trace: Vec<&'static str>,
    fail: Option<&'static str>,
}
#[derive(Clone, Default)]
struct Memory(Arc<Mutex<Saved>>);
impl Journal for Memory {
    fn session_id(&self) -> &str {
        "live-fixture"
    }
    fn append(&mut self, data: ItemData, _: Timestamp) -> BoxFuture<'_, Result<u64>> {
        Box::pin(async move {
            let mut saved = self.0.lock().unwrap();
            let kind = match data {
                ItemData::Input(_) => "input",
                ItemData::Turn(_) => "turn",
                ItemData::ModelResponse(_) => "response",
                ItemData::ToolCallStatus(_) => "status",
                ItemData::Fork(_) => "fork",
            };
            if saved.fail == Some(kind) {
                return Err(Error(format!("{kind} failed")));
            }
            saved.trace.push(kind);
            saved.items.push(data);
            Ok(saved.items.len() as u64)
        })
    }
    fn save_operation(&mut self, operation: Operation) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let mut saved = self.0.lock().unwrap();
            if saved.fail == Some("update") {
                return Err(Error("update failed".into()));
            }
            saved.trace.push("update");
            saved.updates.push(operation);
            Ok(())
        })
    }
    fn checkpoint(&mut self, _: Json) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { panic!("memory journal does not checkpoint") })
    }
}
impl Operations for Memory {
    fn add(&mut self, operation: Operation) -> Result<()> {
        let mut saved = self.0.lock().unwrap();
        assert!(saved.items.iter().any(|item| matches!(item, ItemData::ToolCallStatus(status) if status.operations.contains(&operation))), "dispatch preceded its durable status");
        saved.trace.push("dispatch");
        saved.adds.push(operation);
        if saved.fail == Some("dispatch") {
            return Err(Error("dispatch failed".into()));
        }
        Ok(())
    }
    fn cancel(&mut self, id: &str, reason: &str) -> Result<()> {
        let mut saved = self.0.lock().unwrap();
        saved.cancels.push((id.into(), reason.into()));
        if saved.fail == Some("cancel") {
            return Err(Error("cancel failed".into()));
        }
        Ok(())
    }
}
struct Tools;
impl ReplayTools for Tools {
    fn available(&self, name: &str) -> bool {
        name != "Missing"
    }
    fn result(
        &self,
        call: &ToolCall,
        status: &CallStatus,
        operations: &[Arc<Operation>],
    ) -> Result<Vec<ToolResultOutput>> {
        if call.name == "Reject" {
            return Err(Error("result failed".into()));
        }
        let text = if !status.error.is_empty() {
            status.error.clone()
        } else if operations.is_empty() {
            "immediate".into()
        } else {
            operations
                .iter()
                .map(|operation| operation.status.as_str())
                .collect::<Vec<_>>()
                .join(",")
        };
        Ok(vec![ToolResultOutput {
            kind: "text".into(),
            value: text,
        }])
    }
    fn running(&self, _: &ToolCall, operations: &[Arc<Operation>]) -> String {
        operations
            .iter()
            .filter_map(|operation| operation.state.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }
}
impl LiveTools for Tools {
    fn translate(&self, call: &ToolCall, submission: &mut Submission) -> CallStatus {
        if call.name == "Immediate" {
            return CallStatus::default();
        }
        let count = call.arguments.parse::<usize>().unwrap_or(1).min(8);
        let waiting_for = (0..count).map(|_| submission.submit(spec())).collect();
        CallStatus {
            waiting_for: Some(waiting_for),
            ..CallStatus::default()
        }
    }
    fn waits(&self, name: &str) -> bool {
        name == "Wait"
    }
}
fn spec() -> OperationSpec {
    OperationSpec {
        kind: "value".into(),
        version: 1,
        state: Json::parse("0").unwrap(),
        idempotency: Json::default(),
        max_output_length: 0,
    }
}
struct Request {
    request: SharedRequest,
    cancellation: CancellationToken,
    reply: oneshot::Sender<ModelResult>,
}
struct FakeModel {
    requests: mpsc::UnboundedSender<Request>,
    saved: Memory,
}
impl Model for FakeModel {
    fn respond(
        self: Arc<Self>,
        request: SharedRequest,
        _: String,
        cancellation: CancellationToken,
    ) -> BoxFuture<'static, ModelResult> {
        let (reply, receive) = oneshot::channel();
        self.saved.0.lock().unwrap().trace.push("model");
        self.requests
            .send(Request {
                request,
                cancellation: cancellation.clone(),
                reply,
            })
            .unwrap_or_else(|_| panic!("test request receiver closed"));
        Box::pin(async move {
            tokio::select! {
                result = receive => result.map_err(|_| ModelError::new("test response closed"))?,
                _ = cancellation.cancelled() => Err(ModelError::new("context canceled")),
            }
        })
    }
}
struct Rig {
    input: InputHandle,
    updates: OperationHandle,
    requests: mpsc::UnboundedReceiver<Request>,
    saved: Memory,
    cancellation: CancellationToken,
    task: tokio::task::JoinHandle<Result<()>>,
}
impl Rig {
    fn new(config: Config) -> Self {
        Self::custom(config, |driver| driver)
    }
    fn custom(config: Config, customize: impl FnOnce(Coordinator) -> Coordinator) -> Self {
        let (input, updates, channels) = channels(QueueLimits::default()).unwrap();
        let (send, requests) = mpsc::unbounded_channel();
        let saved = Memory::default();
        let model = Arc::new(FakeModel {
            requests: send,
            saved: saved.clone(),
        });
        let driver = Coordinator::new(
            Builder::default(),
            Arc::new(Tools),
            Box::new(saved.clone()),
            model,
            Box::new(saved.clone()),
            channels,
            config,
        );
        let cancellation = CancellationToken::new();
        let task = tokio::spawn(customize(driver).run(cancellation.clone()));
        Self {
            input,
            updates,
            requests,
            saved,
            cancellation,
            task,
        }
    }
    async fn send(&self, id: &str, text: &str) {
        self.input.submit(external(id, text, "")).await.unwrap();
    }
    async fn control(&self, mode: &str) {
        self.input.submit(control(mode)).await.unwrap();
    }
    async fn request(&mut self) -> Request {
        tokio::time::timeout(Duration::from_secs(10), self.requests.recv())
            .await
            .expect("model request deadline")
            .expect("model closed")
    }
    async fn complete(&self, index: usize, status: &str) {
        let mut operation = self.saved.0.lock().unwrap().adds[index].clone();
        operation.status = status.into();
        self.updates.submit(operation).await.unwrap();
    }
    async fn abort(self) {
        self.cancellation.cancel();
        let result = self.task.await.unwrap();
        assert!(result.unwrap_err().to_string().contains("context canceled"));
    }
    fn count(&self, kind: &str) -> usize {
        self.saved
            .0
            .lock()
            .unwrap()
            .items
            .iter()
            .filter(|item| match item {
                ItemData::Input(_) => kind == "input",
                ItemData::Turn(_) => kind == "turn",
                ItemData::ModelResponse(_) => kind == "response",
                ItemData::ToolCallStatus(_) => kind == "status",
                _ => false,
            })
            .count()
    }
}
fn external(id: &str, text: &str, delivery: &str) -> Input {
    Input {
        id: id.into(),
        kind: "external".into(),
        payload: Json::parse(&serde_json::to_string(text).unwrap()).unwrap(),
        delivery: delivery.into(),
    }
}
fn control(mode: &str) -> Input {
    Input {
        id: uuid::Uuid::new_v4().to_string(),
        kind: "control".into(),
        payload: Json::parse(&format!(r#"{{"Mode":"{mode}","Reason":"requested stop"}}"#)).unwrap(),
        delivery: String::new(),
    }
}
fn text(text: &str) -> Response {
    Response {
        output: Some(vec![LlmItem {
            provider_id: String::new(),
            model: String::new(),
            data: LlmData::Message(Message {
                role: "assistant".into(),
                text: text.into(),
                phase: String::new(),
                ..Message::default()
            }),
        }]),
        ..Response::default()
    }
}
fn calls(names: &[(&str, &str)]) -> Response {
    Response {
        output: Some(
            names
                .iter()
                .map(|(id, name)| LlmItem {
                    provider_id: String::new(),
                    model: String::new(),
                    data: LlmData::ToolCall(ToolCall {
                        call_id: (*id).into(),
                        name: (*name).into(),
                        arguments: "1".into(),
                    }),
                })
                .collect(),
        ),
        ..Response::default()
    }
}
fn reply(request: Request, response: Response) {
    assert!(request.reply.send(Ok(response)).is_ok());
}
fn result(request: &SharedRequest, id: &str) -> Option<String> {
    request.input.iter().find_map(|item| match &item.data {
        LlmData::ToolResult(result) if result.call_id == id => Some(
            result
                .output
                .iter()
                .map(|part| part.value.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        _ => None,
    })
}
fn has_text(request: &SharedRequest, needle: &str) -> bool {
    request.input.iter().any(
        |item| matches!(&item.data, LlmData::Message(message) if message.text.contains(needle)),
    )
}
async fn settle() {
    for _ in 0..5 {
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(2)).await;
    }
}

#[tokio::test(start_paused = true)]
async fn idle_session_stays_available_and_stop_does_not_make_a_turn() {
    let mut run = Rig::new(Config::default());
    settle().await;
    assert!(run.requests.try_recv().is_err());
    for index in 0..2 {
        run.send(&format!("i{index}"), "hello").await;
        let request = run.request().await;
        assert!(has_text(&request.request, "hello"));
        reply(request, text("done"));
        settle().await;
        assert!(!run.task.is_finished());
    }
    run.control("when_idle").await;
    run.task.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn steering_drops_the_old_future_and_its_response() {
    let mut run = Rig::new(Config::default());
    run.send("one", "first").await;
    let first = run.request().await;
    run.send("two", "change course").await;
    let second = run.request().await;
    assert!(first.cancellation.is_cancelled());
    assert!(first.reply.send(Ok(calls(&[("stale", "Bash")]))).is_err());
    assert!(has_text(&second.request, "change course"));
    reply(second, text("done"));
    settle().await;
    assert_eq!(run.count("response"), 1);
    assert!(run.saved.0.lock().unwrap().adds.is_empty());
    run.abort().await;
}

#[tokio::test(start_paused = true)]
async fn durable_status_precedes_dispatch_and_completion_is_delivered_once() {
    let mut run = Rig::new(Config::default());
    run.send("one", "work").await;
    reply(run.request().await, calls(&[("call", "Bash")]));
    settle().await;
    assert_eq!(run.saved.0.lock().unwrap().adds.len(), 1);
    assert_eq!(run.count("status"), 1);
    run.complete(0, "completed").await;
    let next = run.request().await;
    assert_eq!(result(&next.request, "call").as_deref(), Some("completed"));
    run.control("when_idle").await;
    reply(next, text("done"));
    run.task.await.unwrap().unwrap();
    let saved = run.saved.0.lock().unwrap();
    assert_eq!(saved.adds.len(), 1);
    assert_eq!(saved.updates.len(), 1);
    assert_eq!(
        saved.trace.iter().filter(|kind| **kind == "model").count(),
        2
    );
}
