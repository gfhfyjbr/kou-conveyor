use super::*;
use kou_provider::{Model as ProviderModel, responses};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn actual_http_provider_is_driven_and_its_response_is_journaled() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(5), async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut received = Vec::new();
            let header_end = loop {
                let mut chunk = [0u8; 4096];
                let size = socket.read(&mut chunk).await.unwrap();
                assert!(size > 0);
                received.extend_from_slice(&chunk[..size]);
                assert!(received.len() <= 64 << 10);
                if let Some(at) = received.windows(4).position(|part| part == b"\r\n\r\n") { break at + 4; }
            };
            let header = std::str::from_utf8(&received[..header_end]).unwrap().to_lowercase();
            assert!(header.starts_with("post /responses http/1.1"));
            assert!(header.contains("authorization: bearer owned-test"));
            let size: usize = header.lines().find_map(|line| line.strip_prefix("content-length:")).unwrap().trim().parse().unwrap();
            assert!(size < 64 << 10);
            while received.len() < header_end + size {
                let mut chunk = [0u8; 4096];
                let count = socket.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                received.extend_from_slice(&chunk[..count]);
            }
            let request: serde_json::Value = serde_json::from_slice(&received[header_end..header_end + size]).unwrap();
            let payload = r#"{"id":"owned","status":"completed","output":[{"type":"message","id":"m","role":"assistant","content":[{"type":"output_text","text":"owned answer"}]}],"usage":{"input_tokens":42,"output_tokens":3}}"#;
            let body = format!("event: response.completed\ndata: {{\"type\":\"response.completed\",\"response\":{payload}}}\n\n");
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            request
        }).await.expect("owned HTTP server deadline")
    });
    let mut config = responses::Config::new(format!("http://{address}/responses"));
    config
        .headers
        .insert("authorization", "Bearer owned-test".parse().unwrap());
    let adapter = responses::Adapter::new(config).unwrap();
    let mut run = Rig::custom(Config::default(), |mut driver| {
        driver.model = Arc::new(adapter);
        driver.history.context.set_model(ProviderModel {
            id: "gpt-owned".into(),
            ..ProviderModel::default()
        });
        driver
    });
    run.send("one", "owned prompt").await;
    run.control("when_idle").await;
    let outcome = tokio::time::timeout(Duration::from_secs(5), &mut run.task).await;
    let request = server.await.unwrap();
    if outcome.is_err() {
        run.cancellation.cancel();
        let _ = run.task.await;
        panic!("coordinator HTTP deadline");
    }
    outcome.unwrap().unwrap().unwrap();
    assert_eq!(request["model"], "gpt-owned");
    assert_eq!(request["stream"], true);
    let saved = run.saved.0.lock().unwrap();
    let response = saved
        .items
        .iter()
        .find_map(|item| match item {
            ItemData::ModelResponse(response) => Some(response),
            _ => None,
        })
        .unwrap();
    assert_eq!(response.response.usage.input_tokens, 42);
    assert!(
        matches!(&response.response.output.as_ref().unwrap()[0].data, LlmData::Message(message) if message.text == "owned answer")
    );
}
