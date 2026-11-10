use kou_domain::{Json, LlmData};
use kou_provider::{
    Request,
    anthropic::{
        AttemptOptions, CancellationToken, Client, Config, Limits, Reporter, RequestConfig,
        RequestOptions, RespondError, StreamDecoder, StreamLimits, decode_response, prepare,
    },
    responses::StreamEvent,
};
use serde_json::json;
use std::{collections::BTreeMap, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    time::Instant,
};

fn request(text: &str) -> Request {
    serde_json::from_value(json!({"Input":[{"Type":"message","Data":{"Role":"user","Text":text}}]}))
        .unwrap()
}
fn event(kind: &str, value: serde_json::Value) -> String {
    format!("event: {kind}\ndata: {value}\n\n")
}
fn start() -> String {
    event(
        "message_start",
        json!({"type":"message_start","message":{"id":"msg","content":[],"usage":{"input_tokens":3,"output_tokens":0}}}),
    )
}
fn delta(text: &str) -> String {
    event(
        "content_block_delta",
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}),
    )
}
fn text_start() -> String {
    event(
        "content_block_start",
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
    )
}
fn stop() -> String {
    event("message_stop", json!({"type":"message_stop"}))
}
fn stream() -> String {
    start() + &text_start() + &delta("line\n中") + &stop()
}
fn client(base_url: String, idle: Duration, limits: Limits) -> Client {
    Client::with_client(
        Config {
            api_key: "synthetic-key".into(),
            base_url,
            max_attempts: Some(1),
            response_idle_timeout: idle,
            limits,
            ..Default::default()
        },
        reqwest::Client::builder()
            .no_proxy()
            .retry(reqwest::retry::never())
            .build()
            .unwrap(),
    )
    .unwrap()
}
async fn accept(listener: &TcpListener) -> (TcpStream, String) {
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut bytes = Vec::new();
    let end = loop {
        let mut chunk = [0; 8192];
        let n = socket.read(&mut chunk).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let header = std::str::from_utf8(&bytes[..end]).unwrap();
    let length = header
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .map_or(0, |(_, value)| value.trim().parse::<usize>().unwrap());
    while bytes.len() < end + length {
        let mut chunk = [0; 8192];
        let n = socket.read(&mut chunk).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&chunk[..n]);
    }
    (socket, String::from_utf8(bytes).unwrap())
}
async fn reply(socket: &mut TcpStream, status: u16, body: &str) {
    let content_type = if body.starts_with('{') {
        "application/json"
    } else {
        "text/event-stream"
    };
    socket.write_all(format!("HTTP/1.1 {status} fixture\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
    socket.shutdown().await.unwrap();
}
struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}
impl Server {
    async fn finish(mut self) {
        tokio::time::timeout(Duration::from_secs(2), &mut self.0)
            .await
            .unwrap()
            .unwrap();
    }
}
async fn listen() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    (listener, base)
}

