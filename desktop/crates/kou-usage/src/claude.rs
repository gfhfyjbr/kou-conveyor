//! Claude Code's plan limits, asked over its control protocol.
//!
//! `claude --print --input-format stream-json --output-format stream-json`
//! takes requests on its input beside prompts. `get_usage` answers with the
//! data of its `/usage` screen (verified on 2.1.323). No prompt is written,
//! so no model is called; the session is kept off disk, runs no hooks and
//! starts no MCP servers.

use crate::{Answer, Ask, Kind, Limits, Reason, Window, in_order, plan_name, run::Talk, share};
use serde_json::{Value, json};

const ARGS: [&str; 10] = [
    "--print",
    "--output-format",
    "stream-json",
    "--verbose",
    "--input-format",
    "stream-json",
    "--no-session-persistence",
    "--strict-mcp-config",
    "--settings",
    r#"{"disableAllHooks":true}"#,
];

const ASKED: &str = "kou-usage";

pub(crate) fn limits(ask: &Ask) -> Answer {
    let failed = |what: String| Reason::Failed(what);
    let mut talk = Talk::start(ask, &ARGS)
        .map_err(|error| failed(format!("Claude Code could not be started: {error}")))?;
    let request = |id: &str, request: Value| json!({"type": "control_request", "request_id": id, "request": request});
    talk.say(&request("kou-hello", json!({"subtype": "initialize"})))
        .and_then(|()| {
            talk.say(&request(
                ASKED,
                json!({"subtype": "get_usage", "skip_behaviors": true}),
            ))
        })
        .map_err(|error| failed(format!("Claude Code did not take the question: {error}")))?;
    while let Some(message) = talk.hear() {
        let response = &message["response"];
        if message["type"] == "control_response" && response["request_id"] == ASKED {
            return read(response);
        }
    }
    Err(failed(if talk.late() {
        "Claude Code did not answer in time".to_owned()
    } else {
        "Claude Code ended without an answer".to_owned()
    }))
}

/// A control response to `get_usage`: `{subtype: "success", response: {..}}`
/// or `{subtype: "error", error}`.
pub(crate) fn read(response: &Value) -> Answer {
    if response["subtype"] != "success" {
        let said = response["error"].as_str().unwrap_or("no reason given");
        // A version from before the request existed says it does not know it.
        return Err(Reason::Failed(
            if said.to_lowercase().contains("unsupported") {
                "This Claude Code does not report its plan's limits: update it".to_owned()
            } else {
                format!("Claude Code could not say: {}", first_line(said))
            },
        ));
    }
    let usage = &response["response"];
    let limits = &usage["rate_limits"];
    if usage["rate_limits_available"] == false || !limits.is_object() {
        return Err(Reason::NoPlan);
    }
    let mut windows = Vec::new();
    // The service's own rows, each with its kind: the short window, the
    // week, the week for a model.
    for row in limits["limits"].as_array().into_iter().flatten() {
        let Some(used) = row["percent"].as_f64() else {
            continue;
        };
        let scope = &row["scope"];
        let named = |part: &str| {
            scope[part]["display_name"]
                .as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty())
        };
        let kind = match row["kind"].as_str() {
            Some("session") => Kind::Session,
            Some("weekly_all") => Kind::Weekly,
            Some("weekly_scoped") => match named("model").or_else(|| named("surface")) {
                Some(name) => Kind::Model(name.to_owned()),
                None => continue,
            },
            Some(other) if !other.is_empty() => Kind::Other(other.replace('_', " ")),
            _ => continue,
        };
        windows.push(window(kind, used, &row["resets_at"]));
    }
    // An answer without rows (an older service): the fields they replaced.
    if windows.is_empty() {
        let field = |name: &str, kind: Kind| {
            let field = &limits[name];
            field["utilization"]
                .as_f64()
                .map(|used| window(kind, used, &field["resets_at"]))
        };
        windows.extend(field("five_hour", Kind::Session));
        windows.extend(field("seven_day", Kind::Weekly));
        windows.extend(field("seven_day_opus", Kind::Model("Opus".into())));
        windows.extend(field("seven_day_sonnet", Kind::Model("Sonnet".into())));
        for scoped in limits["model_scoped"].as_array().into_iter().flatten() {
            if let (Some(name), Some(used)) = (
                scoped["display_name"].as_str(),
                scoped["utilization"].as_f64(),
            ) {
                windows.push(window(
                    Kind::Model(name.to_owned()),
                    used,
                    &scoped["resets_at"],
                ));
            }
        }
    }
    Ok(Limits {
        plan: usage["subscription_type"].as_str().and_then(plan_name),
        windows: in_order(windows),
    })
}

