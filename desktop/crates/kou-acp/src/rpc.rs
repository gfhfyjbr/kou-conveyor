//! The JSON-RPC 2.0 envelope: one message per line in each direction.
//!
//! The payloads are typed by the protocol's schema crate; the envelope is kept
//! as loose JSON here so that a message the schema does not know (an extension
//! method, a newer notification) is still routed instead of failing the line.

use serde::Serialize;
use serde_json::{Value, json};

/// The request was cancelled by its sender.
pub(crate) const REQUEST_CANCELLED: i64 = -32800;
/// The method is not implemented by the receiver.
pub(crate) const METHOD_NOT_FOUND: i64 = -32601;
/// The parameters are malformed or name something the receiver cannot do.
pub(crate) const INVALID_PARAMS: i64 = -32602;
/// The receiver failed.
pub(crate) const INTERNAL_ERROR: i64 = -32603;
/// The agent wants the user logged in first.
pub(crate) const AUTH_REQUIRED: i64 = -32000;
/// A session, file or terminal that does not exist.
pub(crate) const RESOURCE_NOT_FOUND: i64 = -32002;

/// The error member of a failed response.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// The message with whatever detail the sender attached, for the user.
    pub fn describe(&self) -> String {
        let detail = match &self.data {
            Some(Value::String(text)) => Some(text.clone()),
            Some(Value::Object(map)) => ["details", "message", "error"]
                .iter()
                .find_map(|key| map.get(*key).and_then(Value::as_str))
                .map(str::to_owned),
            _ => None,
        };
        match detail {
            Some(detail) if !detail.is_empty() && detail != self.message => {
                format!("{}: {detail}", self.message)
            }
            _ => self.message.clone(),
        }
    }
}

/// One message from the agent.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Incoming {
    /// The agent calls a client method and waits for the answer.
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    /// The agent reports something.
    Notification { method: String, params: Value },
    /// The agent answers one of our requests.
    Response {
        id: Value,
        result: Result<Value, RpcError>,
    },
}

/// Read one line of the agent's output. A batch yields several messages.
pub(crate) fn parse(line: &str) -> Result<Vec<Incoming>, String> {
    let value: Value = serde_json::from_str(line).map_err(|error| error.to_string())?;
    match value {
        Value::Array(items) if !items.is_empty() => items.into_iter().map(classify).collect(),
        Value::Object(_) => Ok(vec![classify(value)?]),
        _ => Err("neither a JSON-RPC message nor a batch of them".into()),
    }
}

fn classify(value: Value) -> Result<Incoming, String> {
    let Value::Object(mut map) = value else {
        return Err("a JSON-RPC message must be an object".into());
    };
    let id = map.remove("id").filter(|id| !id.is_null());
    if let Some(method) = map.remove("method") {
        let Value::String(method) = method else {
            return Err("the method is not a string".into());
        };
        let params = map.remove("params").unwrap_or(Value::Null);
        return Ok(match id {
            Some(id) => Incoming::Request { id, method, params },
            None => Incoming::Notification { method, params },
        });
    }
    let Some(id) = id else {
        return Err("neither a method nor a response id".into());
    };
    if let Some(error) = map.remove("error") {
        let error = RpcError {
            code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("the agent reported an error without a message")
                .to_owned(),
            data: error.get("data").filter(|data| !data.is_null()).cloned(),
        };
        return Ok(Incoming::Response {
            id,
            result: Err(error),
        });
    }
    match map.remove("result") {
        Some(result) => Ok(Incoming::Response {
            id,
            result: Ok(result),
        }),
        None => Err("a response with neither a result nor an error".into()),
    }
}

/// A request line.
pub(crate) fn request(id: u64, method: &str, params: &impl Serialize) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
}

/// A notification line.
pub(crate) fn notification(method: &str, params: &impl Serialize) -> String {
    json!({ "jsonrpc": "2.0", "method": method, "params": params }).to_string()
}

/// The line answering the agent's request `id`.
pub(crate) fn response(id: &Value, result: Result<Value, RpcError>) -> String {
    match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => {
            let mut body = json!({ "code": error.code, "message": error.message });
            if let Some(data) = error.data {
                body["data"] = data;
            }
            json!({ "jsonrpc": "2.0", "id": id, "error": body })
        }
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_a_request_a_notification_or_a_response() {
        assert_eq!(
            parse(r#"{"jsonrpc":"2.0","id":"a","method":"fs/read_text_file","params":{"x":1}}"#),
            Ok(vec![Incoming::Request {
                id: json!("a"),
                method: "fs/read_text_file".into(),
                params: json!({"x": 1}),
            }])
        );
        assert_eq!(
            parse(r#"{"jsonrpc":"2.0","method":"session/update"}"#),
            Ok(vec![Incoming::Notification {
                method: "session/update".into(),
                params: Value::Null,
            }])
        );
        // A null id is how JSON-RPC 1.0 spelled a notification.
        assert!(matches!(
            parse(r#"{"id":null,"method":"x"}"#).unwrap()[0],
            Incoming::Notification { .. }
        ));
        assert_eq!(
            parse(r#"{"jsonrpc":"2.0","id":4,"result":null}"#),
            Ok(vec![Incoming::Response {
                id: json!(4),
                result: Ok(Value::Null),
            }])
        );
        assert_eq!(
            parse(r#"{"id":5,"error":{"code":-32000,"message":"Authentication required"}}"#),
            Ok(vec![Incoming::Response {
                id: json!(5),
                result: Err(RpcError::new(AUTH_REQUIRED, "Authentication required")),
            }])
        );
    }

    #[test]
    fn a_batch_yields_its_messages_and_garbage_is_an_error() {
        let batch = parse(r#"[{"method":"a"},{"id":1,"result":{}}]"#).unwrap();
        assert_eq!(batch.len(), 2);
        for line in [
            "",
            "not json",
            "[]",
            "42",
            r#"{"id":1}"#,
            r#"{"jsonrpc":"2.0"}"#,
        ] {
            assert!(parse(line).is_err(), "{line:?}");
        }
        assert!(parse(r#"{"method":7}"#).is_err());
    }

    #[test]
    fn every_outgoing_message_is_one_line() {
        let lines = [
            request(1, "session/prompt", &json!({"text": "a\nb"})),
            notification("session/cancel", &json!({"sessionId": "s"})),
            response(&json!(7), Ok(json!({}))),
            response(
                &json!("x"),
                Err(RpcError {
                    code: RESOURCE_NOT_FOUND,
                    message: "no such file".into(),
                    data: Some(json!({"path": "/a"})),
                }),
            ),
        ];
        for line in &lines {
            assert!(!line.contains('\n'), "{line}");
        }
        assert_eq!(
            lines[3],
            r#"{"jsonrpc":"2.0","id":"x","error":{"code":-32002,"message":"no such file","data":{"path":"/a"}}}"#
        );
    }

    #[test]
    fn an_error_is_described_with_its_detail() {
        let mut error = RpcError::new(INTERNAL_ERROR, "Internal error");
        assert_eq!(error.describe(), "Internal error");
        error.data = Some(json!("rate limited"));
        assert_eq!(error.describe(), "Internal error: rate limited");
        error.data = Some(json!({"details": "quota"}));
        assert_eq!(error.describe(), "Internal error: quota");
    }
}