#[test]
fn borrowed_request_limits_and_schema_depth() {
    let mut input = request(&"x".repeat(4 << 20));
    input.tools.push(serde_json::from_value(json!({"Type":"function","Name":"f","Parameters":{"type":"array","properties":{"z":{"type":"number"},"a":{"type":"string"}}}})).unwrap());
    let before = serde_json::to_vec(&input).unwrap();
    let config = RequestConfig::default();
    let wire = prepare(&input, &config, AttemptOptions::default()).unwrap();
    assert!(
        wire.encode(1 << 20)
            .unwrap_err()
            .0
            .contains("request-byte limit")
    );
    let encoded = wire.encode(8 << 20).unwrap();
    assert!(encoded.len() > 4 << 20);
    let prefix = wire.prefix().unwrap();
    assert_eq!(prefix.messages.len(), 1);
    assert_eq!(prefix.messages[0].len(), 8);
    assert_eq!(serde_json::to_vec(&input).unwrap(), before);
    let deep = format!("{}0{}", "[".repeat(180), "]".repeat(180));
    input.tools[0]
        .parameters
        .as_mut()
        .unwrap()
        .insert("deep".into(), Json::parse(&deep).unwrap());
    let wire = prepare(&input, &config, AttemptOptions::default()).unwrap();
    assert!(
        wire.encode(8 << 20)
            .unwrap_err()
            .0
            .contains("schema exceeds the depth limit")
    );
}
#[test]
fn cache_markers_do_not_change_prefix_fingerprints() {
    let input:Request=serde_json::from_value(json!({"Input":[{"Type":"message","Data":{"Role":"system","Text":"prompt"}},{"Type":"message","Data":{"Role":"user","Text":"hello"}},{"Type":"reasoning","Data":{"Raw":{"type":"thinking","thinking":"thought","signature":"sig"}}}]})).unwrap();
    let short = RequestConfig::default();
    let long = RequestConfig {
        cache_ttl: "1h".into(),
        ..short.clone()
    };
    let a = prepare(&input, &short, AttemptOptions::default())
        .unwrap()
        .prefix()
        .unwrap();
    let b = prepare(&input, &long, AttemptOptions::default())
        .unwrap()
        .prefix()
        .unwrap();
    assert_eq!(
        (a.tools, a.system, a.messages),
        (b.tools, b.system, b.messages)
    );
    assert_eq!(a.breakpoints.len(), 2);
    assert!(a.breakpoints.iter().all(|point| point.ttl == "5m"));
    assert!(b.breakpoints.iter().all(|point| point.ttl == "1h"));
}
#[test]
fn sse_all_chunk_boundaries_and_retention_limits() {
    let raw = stream();
    let mut baseline = StreamDecoder::new(StreamLimits::default()).unwrap();
    let mut events = Vec::new();
    baseline
        .feed(raw.as_bytes(), |event| events.push(event))
        .unwrap();
    let expected = baseline.finish().unwrap();
    for cut in 0..=raw.len() {
        let mut decoder = StreamDecoder::new(StreamLimits::default()).unwrap();
        let mut got = Vec::new();
        decoder
            .feed(&raw.as_bytes()[..cut], |event| got.push(event))
            .unwrap();
        decoder
            .feed(&raw.as_bytes()[cut..], |event| got.push(event))
            .unwrap();
        assert_eq!(decoder.finish().unwrap(), expected, "cut {cut}");
        assert_eq!(got, events);
    }
    for limits in [
        StreamLimits {
            frame_bytes: 8,
            ..Default::default()
        },
        StreamLimits {
            retained_bytes: 2,
            ..Default::default()
        },
    ] {
        let mut decoder = StreamDecoder::new(limits).unwrap();
        assert!(decoder.feed(raw.as_bytes(), |_| {}).is_err());
        assert!(decoder.feed(b"x", |_| {}).is_err());
        assert!(decoder.finish().is_err());
    }
    let mut decoder = StreamDecoder::new(StreamLimits {
        content_blocks: 1,
        ..Default::default()
    })
    .unwrap();
    decoder
        .feed((start() + &text_start()).as_bytes(), |_| {})
        .unwrap();
    assert!(
        decoder
            .feed(
                event(
                    "content_block_start",
                    json!({"type":"content_block_start","index":1,"content_block":{"type":"text"}})
                )
                .as_bytes(),
                |_| {}
            )
            .unwrap_err()
            .0
            .contains("content-block limit")
    );
}
#[test]
fn response_usage_keeps_opaque_numbers_and_reconstructs_thinking() {
    let response=decode_response(br#"{"id":"msg","content":[{"type":"thinking","thinking":"think","signature":"sig","ignored":true}],"usage":{"input_tokens":2,"vendor":1e999}}"#,&BTreeMap::new()).unwrap();
    assert!(response.usage.raw.as_str().unwrap().contains("1e999"));
    let LlmData::Reasoning(thinking) = &response.output.unwrap()[0].data else {
        panic!()
    };
    assert_eq!(
        thinking.raw.as_str(),
        Some(r#"{"type":"thinking","thinking":"think","signature":"sig"}"#)
    );
}
#[test]
fn repaired_sdk_inputs_and_exact_tool_inputs_are_separate() {
    for fragments in [["{}", r#"{"a":1,"a":2}"#], ["{half", r#"{"a":1}"#]] {
        let mut raw = start()
            + &event(
                "content_block_start",
                json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"c","name":"f","input":{}}}),
            );
        for fragment in fragments {
            raw += &event(
                "content_block_delta",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":fragment}}),
            );
        }
        raw += &stop();
        let mut decoder = StreamDecoder::new(StreamLimits::default()).unwrap();
        decoder.feed(raw.as_bytes(), |_| {}).unwrap();
        let response = decoder.finish().unwrap();
        let LlmData::ToolCall(call) = &response.output.unwrap()[0].data else {
            panic!()
        };
        assert_eq!(call.arguments, fragments.concat());
        assert_eq!(response.usage.raw.is_missing(), fragments[0] == "{}");
    }
}
#[tokio::test]
async fn cancellation_after_callback_suppresses_later_callbacks() {
    struct Cancel {
        token: CancellationToken,
        count: usize,
    }
    impl Reporter for Cancel {
        fn stream(&mut self, _event: StreamEvent) {
            self.count += 1;
            self.token.cancel();
        }
    }
    let (listener, base) = listen().await;
    let server = Server(tokio::spawn(async move {
        let (mut socket, _) = accept(&listener).await;
        let raw = start() + &text_start() + &delta("one") + &delta("two") + &stop();
        reply(&mut socket, 200, &raw).await;
    }));
    let client = client(base, Duration::from_secs(1), Limits::default());
    let token = CancellationToken::new();
    let mut report = Cancel {
        token: token.clone(),
        count: 0,
    };
    assert!(matches!(
        client
            .respond(
                &request("hello"),
                RequestOptions::default(),
                &token,
                &mut report
            )
            .await,
        Err(RespondError::Cancelled)
    ));
    assert_eq!(report.count, 1);
    server.finish().await;
}
#[tokio::test]
async fn idle_timeout_and_absolute_deadline_are_distinct() {
    for deadline in [false, true] {
        let (listener, base) = listen().await;
        let (started, arrived) = oneshot::channel();
        let server = Server(tokio::spawn(async move {
            let (mut socket, _) = accept(&listener).await;
            started.send(()).unwrap();
            let mut byte = [0];
            assert_eq!(socket.read(&mut byte).await.unwrap(), 0);
        }));
        let client = client(
            base,
            if deadline {
                Duration::from_secs(2)
            } else {
                Duration::from_millis(30)
            },
            Limits::default(),
        );
        let task = tokio::spawn(async move {
            client
                .respond(
                    &request("hello"),
                    RequestOptions {
                        deadline: deadline.then(|| Instant::now() + Duration::from_millis(100)),
                        ..Default::default()
                    },
                    &CancellationToken::new(),
                    &mut (),
                )
                .await
        });
        arrived.await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert!(if deadline {
            matches!(result, Err(RespondError::DeadlineExceeded))
        } else {
            matches!(result, Err(RespondError::IdleTimeout))
        });
        server.finish().await;
    }
}
#[tokio::test]
async fn abort_closes_connection_and_releases_admission() {
    let (listener, base) = listen().await;
    let (started, arrived) = oneshot::channel();
    let (closed, closure) = oneshot::channel();
    let server = Server(tokio::spawn(async move {
        let (mut socket, _) = accept(&listener).await;
        started.send(()).unwrap();
        let mut byte = [0];
        assert_eq!(socket.read(&mut byte).await.unwrap(), 0);
        closed.send(()).unwrap();
        let (mut socket, _) = accept(&listener).await;
        reply(&mut socket, 200, &stream()).await;
    }));
    let client = client(
        base,
        Duration::from_secs(2),
        Limits {
            concurrent_requests: 1,
            ..Default::default()
        },
    );
    let first = client.clone();
    let task = tokio::spawn(async move {
        first
            .respond(
                &request("one"),
                RequestOptions::default(),
                &CancellationToken::new(),
                &mut (),
            )
            .await
    });
    arrived.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(2), closure)
        .await
        .unwrap()
        .unwrap();
    let response = tokio::time::timeout(
        Duration::from_secs(2),
        client.respond(
            &request("two"),
            RequestOptions::default(),
            &CancellationToken::new(),
            &mut (),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.id, "msg");
    server.finish().await;
}
#[tokio::test]
async fn pings_count_as_received_bytes_not_total_request_time() {
    let (listener, base) = listen().await;
    let server = Server(tokio::spawn(async move {
        let (mut socket, _) = accept(&listener).await;
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        for _ in 0..5 {
            socket
                .write_all(b"event: ping\ndata: {\"type\":\"ping\"}\n\n")
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        socket.write_all(stream().as_bytes()).await.unwrap();
        socket.shutdown().await.unwrap();
    }));
    let client = client(base, Duration::from_millis(150), Limits::default());
    let start = Instant::now();
    let response = client
        .respond(
            &request("hello"),
            RequestOptions::default(),
            &CancellationToken::new(),
            &mut (),
        )
        .await
        .unwrap();
    assert_eq!(response.id, "msg");
    assert!(start.elapsed() > Duration::from_millis(150));
    server.finish().await;
}
#[tokio::test]
async fn model_pagination_and_unavailable_endpoints() {
    let (listener, base) = listen().await;
    let server = Server(tokio::spawn(async move {
        for (index, body) in [
            r#"{"data":[{"id":"first"}],"last_id":"cursor +/中"}"#,
            r#"{"data":[{"id":"second"},{"id":"first"}],"has_more":false}"#,
        ]
        .into_iter()
        .enumerate()
        {
            let (mut socket, request) = accept(&listener).await;
            assert!(
                request.starts_with("GET /v1/models?")
                    && request.lines().next().unwrap().contains("limit=1000")
            );
            assert_eq!(
                request.contains("after_id=cursor+%2B%2F%E4%B8%AD"),
                index == 1
            );
            reply(&mut socket, 200, body).await;
        }
    }));
    let client = client(base, Duration::from_secs(2), Limits::default());
    assert_eq!(
        client
            .list_models(RequestOptions::default(), &CancellationToken::new())
            .await
            .unwrap(),
        ["first", "second", "first"]
    );
    server.finish().await;
    for status in [404, 405] {
        let (listener, base) = listen().await;
        let server = Server(tokio::spawn(async move {
            let (mut socket, _) = accept(&listener).await;
            reply(&mut socket, status, "missing").await;
        }));
        let client = client_for_test(base);
        assert!(matches!(
            client
                .list_models(RequestOptions::default(), &CancellationToken::new())
                .await,
            Err(RespondError::ModelsUnavailable)
        ));
        server.finish().await;
    }
}
fn client_for_test(base: String) -> Client {
    client(base, Duration::from_secs(2), Limits::default())
}
#[tokio::test]
async fn model_cycles_stop_at_the_page_limit() {
    let (listener, base) = listen().await;
    let server = Server(tokio::spawn(async move {
        for _ in 0..2 {
            let (mut socket, _) = accept(&listener).await;
            reply(
                &mut socket,
                200,
                r#"{"data":[{"id":"m"}],"last_id":"same","has_more":true}"#,
            )
            .await;
        }
    }));
    let client = client(
        base,
        Duration::from_secs(2),
        Limits {
            model_pages: 2,
            ..Default::default()
        },
    );
    assert!(
        client
            .list_models(RequestOptions::default(), &CancellationToken::new())
            .await
            .unwrap_err()
            .to_string()
            .contains("page-count limit")
    );
    server.finish().await;
}
