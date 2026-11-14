//! Grok Build's plan limits: `grok usage --json`.
//!
//! The command answers only while Grok's background service runs (it starts
//! with a Grok session); without it, it says so, and that is passed on in
//! its own words. On this machine (grok 1.0.77) that is all it ever said, so
//! the shape of a filled answer was not seen: what it prints is searched for
//! anything that reads as a window (a share used, or an amount of a limit,
//! and when it starts over), and an answer with none is unavailable, not
//! guessed at.

use crate::{
    Answer, Ask, Kind, Limits, Reason, Window, claude::first_line, in_order, plan_name, run, share,
    when::from_epoch,
};
use serde_json::Value;

pub(crate) fn limits(ask: &Ask) -> Answer {
    let printed = run::print(ask, &["usage", "--json"])
        .map_err(|error| Reason::Failed(format!("Grok Build could not be started: {error}")))?;
    read(printed.code, &printed.out, &printed.err)
}

pub(crate) fn read(code: Option<i32>, out: &str, err: &str) -> Answer {
    let Some(code) = code else {
        return Err(Reason::Failed(
            "Grok Build did not answer in time".to_owned(),
        ));
    };
    let said = || {
        let line = first_line(if err.trim().is_empty() { out } else { err });
        line.trim_start_matches("Error:").trim().to_owned()
    };
    if code != 0 {
        let said = said();
        return Err(
            if said.to_lowercase().contains("can't show your plan's usage") {
                Reason::NotHere(said)
            } else if said.to_lowercase().contains("login")
                || said.to_lowercase().contains("sign in")
            {
                Reason::SignIn(said)
            } else if said.is_empty() {
                Reason::Failed(format!("Grok Build's usage command failed ({code})"))
            } else {
                Reason::Failed(said)
            },
        );
    }
    // The JSON, whatever is printed around it.
    let json = out
        .find(['{', '['])
        .zip(out.rfind(['}', ']']))
        .filter(|(from, to)| from < to)
        .and_then(|(from, to)| serde_json::from_str::<Value>(&out[from..=to]).ok());
    let Some(json) = json else {
        return Err(Reason::Failed(
            "Grok Build's answer could not be read".to_owned(),
        ));
    };
    let mut windows = Vec::new();
    gather(&json, "", &mut windows);
    if windows.is_empty() {
        return Err(Reason::Failed(
            "Grok Build's answer names no limit this app can read".to_owned(),
        ));
    }
    let plan = [
        "plan",
        "plan_name",
        "planName",
        "tier",
        "subscription",
        "subscription_type",
    ]
    .into_iter()
    .find_map(|key| json[key].as_str())
    .and_then(plan_name);
    Ok(Limits {
        plan,
        windows: in_order(windows),
    })
}

/// Every object under `value` that reads as a window, named by what it
/// says of itself or by the key it stands under.
fn gather(value: &Value, under: &str, windows: &mut Vec<Window>) {
    match value {
        Value::Array(items) => {
            for item in items {
                gather(item, under, windows);
            }
        }
        Value::Object(fields) => {
            let number = |keys: &[&str]| keys.iter().find_map(|key| fields.get(*key)?.as_f64());
            let used = number(&[
                "percent",
                "used_percent",
                "usedPercent",
                "percent_used",
                "percentUsed",
                "utilization",
            ])
            .or_else(|| {
                let used = number(&["used", "usage", "consumed"])?;
                let limit = number(&["limit", "max", "total", "quota"])?;
                (limit > 0.).then(|| used / limit * 100.)
            });
            if let Some(used) = used {
                let named = ["kind", "window", "period", "name", "label", "type"]
                    .into_iter()
                    .find_map(|key| fields.get(key)?.as_str())
                    .unwrap_or(under);
                let resets = [
                    "resets_at",
                    "resetsAt",
                    "reset_at",
                    "resetAt",
                    "resets",
                    "reset",
                ]
                .into_iter()
                .find_map(|key| match fields.get(key)? {
                    Value::String(text) => crate::parse_time(text),
                    Value::Number(seconds) => seconds.as_i64().and_then(from_epoch),
                    _ => None,
                });
                windows.push(Window {
                    kind: kind_of(named),
                    used: share(used),
                    resets,
                });
                return;
            }
            for (key, inner) in fields {
                gather(inner, key, windows);
            }
        }
        _ => {}
    }
}

fn kind_of(named: &str) -> Kind {
    let name = named.to_lowercase();
    if name.contains("week") || name.contains("7d") || name.contains("seven") {
        Kind::Weekly
    } else if name.contains("hour")
        || name.contains("5h")
        || name.contains("session")
        || name.contains("five")
    {
        Kind::Session
    } else if name.trim().is_empty() {
        Kind::Other("Plan".to_owned())
    } else {
        Kind::Other(named.replace('_', " "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_its_service_grok_says_so_in_its_own_words() {
        // What grok 1.0.77 printed here, with no session open.
        let here = read(
            Some(1),
            "",
            "Error: `grok usage` can't show your plan's usage here yet. Run /usage inside grok instead.\n",
        );
        assert_eq!(
            here,
            Err(Reason::NotHere(
                "`grok usage` can't show your plan's usage here yet. Run /usage inside grok instead."
                    .into()
            ))
        );
        assert!(matches!(
            read(Some(1), "", "Error: not logged in, run grok login"),
            Err(Reason::SignIn(_))
        ));
        assert_eq!(
            read(Some(2), "", "").unwrap_err().message(),
            "Grok Build's usage command failed (2)"
        );
        assert_eq!(
            read(None, "", "").unwrap_err().message(),
            "Grok Build did not answer in time"
        );
        assert_eq!(
            read(Some(0), "no json here", "").unwrap_err().message(),
            "Grok Build's answer could not be read"
        );
        assert_eq!(
            read(Some(0), r#"{"plan":"heavy","note":"nothing counted"}"#, "")
                .unwrap_err()
                .message(),
            "Grok Build's answer names no limit this app can read"
        );
    }

    #[test]
    fn whatever_reads_as_a_window_in_its_answer_is_one() {
        // Not a shape Grok was seen to print: the kinds of thing a window
        // is recognised by.
        let answer = read(
            Some(0),
            r#"note
            {"plan": "super_grok", "limits": {
                "weekly": {"used_percent": 42.5, "resets_at": "2026-11-18T00:00:01Z"},
                "five_hour": {"used": 30, "limit": 120, "reset": 1794960001},
                "rows": [{"name": "grok-5 heavy", "utilization": 9}],
                "broken": {"used": 3, "limit": 0}
            }}"#,
            "",
        )
        .expect("limits");
        assert_eq!(answer.plan.as_deref(), Some("Super grok"));
        let shown: Vec<(String, f32, bool)> = answer
            .windows
            .iter()
            .map(|window| (window.kind.label(), window.used, window.resets.is_some()))
            .collect();
        assert_eq!(
            shown,
            [
                ("5h".to_owned(), 25., true),
                ("Weekly".to_owned(), 42.5, true),
                ("grok-5 heavy".to_owned(), 9., false),
            ]
        );
    }
}
