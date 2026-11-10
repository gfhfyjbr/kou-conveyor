use kou_provider::{
    Request,
    responses::{
        Adapter, CancellationToken, Config, Exchange, Reporter, RequestOptions, RespondError,
        Retry, StreamEvent, prompt_cache_key,
    },
};
use reqwest::{
    Client,
    header::{HeaderName, HeaderValue},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    sync::Notify,
    task::{JoinHandle, JoinSet},
    time::{Instant, timeout},
};

#[derive(Clone)]
struct Reply {
    status: u16,
    body: Vec<u8>,
    headers: Vec<(String, String)>,
    content_type: &'static str,
    chunk: usize,
    before_headers: Duration,
    between_chunks: Duration,
    hold: bool,
    disconnect: bool,
    length: Option<usize>,
}
impl Reply {
    fn http(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            body: body.into(),
            headers: vec![],
            content_type: if (200..300).contains(&status) {
                "text/event-stream"
            } else {
                "application/json"
            },
            chunk: usize::MAX,
            before_headers: Duration::ZERO,
            between_chunks: Duration::ZERO,
            hold: false,
            disconnect: false,
            length: None,
        }
    }
    fn success() -> Self {
        Self::http(200, terminal("completed", json!({"id":"ok","output":[]})))
    }
    fn stream(kind: &str, code: &str, message: &str) -> Self {
        let error = json!({"code":code,"message":message});
        let event = if kind == "error" {
            json!({"type":kind,"error":error})
        } else {
            json!({"type":kind,"response":{"id":"failed","status":"failed","output":[],"error":error}})
        };
        Self::http(200, format!("data: {event}\n\n"))
    }
}
fn terminal(status: &str, mut body: Value) -> String {
    body["status"] = status.into();
    format!(
        "data: {}\n\n",
        json!({"type":format!("response.{status}"),"response":body})
    )
}
fn delta(text: &str) -> String {
    format!(
        "data: {}\n\n",
        json!({"type":"response.output_text.delta","output_index":0,"delta":text})
    )
}
#[derive(Clone)]
struct Captured {
    body: Vec<u8>,
    headers: BTreeMap<String, Vec<String>>,
}
struct Server {
    endpoint: String,
    requests: Arc<Mutex<Vec<Captured>>>,
    changed: Arc<Notify>,
    closed: Arc<AtomicUsize>,
    closed_changed: Arc<Notify>,
    task: JoinHandle<()>,
}
impl Server {
    async fn new(replies: Vec<Reply>) -> Self {
        assert!(!replies.is_empty());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/responses", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let changed = Arc::new(Notify::new());
        let closed = Arc::new(AtomicUsize::new(0));
        let closed_changed = Arc::new(Notify::new());
        let task = {
            let (requests, changed, closed, closed_changed) = (
                requests.clone(),
                changed.clone(),
                closed.clone(),
                closed_changed.clone(),
            );
            tokio::spawn(async move {
                let mut children = JoinSet::new();
                let mut index = 0;
                loop {
                    tokio::select! {
                        accepted = listener.accept() => {
                            let Ok((stream, _)) = accepted else { break; };
                            let reply = replies[index.min(replies.len()-1)].clone();
                            index += 1;
                            let (requests, changed, closed, closed_changed) = (requests.clone(), changed.clone(), closed.clone(), closed_changed.clone());
                            children.spawn(async move {
                                let mut stream = BufReader::new(stream);
                                let mut line = String::new();
                                if stream.read_line(&mut line).await.is_err() { return; }
                                assert!(line.starts_with("POST /responses HTTP/1.1"), "{line}");
                                let mut headers: BTreeMap<String, Vec<String>> = BTreeMap::new();
                                loop {
                                    line.clear();
                                    if stream.read_line(&mut line).await.unwrap_or(0) == 0 { return; }
                                    if line == "\r\n" { break; }
                                    let (name, value) = line.split_once(':').unwrap();
                                    headers.entry(name.to_ascii_lowercase()).or_default().push(value.trim().to_owned());
                                }
                                let length: usize = headers.get("content-length").unwrap()[0].parse().unwrap();
                                assert!(length <= 1 << 20);
                                let mut body = vec![0; length];
                                if stream.read_exact(&mut body).await.is_err() { return; }
                                requests.lock().unwrap().push(Captured { body, headers });
                                changed.notify_one();
                                struct Closed { count: Arc<AtomicUsize>, changed: Arc<Notify> }
                                impl Drop for Closed { fn drop(&mut self) { self.count.fetch_add(1,Ordering::SeqCst);self.changed.notify_one(); } }
                                let _closed=Closed { count: closed, changed: closed_changed };
                                if reply.disconnect { return; }
                                tokio::time::sleep(reply.before_headers).await;
                                let mut stream = stream.into_inner();
                                let mut headers = format!("HTTP/1.1 {} Test\r\nConnection: close\r\n", reply.status);
                                if !reply.content_type.is_empty() { headers.push_str(&format!("Content-Type: {}\r\n", reply.content_type)); }
                                for (key, value) in reply.headers { headers.push_str(&format!("{key}: {value}\r\n")); }
                                if reply.hold { headers.push_str("Transfer-Encoding: chunked\r\n"); }
                                else { headers.push_str(&format!("Content-Length: {}\r\n", reply.length.unwrap_or(reply.body.len()))); }
                                headers.push_str("\r\n");
                                if stream.write_all(headers.as_bytes()).await.is_err() { return; }
                                for bytes in reply.body.chunks(reply.chunk.max(1)) {
                                    let bytes = if reply.hold { format!("{:x}\r\n{}\r\n", bytes.len(), std::str::from_utf8(bytes).unwrap()).into_bytes() }
                                        else { bytes.to_vec() };
                                    if stream.write_all(&bytes).await.is_err() { return; }
                                    tokio::time::sleep(reply.between_chunks).await;
                                }
                                if reply.hold {
                                    let _ = stream.read(&mut [0;1]).await;
                                }
                            });
                        }
                        result = children.join_next(), if !children.is_empty() => {
                            if let Some(Err(error)) = result { assert!(error.is_cancelled(), "{error}"); }
                        }
                    }
                }
            })
        };
        Self {
            endpoint,
            requests,
            changed,
            closed,
            closed_changed,
            task,
        }
    }
    fn config(&self) -> Config {
        let mut config = Config::new(&self.endpoint);
        config.retry.initial_backoff = Duration::from_millis(1);
        config.retry.max_backoff = Duration::from_millis(4);
        config
    }
    fn adapter(&self, config: Config) -> Adapter {
        let client = Client::builder()
            .no_proxy()
            .retry(reqwest::retry::never())
            .build()
            .unwrap();
        Adapter::with_client(config, client).unwrap()
    }
    async fn wait_requests(&self, count: usize) {
        timeout(Duration::from_secs(3), async {
            while self.requests.lock().unwrap().len() < count {
                self.changed.notified().await;
            }
        })
        .await
        .unwrap();
    }
    async fn wait_closed(&self) {
        timeout(Duration::from_secs(3), async {
            while self.closed.load(Ordering::SeqCst) == 0 {
                self.closed_changed.notified().await;
            }
        })
        .await
        .unwrap();
    }
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
#[derive(Default)]
struct Report {
    events: Vec<StreamEvent>,
    warnings: Vec<String>,
    retries: Vec<(usize, usize, Duration, String)>,
    traces: Vec<(Vec<u8>, u16, Vec<u8>)>,
}
impl Reporter for Report {
    fn stream(&mut self, event: StreamEvent) {
        self.events.push(event);
    }
    fn warning(&mut self, message: &str) {
        self.warnings.push(message.into());
    }
    fn retry(&mut self, retry: Retry<'_>) {
        self.retries.push((
            retry.attempt,
            retry.max_attempts,
            retry.delay,
            retry.error.to_string(),
        ));
    }
    fn trace(&mut self, trace: Exchange<'_>) {
        self.traces.push((
            trace.request_body.into(),
            trace.status_code,
            trace.response_body.into(),
        ));
    }
}
fn request() -> Request {
    serde_json::from_value(json!({"Model":{"ID":"model"},"Input":[{"Type":"message","Data":{"Role":"user","Text":"hello"}}]})).unwrap()
}
async fn respond(
    adapter: &Adapter,
    report: &mut impl Reporter,
) -> Result<kou_domain::Response, RespondError> {
    timeout(
        Duration::from_secs(4),
        adapter.respond(
            &request(),
            RequestOptions::default(),
            &CancellationToken::new(),
            report,
        ),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn actual_http_request_headers_cache_stream_and_borrowed_trace() {
    let mut reply = Reply::success();
    reply.chunk = 1;
    let server = Server::new(vec![reply]).await;
    let mut config = server.config();
    config.prompt_cache_key_field = true;
    config.cache_key_header = Some(HeaderName::from_static("x-cache-key"));
    config
        .headers
        .append("x-custom", HeaderValue::from_static("one"));
    config
        .headers
        .append("x-custom", HeaderValue::from_static("two"));
    config
        .headers
        .append("accept", HeaderValue::from_static("wrong"));
    let adapter = server.adapter(config);
    let mut report = Report::default();
    let response = adapter
        .respond(
            &request(),
            RequestOptions {
                cache_key: "private-session",
                ..RequestOptions::default()
            },
            &CancellationToken::new(),
            &mut report,
        )
        .await
        .unwrap();
    assert_eq!(response.id, "ok");
    let capture = server.requests.lock().unwrap()[0].clone();
    assert_eq!(capture.headers["accept"], ["text/event-stream"]);
    assert_eq!(capture.headers["x-custom"], ["one", "two"]);
    assert_eq!(
        capture.headers["x-cache-key"],
        [prompt_cache_key("private-session")]
    );
    let body: Value = serde_json::from_slice(&capture.body).unwrap();
    assert_eq!(
        body["prompt_cache_key"],
        prompt_cache_key("private-session")
    );
    assert_eq!(report.traces.len(), 1);
    assert_eq!(report.traces[0].0, capture.body);
    assert_eq!(report.traces[0].1, 200);
    assert_eq!(
        serde_json::from_slice::<Value>(&report.traces[0].2).unwrap()["id"],
        "ok"
    );
}
#[tokio::test]
async fn mixed_http_inband_failed_terminal_and_eof_share_one_retry_budget() {
    let server = Server::new(vec![
        Reply::http(503, ""),
        Reply::stream("response.failed", "server_error", "retry"),
        Reply::stream("error", "rate_limit_exceeded", "try again in 1ms"),
        Reply::http(200, delta("discarded")),
        Reply::success(),
    ])
    .await;
    let adapter = server.adapter(server.config());
    let mut report = Report::default();
    assert_eq!(respond(&adapter, &mut report).await.unwrap().id, "ok");
    assert_eq!(server.count(), 5);
    let bodies: Vec<_> = server
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|request| request.body.clone())
        .collect();
    assert!(bodies.windows(2).all(|bodies| bodies[0] == bodies[1]));
    assert_eq!(report.retries.len(), 4);
    assert_eq!(
        report
            .retries
            .iter()
            .map(|retry| (retry.0, retry.1))
            .collect::<Vec<_>>(),
        [(1, 5), (2, 5), (3, 5), (4, 5)]
    );
    assert_eq!(report.events.iter().filter(|event| event.reset).count(), 4);
    assert_eq!(
        report
            .events
            .iter()
            .filter(|event| !event.reset)
            .map(|event| event.text.as_str())
            .collect::<Vec<_>>(),
        ["discarded"]
    );
    assert_eq!(report.traces.len(), 1);
}
#[tokio::test]
async fn exhausted_http_stream_and_terminal_failures_keep_distinct_error_contracts() {
    for kind in ["http", "error", "response.failed"] {
        let reply = if kind == "http" {
            Reply::http(503, r#"{"error":{"code":"server_error","message":"bad"}}"#)
        } else {
            Reply::stream(kind, "server_error", "bad")
        };
        let server = Server::new(vec![reply]).await;
        let mut config = server.config();
        config.retry.max_attempts = 2;
        let adapter = server.adapter(config);
        let result = respond(&adapter, &mut ()).await;
        assert_eq!(server.count(), 2);
        match kind {
            "response.failed" => assert_eq!(result.unwrap().failure.unwrap().code, "server_error"),
            "http" => assert_eq!(
                result.unwrap_err().to_string(),
                "create response: responses API error server_error: bad"
            ),
            _ => assert_eq!(
                result.unwrap_err().to_string(),
                "responses API error server_error: bad"
            ),
        }
    }
}
#[tokio::test]
async fn usage_limits_expose_uncapped_final_wait_but_billing_quota_does_not() {
    for (code, rate_limited) in [
        ("rate_limit_exceeded", true),
        ("usage_limit_reached", true),
        ("insufficient_quota", false),
        ("credit_balance_exhausted", false),
    ] {
        let mut reply = Reply::http(
            429,
            json!({"error":{"code":code,"message":"try again in 45s","resets_in_seconds":90}})
                .to_string(),
        );
        reply.headers.push(("Retry-After".into(), "120".into()));
        let server = Server::new(vec![reply]).await;
        let mut config = server.config();
        config.retry.max_attempts = 2;
        let adapter = server.adapter(config);
        let mut report = Report::default();
        let error = respond(&adapter, &mut report).await.unwrap_err();
        if rate_limited {
            assert!(
                matches!(error, RespondError::RateLimited { retry_after, .. } if retry_after == Duration::from_secs(120))
            );
        } else {
            assert!(matches!(error, RespondError::Api { .. }));
        }
        assert_eq!(
            server.count(),
            if code == "rate_limit_exceeded" { 2 } else { 1 }
        );
        assert!(
            report
                .retries
                .iter()
                .all(|retry| retry.2 == Duration::from_millis(4))
        );
    }
    let server = Server::new(vec![Reply::stream(
        "response.failed",
        "usage_limit_reached",
        "try again in 45s",
    )])
    .await;
    assert_eq!(
        respond(&server.adapter(server.config()), &mut ())
            .await
            .unwrap()
            .failure
            .unwrap()
            .code,
        "usage_limit_reached"
    );
    assert_eq!(server.count(), 1);
}
#[tokio::test]
async fn context_overflow_and_final_errors_do_not_retry() {
    for terminal_failure in [false, true] {
        let message = "This model's maximum context length is 128000 tokens. However, your messages resulted in 130225 tokens.";
        let reply = if terminal_failure {
            Reply::stream("response.failed", "context_length_exceeded", message)
        } else {
            Reply::http(
                400,
                json!({"error":{"code":"context_length_exceeded","message":message}}).to_string(),
            )
        };
        let server = Server::new(vec![reply]).await;
        let error = respond(&server.adapter(server.config()), &mut ())
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            RespondError::ContextOverflow {
                tokens: 130225,
                limit: 128000,
                ..
            }
        ));
        assert_eq!(server.count(), 1);
    }
    for (status, code, kind) in [
        (400, "missing_required_parameter", ""),
        (429, "bio_policy", ""),
        (503, "", "authentication_error"),
        (200, "invalid_api_key", ""),
    ] {
        let reply = if status == 200 {
            Reply::stream("error", code, "bad key")
        } else {
            Reply::http(
                status,
                json!({"error":{"code":code,"type":kind,"message":"final"}}).to_string(),
            )
        };
        let server = Server::new(vec![reply]).await;
        assert!(
            respond(&server.adapter(server.config()), &mut ())
                .await
                .is_err()
        );
        assert_eq!(server.count(), 1);
    }
}
#[tokio::test]
async fn disconnected_and_truncated_streams_retry_but_protocol_and_limits_do_not() {
    for disconnect in [true, false] {
        let mut reply = Reply::http(200, "data: {\"type\":\"response.created\"");
        reply.disconnect = disconnect;
        reply.length = Some(4096);
        let server = Server::new(vec![reply, Reply::success()]).await;
        assert_eq!(
            respond(&server.adapter(server.config()), &mut ())
                .await
                .unwrap()
                .id,
            "ok"
        );
        assert_eq!(server.count(), 2);
    }
    for body in [
        "data: invalid-json\n\n".into(),
        format!("data: {}\n\n", "x".repeat(512)),
    ] {
        let server = Server::new(vec![Reply::http(200, body)]).await;
        let mut config = server.config();
        config.limits.stream.frame_bytes = 256;
        assert!(matches!(
            respond(&server.adapter(config), &mut ()).await,
            Err(RespondError::Protocol(_))
        ));
        assert_eq!(server.count(), 1);
    }
    let server = Server::new(vec![Reply::http(503, vec![b'x'; (1 << 20) + 1])]).await;
    assert_eq!(
        respond(&server.adapter(server.config()), &mut ())
            .await
            .unwrap_err()
            .to_string(),
        "responses API error response exceeds 1 MiB"
    );
    assert_eq!(server.count(), 1);
    let server = Server::new(vec![Reply::success()]).await;
    let mut config = server.config();
    config.limits.request_bytes = 16;
    assert!(
        respond(&server.adapter(config), &mut ())
            .await
            .unwrap_err()
            .to_string()
            .contains("request-byte limit")
    );
    assert_eq!(server.count(), 0);
}
#[tokio::test]
async fn terminal_frame_returns_without_waiting_for_server_eof_and_closes_body() {
    let mut reply = Reply::success();
    reply.hold = true;
    let server = Server::new(vec![reply]).await;
    assert_eq!(
        respond(&server.adapter(server.config()), &mut ())
            .await
            .unwrap()
            .id,
        "ok"
    );
    server.wait_closed().await;
}
#[tokio::test]
async fn cancellation_during_headers_body_backoff_and_admission_releases_request() {
    for stage in ["headers", "body", "backoff"] {
        let mut reply = if stage == "backoff" {
            Reply::http(503, "retry")
        } else {
            Reply::http(200, delta("partial"))
        };
        if stage == "headers" {
            reply.before_headers = Duration::from_secs(10);
        }
        if stage == "body" {
            reply.hold = true;
        }
        let server = Server::new(vec![reply]).await;
        let mut config = server.config();
        config.retry.initial_backoff = Duration::from_secs(30);
        config.retry.max_backoff = Duration::from_secs(30);
        let adapter = server.adapter(config);
        let token = CancellationToken::new();
        struct Signals {
            body: Arc<Notify>,
            retry: Arc<Notify>,
        }
        impl Reporter for Signals {
            fn stream(&mut self, _: StreamEvent) {
                self.body.notify_one();
            }
            fn retry(&mut self, _: Retry<'_>) {
                self.retry.notify_one();
            }
        }
        let body = Arc::new(Notify::new());
        let retry = Arc::new(Notify::new());
        let task = {
            let (adapter, token) = (adapter.clone(), token.clone());
            let mut report = Signals {
                body: body.clone(),
                retry: retry.clone(),
            };
            tokio::spawn(async move {
                adapter
                    .respond(&request(), RequestOptions::default(), &token, &mut report)
                    .await
            })
        };
        server.wait_requests(1).await;
        if stage == "body" {
            timeout(Duration::from_secs(1), body.notified())
                .await
                .unwrap();
        }
        if stage == "backoff" {
            timeout(Duration::from_secs(1), retry.notified())
                .await
                .unwrap();
        }
        token.cancel();
        assert!(
            matches!(
                timeout(Duration::from_secs(1), task)
                    .await
                    .unwrap()
                    .unwrap(),
                Err(RespondError::Cancelled)
            ),
            "{stage}"
        );
        assert_eq!(server.count(), 1);
    }
    let mut reply = Reply::http(200, delta("partial"));
    reply.hold = true;
    let server = Server::new(vec![reply, Reply::success()]).await;
    let mut config = server.config();
    config.limits.concurrent_requests = 1;
    let adapter = server.adapter(config);
    let first_token = CancellationToken::new();
    let first = {
        let (adapter, token) = (adapter.clone(), first_token.clone());
        tokio::spawn(async move {
            adapter
                .respond(&request(), RequestOptions::default(), &token, &mut ())
                .await
        })
    };
    server.wait_requests(1).await;
    let token = CancellationToken::new();
    token.cancel();
    assert!(matches!(
        adapter
            .respond(&request(), RequestOptions::default(), &token, &mut ())
            .await,
        Err(RespondError::Cancelled)
    ));
    let error = adapter
        .respond(
            &request(),
            RequestOptions {
                deadline: Some(Instant::now() + Duration::from_millis(40)),
                ..RequestOptions::default()
            },
            &CancellationToken::new(),
            &mut (),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, RespondError::DeadlineExceeded));
    assert_eq!(server.count(), 1);
    first_token.cancel();
    assert!(matches!(first.await.unwrap(), Err(RespondError::Cancelled)));
    server.wait_closed().await;
    assert_eq!(respond(&adapter, &mut ()).await.unwrap().id, "ok");
}
#[tokio::test]
async fn idle_and_absolute_deadlines_cover_headers_and_body() {
    for headers in [true, false] {
        for deadline in [true, false] {
            let mut reply = Reply::http(200, delta("partial"));
            reply.hold = true;
            if headers {
                reply.before_headers = Duration::from_secs(10);
            }
            let server = Server::new(vec![reply]).await;
            let mut config = server.config();
            config.retry.max_attempts = 1;
            config.response_idle_timeout = if deadline {
                Duration::from_secs(10)
            } else {
                Duration::from_millis(50)
            };
            let adapter = server.adapter(config);
            let options = RequestOptions {
                deadline: deadline.then(|| Instant::now() + Duration::from_millis(50)),
                ..RequestOptions::default()
            };
            let error = adapter
                .respond(&request(), options, &CancellationToken::new(), &mut ())
                .await
                .unwrap_err();
            assert!(
                if deadline {
                    matches!(error, RespondError::DeadlineExceeded)
                } else {
                    matches!(error, RespondError::IdleTimeout)
                },
                "{error}"
            );
            assert_eq!(server.count(), 1);
        }
    }
}
#[tokio::test]
async fn slow_reporter_does_not_consume_network_idle_budget() {
    struct Slow;
    impl Reporter for Slow {
        fn stream(&mut self, _: StreamEvent) {
            std::thread::sleep(Duration::from_millis(150));
        }
    }
    let first = delta("text");
    let mut reply = Reply::http(
        200,
        format!(
            "{first}{}",
            terminal("completed", json!({"id":"ok","output":[]}))
        ),
    );
    reply.chunk = first.len();
    reply.between_chunks = Duration::from_millis(10);
    let server = Server::new(vec![reply]).await;
    let mut config = server.config();
    config.retry.max_attempts = 1;
    config.response_idle_timeout = Duration::from_millis(75);
    assert_eq!(
        respond(&server.adapter(config), &mut Slow)
            .await
            .unwrap()
            .id,
        "ok"
    );
}
#[tokio::test]
async fn dropping_inflight_future_closes_network_and_releases_admission() {
    let mut reply = Reply::http(200, delta("partial"));
    reply.hold = true;
    let server = Server::new(vec![reply, Reply::success()]).await;
    let mut config = server.config();
    config.limits.concurrent_requests = 1;
    let adapter = server.adapter(config);
    let task = {
        let adapter = adapter.clone();
        tokio::spawn(async move { respond(&adapter, &mut ()).await })
    };
    server.wait_requests(1).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    server.wait_closed().await;
    assert_eq!(respond(&adapter, &mut ()).await.unwrap().id, "ok");
}
#[tokio::test]
async fn rejected_images_retry_once_without_history_clone_and_remain_model_scoped() {
    let rejected = Reply::http(400, r#"{"error":{"message":"Images are not supported"}}"#);
    let server = Server::new(vec![rejected, Reply::success()]).await;
    let adapter = server.adapter(server.config());
    let request:Request=serde_json::from_value(json!({"Model":{"ID":"visionless"},"Input":[
        {"Type":"message","Data":{"Role":"user","Text":"look","Images":[{"Label":"  cat ","URL":"data:private"},{"Label":" ","URL":"other"}]}},
        {"Type":"tool_result","Data":{"CallID":"call","Output":[{"Kind":"image","Value":"data:private"},{"Kind":"text","Value":"after"}]}},
        {"Type":"tool_result","Data":{"CallID":"call","Output":[{"Kind":"image","Value":"data:late"},{"Kind":"text","Value":"late"}]}}
    ]})).unwrap();
    let before = serde_json::to_vec(&request).unwrap();
    let mut report = Report::default();
    assert_eq!(
        adapter
            .respond(
                &request,
                RequestOptions::default(),
                &CancellationToken::new(),
                &mut report
            )
            .await
            .unwrap()
            .id,
        "ok"
    );
    assert_eq!(report.warnings.len(), 1);
    assert_eq!(report.events.iter().filter(|event| event.reset).count(), 1);
    assert_eq!(serde_json::to_vec(&request).unwrap(), before);
    let bodies: Vec<Value> = server
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|request| serde_json::from_slice(&request.body).unwrap())
        .collect();
    assert!(bodies[0].to_string().contains("data:private"));
    assert!(!bodies[1].to_string().contains("data:"));
    assert_eq!(
        bodies[1]["input"][0]["content"],
        "look\n\n[The images of this message were left out: this model cannot read images. They were cat.]"
    );
    assert_eq!(
        bodies[1]["input"][1]["output"][0]["text"],
        "[An image was left out here: this model cannot read images.]"
    );
    assert!(
        bodies[1]["input"][2]["content"]
            .as_str()
            .unwrap()
            .contains("\n[An image was left out here: this model cannot read images.]\nlate")
    );
    adapter
        .clone()
        .respond(
            &request,
            RequestOptions::default(),
            &CancellationToken::new(),
            &mut report,
        )
        .await
        .unwrap();
    assert_eq!(server.count(), 3);
    assert_eq!(report.warnings.len(), 1);
    {
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests[1].body, requests[2].body);
    }
    let mut other = request.clone();
    other.model.id = "other".into();
    adapter
        .respond(
            &other,
            RequestOptions::default(),
            &CancellationToken::new(),
            &mut (),
        )
        .await
        .unwrap();
    assert!(
        String::from_utf8_lossy(&server.requests.lock().unwrap()[3].body).contains("data:private")
    );
}
#[tokio::test]
async fn gzip_is_decoded_before_frame_accounting_and_content_type_is_case_insensitive() {
    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, bytes).unwrap();
        encoder.finish().unwrap()
    }
    for content_type in ["", "Text/Event-Stream; charset=utf-8"] {
        let mut reply = Reply::success();
        reply.body = gzip(&reply.body);
        reply.content_type = content_type;
        reply
            .headers
            .push(("Content-Encoding".into(), "gzip".into()));
        let server = Server::new(vec![reply]).await;
        assert_eq!(
            respond(&server.adapter(server.config()), &mut ())
                .await
                .unwrap()
                .id,
            "ok"
        );
        assert_eq!(
            server.requests.lock().unwrap()[0].headers["accept-encoding"],
            ["gzip"]
        );
    }
    let mut reply = Reply::http(200, gzip(delta(&"x".repeat(16 << 10)).as_bytes()));
    reply
        .headers
        .push(("Content-Encoding".into(), "gzip".into()));
    let server = Server::new(vec![reply]).await;
    let mut config = server.config();
    config.limits.stream.frame_bytes = 512;
    assert!(matches!(
        respond(&server.adapter(config), &mut ()).await,
        Err(RespondError::Protocol(_))
    ));
    assert_eq!(server.count(), 1);
}
#[tokio::test]
async fn cancellation_in_reporter_suppresses_later_events_from_the_same_network_chunk() {
    struct CancelReport {
        token: CancellationToken,
        events: usize,
    }
    impl Reporter for CancelReport {
        fn stream(&mut self, _: StreamEvent) {
            self.events += 1;
            self.token.cancel();
        }
    }
    let mut reply = Reply::http(200, format!("{}{}", delta("first"), delta("do not report")));
    reply.hold = true;
    let server = Server::new(vec![reply]).await;
    let adapter = server.adapter(server.config());
    let token = CancellationToken::new();
    let mut report = CancelReport {
        token: token.clone(),
        events: 0,
    };
    assert!(matches!(
        adapter
            .respond(&request(), RequestOptions::default(), &token, &mut report)
            .await,
        Err(RespondError::Cancelled)
    ));
    assert_eq!(report.events, 1);
    server.wait_closed().await;
}
#[tokio::test]
async fn openai_fireworks_openrouter_and_ollama_use_their_real_rust_clients() {
    use kou_provider::clients::{Client as ProviderClient, Config, Service};
    for service in [
        Service::OpenAi,
        Service::Fireworks,
        Service::OpenRouter,
        Service::Ollama,
    ] {
        let body = terminal(
            "completed",
            json!({"id":"ok","output":[],"usage":{
                "input_tokens":8,"output_tokens":4,"prompt_tokens":11,"completion_tokens":7,
                "prompt_tokens_details":{"cached_tokens":3},"completion_tokens_details":{"reasoning_tokens":5}
            }}),
        );
        let server = Server::new(vec![Reply::http(200, body)]).await;
        let mut config = Config::new(service);
        config.api_key = "fixture-key".into();
        config.base_url = server.endpoint.strip_suffix("/responses").unwrap().into();
        let http = Client::builder()
            .no_proxy()
            .retry(reqwest::retry::never())
            .build()
            .unwrap();
        let client = ProviderClient::with_client(config, http).unwrap();
        let mut report = Report::default();
        let response = client
            .respond(
                &request(),
                RequestOptions {
                    cache_key: "session-1",
                    ..RequestOptions::default()
                },
                &CancellationToken::new(),
                &mut report,
            )
            .await
            .unwrap();
        let captured = server.requests.lock().unwrap()[0].clone();
        let body: Value = serde_json::from_slice(&captured.body).unwrap();
        assert_eq!(captured.headers["content-type"], ["application/json"]);
        if service == Service::Ollama {
            assert!(!captured.headers.contains_key("authorization"));
            assert!(report.traces.is_empty());
        } else {
            assert_eq!(captured.headers["authorization"], ["Bearer fixture-key"]);
            assert_eq!(report.traces.len(), 1);
        }
        let cache = prompt_cache_key("session-1");
        match service {
            Service::OpenAi => assert_eq!(body["prompt_cache_key"], cache),
            Service::Fireworks => {
                assert_eq!(captured.headers["x-session-affinity"], [cache]);
                assert_eq!(
                    (
                        response.usage.input_tokens,
                        response.usage.output_tokens,
                        response.usage.cached_input_tokens,
                        response.usage.reasoning_tokens
                    ),
                    (11, 7, 3, 5)
                );
            }
            Service::OpenRouter => {
                assert_eq!(captured.headers["x-session-id"], [cache]);
                assert_eq!(
                    body["cache_control"],
                    json!({"type":"ephemeral","ttl":"1h"})
                );
            }
            Service::Ollama => assert!(body.get("prompt_cache_key").is_none()),
        }
        if service != Service::OpenAi {
            assert!(body.get("prompt_cache_key").is_none());
        }
    }
}
#[tokio::test]
async fn codex_subscription_headers_limits_cancellation_and_renewal_errors() {
    use kou_provider::codex::{Client, Config};
    let server = Server::new(vec![
        Reply::success(),
        Reply::http(
            401,
            r#"{"error":{"code":"invalid_api_key","message":"expired"}}"#,
        ),
    ])
    .await;
    let client = Client::new(Config {
        access_token: "opaque-subscription-token".into(),
        account_id: "account-1".into(),
        base_url: server.endpoint.strip_suffix("/responses").unwrap().into(),
        max_attempts: Some(1),
        ..Config::default()
    })
    .unwrap();
    let mut source = request();
    let mut report = Report::default();
    assert_eq!(
        client
            .respond(
                &source,
                RequestOptions {
                    cache_key: "codex-session",
                    ..RequestOptions::default()
                },
                &CancellationToken::new(),
                &mut report
            )
            .await
            .unwrap()
            .id,
        "ok"
    );
    let captured = server.requests.lock().unwrap()[0].clone();
    assert_eq!(
        captured.headers["authorization"],
        ["Bearer opaque-subscription-token"]
    );
    assert_eq!(captured.headers["chatgpt-account-id"], ["account-1"]);
    assert_eq!(captured.headers["originator"], ["kou-conveyor"]);
    assert_eq!(captured.headers["user-agent"], ["kou-conveyor"]);
    assert_eq!(
        captured.headers["session-id"],
        [prompt_cache_key("codex-session")]
    );
    assert!(!captured.headers.contains_key("session_id"));
    let body: Value = serde_json::from_slice(&captured.body).unwrap();
    assert_eq!(body["prompt_cache_key"], prompt_cache_key("codex-session"));
    assert_eq!(body["store"], false);
    source.model.max_output_tokens = Some(0);
    assert_eq!(
        client
            .respond(
                &source,
                RequestOptions::default(),
                &CancellationToken::new(),
                &mut ()
            )
            .await
            .unwrap_err()
            .to_string(),
        "codex does not support max_output_tokens"
    );
    let token = CancellationToken::new();
    token.cancel();
    assert!(matches!(
        client
            .respond(&source, RequestOptions::default(), &token, &mut ())
            .await,
        Err(RespondError::Cancelled)
    ));
    assert!(matches!(
        client
            .respond(
                &source,
                RequestOptions {
                    deadline: Some(Instant::now()),
                    ..RequestOptions::default()
                },
                &CancellationToken::new(),
                &mut ()
            )
            .await,
        Err(RespondError::DeadlineExceeded)
    ));
    assert_eq!(server.count(), 1);
    source.model.max_output_tokens = None;
    let error = client
        .respond(
            &source,
            RequestOptions::default(),
            &CancellationToken::new(),
            &mut (),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, RespondError::CredentialsRejected { .. }));
    assert!(
        error.to_string().starts_with(
            "codex credentials rejected; renew them externally and recreate the client:"
        )
    );
    assert_eq!(error.api_error().unwrap().status_code, 401);
    assert_eq!(server.count(), 2);
}
#[tokio::test]
async fn codex_refuses_to_forward_credentials_through_redirects() {
    use kou_provider::codex::{Client, Config};
    for status in [307, 308] {
        let target = Server::new(vec![Reply::success()]).await;
        let mut redirect = Reply::http(status, "");
        redirect
            .headers
            .push(("Location".into(), target.endpoint.clone()));
        let source = Server::new(vec![redirect]).await;
        let client = Client::new(Config {
            access_token: "opaque-token".into(),
            account_id: "account".into(),
            base_url: source.endpoint.strip_suffix("/responses").unwrap().into(),
            max_attempts: Some(1),
            ..Config::default()
        })
        .unwrap();
        let error = client
            .respond(
                &request(),
                RequestOptions::default(),
                &CancellationToken::new(),
                &mut (),
            )
            .await
            .unwrap_err();
        assert_eq!(error.api_error().unwrap().status_code, status);
        assert_eq!(source.count(), 1);
        assert_eq!(target.count(), 0);
    }
}
#[tokio::test]
async fn codex_auth_file_is_a_constructor_snapshot_not_a_background_refresh_service() {
    use kou_provider::codex::{Client, Config};
    let dir = tempfile::tempdir().unwrap();
    let auth = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
    std::fs::write(
        auth.path(),
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"token-1","account_id":"account-1"}}"#,
    )
    .unwrap();
    let server = Server::new(vec![Reply::success()]).await;
    let client = Client::new(Config {
        auth_file: Some(auth.path().into()),
        base_url: server.endpoint.strip_suffix("/responses").unwrap().into(),
        ..Config::default()
    })
    .unwrap();
    for _ in 0..2 {
        assert_eq!(
            client
                .respond(
                    &request(),
                    RequestOptions::default(),
                    &CancellationToken::new(),
                    &mut ()
                )
                .await
                .unwrap()
                .id,
            "ok"
        );
        std::fs::write(auth.path(), r#"{"auth_mode":"chatgpt","tokens":{"access_token":"token-2","account_id":"account-2"}}"#).unwrap();
    }
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(
        |request| request.headers["authorization"] == ["Bearer token-1"]
            && request.headers["chatgpt-account-id"] == ["account-1"]
    ));
    assert!(
        std::fs::read_to_string(auth.path())
            .unwrap()
            .contains("token-2")
    );
}
#[test]
fn invalid_configuration_is_rejected_without_network() {
    assert_eq!(
        Config::new("http://example.invalid").response_idle_timeout,
        Duration::from_secs(1800)
    );
    assert_eq!(Config::new("http://example.invalid").retry.max_attempts, 5);
    for endpoint in ["", "  ", "ftp://localhost", "not a url"] {
        assert!(Adapter::new(Config::new(endpoint)).is_err());
    }
    let mut config = Config::new("http://example.invalid");
    config.retry.max_attempts = 0;
    assert!(Adapter::new(config).is_err());
    let mut config = Config::new("http://example.invalid");
    config.response_idle_timeout = Duration::ZERO;
    assert!(Adapter::new(config).is_err());
}
