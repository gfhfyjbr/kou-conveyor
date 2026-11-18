//! Codex's plan limits, asked of its app server.
//!
//! `codex app-server` speaks JSON-RPC in lines. After `initialize`,
//! `account/rateLimits/read` answers with the account's windows; no thread
//! is started. The shapes are those of the schema codex-cli 0.160.0 writes
//! (`codex app-server generate-json-schema`): here the request itself was
//! answered with the service's refusal of the stored sign-in, so a filled
//! answer was not seen.

use crate::{
    Answer, Ask, Kind, Limits, Reason, Window, claude::first_line, in_order, plan_name, run::Talk,
    share, when::from_epoch,
};
use serde_json::{Value, json};

const HELLO: i64 = 1;
const ASKED: i64 = 2;

pub(crate) fn limits(ask: &Ask) -> Answer {
    let failed = |what: String| Reason::Failed(what);
    let mut talk = Talk::start(ask, &["app-server"])
        .map_err(|error| failed(format!("Codex could not be started: {error}")))?;
    talk.say(&json!({
        "id": HELLO,
        "method": "initialize",
        "params": {"clientInfo": {"name": "kou-desktop", "title": "kou", "version": env!("CARGO_PKG_VERSION")}},
    }))
    .map_err(|error| failed(format!("Codex did not take the question: {error}")))?;
    while let Some(message) = talk.hear() {
        if message["id"] == HELLO {
            if message.get("error").is_some() {
                return Err(failed(refusal(&message["error"])));
            }
            talk.say(&json!({"method": "initialized"}))
                .and_then(|()| talk.say(&json!({"id": ASKED, "method": "account/rateLimits/read"})))
                .map_err(|error| failed(format!("Codex did not take the question: {error}")))?;
        } else if message["id"] == ASKED {
            return read(&message);
        }
    }
    Err(failed(if talk.late() {
        "Codex did not answer in time".to_owned()
    } else {
        "Codex ended without an answer".to_owned()
    }))
}

fn refusal(error: &Value) -> String {
    format!(
        "Codex could not say: {}",
        first_line(error["message"].as_str().unwrap_or("no reason given"))
    )
}

/// The answer to `account/rateLimits/read`: `{result: {rateLimits, ..}}` or
/// `{error: {message}}`.
pub(crate) fn read(message: &Value) -> Answer {
    if let Some(error) = message.get("error") {
        let said = error["message"].as_str().unwrap_or_default();
        // The service turned the stored sign-in away: only the user can
        // sign in again.
        if said.contains("401") || said.to_lowercase().contains("signing in again") {
            return Err(Reason::SignIn(
                "Codex's sign-in was refused by its service: sign in again with `codex login`"
                    .to_owned(),
            ));
        }
        return Err(Reason::Failed(refusal(error)));
    }
    let result = &message["result"];
    let main = &result["rateLimits"];
    if !main.is_object() {
        return Err(Reason::NoPlan);
    }
    let mut windows = snapshot(main, None);
    // Limits beside the main one (a model's own), each under its name.
    let main_id = main["limitId"].as_str();
    if let Some(others) = result["rateLimitsByLimitId"].as_object() {
        for (id, other) in others {
            let name = other["limitName"]
                .as_str()
                .map(str::trim)
                .unwrap_or_default();
            if Some(id.as_str()) == main_id || name.is_empty() {
                continue;
            }
            windows.extend(snapshot(other, Some(name)));
        }
    }
    Ok(Limits {
        plan: main["planType"].as_str().and_then(plan_name),
        windows: in_order(windows),
    })
}

/// The two windows of one limit: by how long each is, the short one and the
/// week. `name` is the limit's, when it is not the account's main one.
fn snapshot(limit: &Value, name: Option<&str>) -> Vec<Window> {
    ["primary", "secondary"]
        .into_iter()
        .filter_map(|which| {
            let window = &limit[which];
            let used = window["usedPercent"].as_f64()?;
            let minutes = window["windowDurationMins"].as_i64();
            let kind = match (minutes, name) {
                // Up to half a day is the short window; from six days, the week.
                (Some(minutes), None) if minutes <= 12 * 60 => Kind::Session,
                (Some(minutes), None) if minutes >= 6 * 24 * 60 => Kind::Weekly,
                (Some(minutes), Some(name)) if minutes >= 6 * 24 * 60 => {
                    Kind::Model(name.to_owned())
                }
                (Some(minutes), Some(name)) => Kind::Other(format!("{name} {}", span(minutes))),
                (Some(minutes), None) => Kind::Other(span(minutes)),
                // Without a length, the first is the short one by convention.
                (None, None) if which == "primary" => Kind::Session,
                (None, None) => Kind::Weekly,
                (None, Some(name)) => Kind::Other(name.to_owned()),
            };
            Some(Window {
                kind,
                used: share(used),
                resets: window["resetsAt"].as_i64().and_then(from_epoch),
            })
        })
        .collect()
}