fn window(kind: Kind, used: f64, resets: &Value) -> Window {
    Window {
        kind,
        used: share(used),
        resets: resets.as_str().and_then(crate::parse_time),
    }
}

pub(crate) fn first_line(text: &str) -> String {
    let line = text.trim().lines().next().unwrap_or("").trim();
    let mut short: String = line.chars().take(160).collect();
    if line.chars().count() > 160 {
        short.push('…');
    }
    short
}

#[cfg(test)]
mod tests {
    use super::*;

    fn success(usage: Value) -> Value {
        json!({"subtype": "success", "request_id": ASKED, "response": usage})
    }

    #[test]
    fn the_rows_of_the_usage_screen_become_windows() {
        // As 2.1.323 answered here: no short window on this plan, the week,
        // and the week for one model.
        let answer = read(&success(json!({
            "subscription_type": "max",
            "rate_limits_available": true,
            "rate_limits": {
                "five_hour": null,
                "seven_day": {"utilization": 66, "resets_at": "2026-11-18T00:00:01.000905+00:00"},
                "limits": [
                    {"kind": "weekly_all", "group": "weekly", "percent": 66, "severity": "normal",
                     "resets_at": "2026-11-18T00:00:01.000905+00:00", "scope": null, "is_active": false},
                    {"kind": "weekly_scoped", "group": "weekly", "percent": 84, "severity": "warning",
                     "resets_at": "2026-11-18T00:00:01.001065+00:00",
                     "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null},
                     "is_active": true},
                    {"kind": "session", "group": "session", "percent": 12.5,
                     "resets_at": "2026-11-13T18:00:00+00:00", "scope": null},
                    {"kind": "weekly_scoped", "percent": 3, "scope": {"model": null, "surface": null}},
                    {"kind": "weekly_all", "percent": "a lot"}
                ]
            }
        })))
        .expect("limits");
        assert_eq!(answer.plan.as_deref(), Some("Max"));
        let shown: Vec<(String, f32, bool)> = answer
            .windows
            .iter()
            .map(|window| (window.kind.label(), window.used, window.resets.is_some()))
            .collect();
        assert_eq!(
            shown,
            [
                ("5h".to_owned(), 12.5, true),
                ("Weekly".to_owned(), 66., true),
                ("Fable weekly".to_owned(), 84., true),
            ]
        );
    }

    #[test]
    fn an_answer_without_rows_is_read_from_the_older_fields() {
        let answer = read(&success(json!({
            "subscription_type": "pro",
            "rate_limits_available": true,
            "rate_limits": {
                "five_hour": {"utilization": 40, "resets_at": "2026-11-13T18:00:00Z"},
                "seven_day": {"utilization": 250, "resets_at": null},
                "seven_day_opus": {"utilization": null, "resets_at": null},
                "seven_day_sonnet": {"utilization": 7, "resets_at": "soon"},
                "model_scoped": [{"display_name": "Fable", "utilization": 91, "resets_at": null}]
            }
        })))
        .expect("limits");
        let shown: Vec<(String, f32, bool)> = answer
            .windows
            .iter()
            .map(|window| (window.kind.label(), window.used, window.resets.is_some()))
            .collect();
        assert_eq!(
            shown,
            [
                ("5h".to_owned(), 40., true),
                // Never more than all of it.
                ("Weekly".to_owned(), 100., false),
                ("Fable weekly".to_owned(), 91., false),
                ("Sonnet weekly".to_owned(), 7., false),
            ]
        );
    }

    #[test]
    fn what_is_not_a_plan_s_and_what_fails_is_said() {
        let api_key = read(&success(json!({
            "subscription_type": null, "rate_limits_available": false, "rate_limits": null
        })));
        assert_eq!(api_key, Err(Reason::NoPlan));
        let old = read(&json!({
            "subtype": "error", "request_id": ASKED,
            "error": "Unsupported control request subtype: get_usage"
        }));
        assert_eq!(
            old.unwrap_err().message(),
            "This Claude Code does not report its plan's limits: update it"
        );
        let broken = read(&json!({"subtype": "error", "error": "fetch failed\nstack"}));
        assert_eq!(
            broken.unwrap_err().message(),
            "Claude Code could not say: fetch failed"
        );
        // A plan with nothing to count yet is a plan with no windows.
        let empty = read(&success(json!({
            "subscription_type": "max", "rate_limits_available": true, "rate_limits": {}
        })));
        assert_eq!(empty.map(|limits| limits.windows.len()), Ok(0));
    }
}
