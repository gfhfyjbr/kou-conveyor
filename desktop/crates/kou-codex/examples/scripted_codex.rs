//! The app-server peer for the transport tests. The same executable can
//! serve as a keeper so the tests need no installed desktop application.
use serde_json::{Value, json};
use std::io::{BufRead as _, Write as _};

fn say(message: Value) {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{message}").unwrap();
    out.flush().unwrap();
}

fn event(method: &str, params: Value) {
    say(json!({"method":method,"params":params}));
}
fn reply(id: &Value, result: Value) {
    say(json!({"id":id,"result":result}));
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|s| s == "host") {
        std::process::exit(kou_claude::host::run(&args[1..]));
    }
    assert_eq!(args, ["app-server"]);
    let mut history = Vec::new();
    let mut pending = Vec::new();
    let mut greeted = false;
    let model = |name: &str| {
        json!({"model":name,"displayName":name,"isDefault":name=="one",
        "supportedReasoningEfforts":[{"reasoningEffort":"high","description":"Think longer"}],"defaultReasoningEffort":"high"})
    };
    for line in std::io::stdin().lock().lines() {
        let m: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let id = &m["id"];
        let p = &m["params"];
        match m["method"].as_str() {
            Some("initialize") => {
                assert!(!greeted);
                reply(id, json!({}));
            }
            Some("initialized") => greeted = true,
            _ if !greeted => panic!("request before initialize"),
            Some("model/list") if p["cursor"].is_null() => {
                reply(id, json!({"data":[model("one")],"nextCursor":"2"}))
            }
            Some("model/list") => reply(id, json!({"data":[model("two")],"nextCursor":null})),
            Some("config/read") => reply(id, json!({"config":{"model":"two"}})),
            Some("thread/list") => reply(
                id,
                json!({"data":[{"id":"s","name":"Fixture","cwd":std::env::current_dir().unwrap(),"updatedAt":100}],"nextCursor":null}),
            ),
            Some("collaborationMode/list") => reply(
                id,
                json!({"data":[{"name":"Default","mode":"default"},{"name":"Plan","mode":"plan"}]}),
            ),
            Some("thread/start" | "thread/resume") => {
                let mut turns = history.clone();
                if !pending.is_empty() {
                    turns.push(json!({"id":"t","status":"inProgress","items":pending}));
                }
                reply(
                    id,
                    json!({"model":p["model"].as_str().unwrap_or("one"),"thread":{"id":"s","name":"Fixture","turns":turns}}),
                );
            }
            Some("turn/start") => {
                assert_eq!(p["threadId"], "s");
                if p["input"][0]["text"] == "reject" {
                    say(json!({"id":id,"error":{"code":-32602,"message":"rejected test input"}}));
                    continue;
                }
                let turn = json!({"id":"t","status":"inProgress","items":[]});
                event("turn/started", json!({"threadId":"s","turn":turn}));
                reply(id, json!({"turn":turn}));
                pending = vec![json!({"id":"user","type":"userMessage","content":p["input"]})];
                event(
                    "item/completed",
                    json!({"threadId":"s","turnId":"t","item":pending[0]}),
                );
                event(
                    "item/agentMessage/delta",
                    json!({"threadId":"s","turnId":"t","itemId":"progress","delta":"Checking the folder"}),
                );
                let cmd = json!({"id":"cmd","type":"commandExecution","command":"pwd","status":"inProgress"});
                event(
                    "item/started",
                    json!({"threadId":"s","turnId":"t","item":cmd}),
                );
                pending.push(cmd);
                say(
                    json!({"id":"same-7","method":"item/commandExecution/requestApproval","params":{
                    "threadId":"s","turnId":"t","itemId":"cmd","command":"pwd","availableDecisions":["accept","decline"]}}),
                );
            }
            None if id == "same-7" => {
                assert_eq!(m["result"], json!({"decision":"accept"}));
                pending[1]["status"] = json!("completed");
                pending[1]["aggregatedOutput"] = json!("/work");
                pending[1]["exitCode"] = json!(0);
                event(
                    "item/completed",
                    json!({"threadId":"s","turnId":"t","item":pending[1]}),
                );
                say(
                    json!({"id":27,"method":"item/tool/requestUserInput","params":{"threadId":"s","turnId":"t","itemId":"ask",
                    "questions":[{"id":"package","question":"Which package?","options":null,"isOther":true}]}}),
                );
            }
            None if id == 27 => {
                assert_eq!(
                    m["result"],
                    json!({"answers":{"package":{"answers":["kou"]}}})
                );
                event(
                    "item/agentMessage/delta",
                    json!({"threadId":"unrelated","turnId":"t","itemId":"wrong","delta":"wrong"}),
                );
                event(
                    "item/agentMessage/delta",
                    json!({"threadId":"s","turnId":"t","itemId":"msg","delta":"hello"}),
                );
                let item = json!({"id":"msg","type":"agentMessage","text":"hello!"});
                event(
                    "item/completed",
                    json!({"threadId":"s","turnId":"t","item":item}),
                );
                pending.push(item);
            }
            Some("turn/steer") => {
                assert_eq!(p["expectedTurnId"], "t");
                let item = json!({"id":"steer","type":"userMessage","content":p["input"]});
                event(
                    "item/completed",
                    json!({"threadId":"s","turnId":"t","item":item}),
                );
                pending.push(item);
                reply(id, json!({"turnId":"t"}));
            }
            Some("turn/interrupt") => {
                assert_eq!(p, &json!({"threadId":"s","turnId":"t"}));
                reply(id, json!({}));
                let turn = json!({"id":"t","status":"interrupted","items":pending});
                history.push(turn.clone());
                pending.clear();
                event("turn/completed", json!({"threadId":"s","turn":turn}));
            }
            Some(other) => say(json!({"id":id,"error":{"code":-32601,"message":other}})),
            None => panic!("unexpected response: {m}"),
        }
    }
}