/// A window's length in words: `5h`, `3d`.
fn span(minutes: i64) -> String {
    if minutes >= 24 * 60 {
        format!("{}d", minutes / (24 * 60))
    } else if minutes >= 60 {
        format!("{}h", minutes / 60)
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_windows_of_the_account_are_the_short_one_and_the_week() {
        let answer = read(&json!({"id": 2, "result": {
            "accountId": "a",
            "rateLimits": {
                "limitId": "codex", "limitName": null, "planType": "plus",
                "primary": {"usedPercent": 31, "windowDurationMins": 300, "resetsAt": 1_794_960_001},
                "secondary": {"usedPercent": 78, "windowDurationMins": 10080, "resetsAt": null},
                "credits": null
            },
            "rateLimitsByLimitId": {
                "codex": {"limitId": "codex", "primary": {"usedPercent": 31, "windowDurationMins": 300}},
                "codex_other": {
                    "limitId": "codex_other", "limitName": "GPT-6.1-Sol",
                    "primary": {"usedPercent": 5, "windowDurationMins": 300, "resetsAt": null},
                    "secondary": {"usedPercent": 44, "windowDurationMins": 10080, "resetsAt": null}
                },
                "unnamed": {"limitId": "unnamed", "primary": {"usedPercent": 99, "windowDurationMins": 300}}
            }
        }}))
        .expect("limits");
        assert_eq!(answer.plan.as_deref(), Some("Plus"));
        let shown: Vec<(String, f32, bool)> = answer
            .windows
            .iter()
            .map(|window| (window.kind.label(), window.used, window.resets.is_some()))
            .collect();
        assert_eq!(
            shown,
            [
                ("5h".to_owned(), 31., true),
                ("Weekly".to_owned(), 78., false),
                ("GPT-6.1-Sol weekly".to_owned(), 44., false),
                ("GPT-6.1-Sol 5h".to_owned(), 5., false),
            ]
        );
    }

    #[test]
    fn a_refused_sign_in_says_what_to_do_and_other_failures_what_was_said() {
        // What the service answered here, through codex-cli 0.160.0.
        let refused = read(&json!({"id": 2, "error": {"code": -32603, "message":
            "failed to fetch codex rate limits: GET https://chatgpt.com/backend-api/wham/usage failed: 401 Unauthorized; content-type=text/plain; body={\n  \"error\": {\n    \"message\": \"Could not find the appropriate key in your authentication token. Please try signing in again.\"\n  }\n}"}}));
        assert_eq!(
            refused,
            Err(Reason::SignIn(
                "Codex's sign-in was refused by its service: sign in again with `codex login`"
                    .into()
            ))
        );
        let offline = read(
            &json!({"id": 2, "error": {"message": "failed to fetch codex rate limits: dns error\nmore"}}),
        );
        assert_eq!(
            offline.unwrap_err().message(),
            "Codex could not say: failed to fetch codex rate limits: dns error"
        );
        assert_eq!(
            read(&json!({"id": 2, "result": {"rateLimits": null}})),
            Err(Reason::NoPlan)
        );
        // Windows without a length: the first is the short one.
        let bare = read(&json!({"id": 2, "result": {"rateLimits": {
            "primary": {"usedPercent": 1}, "secondary": {"usedPercent": 2}
        }}}))
        .expect("limits");
        let kinds: Vec<_> = bare
            .windows
            .iter()
            .map(|window| window.kind.clone())
            .collect();
        assert_eq!(kinds, [Kind::Session, Kind::Weekly]);
        assert_eq!(
            (span(300), span(4320), span(45)),
            ("5h".into(), "3d".into(), "45m".into())
        );
    }
}
