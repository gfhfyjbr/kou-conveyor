//! How much of an account's plan is used, asked of each harness's own
//! command line.
//!
//! No prompt is sent and no credential is read here: the harness is started
//! without a conversation, asked for the figures behind its own `/usage`
//! screen, and stopped. It answers from the login it has.
//!
//! - Claude Code: a control request (`get_usage`) on the stream it speaks
//!   with its SDK. The request is marked experimental there, so everything
//!   read from the answer is optional and an answer that cannot be read is
//!   an error to show, never a panic.
//! - Codex: `account/rateLimits/read` of its app server.
//! - Grok Build: `grok usage --json`.

use anyhow::{Context as _, anyhow, bail};
use serde_json::{Value, json};
use std::{
    io::{BufRead as _, BufReader, Write as _},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// How near its limit a window is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Normal,
    /// Close: what the harness itself warns about, or four fifths used.
    Warning,
    /// Used up.
    Reached,
}

/// One window of a plan: the five hours, the week, a model's week.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    /// `5h`, `Weekly`, `Fable weekly`.
    pub name: String,
    /// How much of it is used, from 0 to 100.
    pub used: f32,
    /// When it starts over, when the harness says.
    pub resets: Option<SystemTime>,
    pub level: Level,
}

/// What an account's plan allows, as far as its harness tells.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Limits {
    /// The plan's name: `Max`, `Pro`, `plus`.
    pub plan: Option<String>,
    /// Its windows, the shortest first.
    pub windows: Vec<Window>,
    /// Why there are no windows, when the harness answered and has none.
    pub note: Option<String>,
}

/// A harness started to be asked something, read line by line with a time
/// limit, and stopped when this is dropped.
struct Talk {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
}

impl Talk {
    fn start(mut command: Command) -> anyhow::Result<Self> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().context("starting it")?;
        let stdout = child.stdout.take().context("its output")?;
        let (heard, lines) = mpsc::channel();
        std::thread::Builder::new()
            .name("kou-usage".to_owned())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if heard.send(line).is_err() {
                        break;
                    }
                }
            })
            .context("a thread to read it")?;
        Ok(Self {
            stdin: child.stdin.take(),
            child,
            lines,
        })
    }

    fn say(&mut self, message: &Value) -> anyhow::Result<()> {
        let stdin = self.stdin.as_mut().context("its input is closed")?;
        writeln!(stdin, "{message}").context("writing to it")?;
        stdin.flush().context("writing to it")
    }

    /// The first line `pick` takes, heard before `until`.
    fn hear<T>(
        &mut self,
        until: Instant,
        mut pick: impl FnMut(&Value) -> Option<T>,
    ) -> anyhow::Result<T> {
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                bail!("it did not answer in time");
            }
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    if let Ok(message) = serde_json::from_str::<Value>(&line)
                        && let Some(found) = pick(&message)
                    {
                        return Ok(found);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => bail!("it did not answer in time"),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    bail!("it exited without an answer")
                }
            }
        }
    }
}

impl Drop for Talk {
    fn drop(&mut self) {
        // Its input closed, it leaves by itself; if not, it is stopped.
        drop(self.stdin.take());
        let until = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < until {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A harness's command, without what would tell it that it runs inside
/// a pane of this app or inside another agent.
fn command(program: &Path, cwd: &Path) -> Command {
    let mut command = Command::new(program);
    command.current_dir(cwd);
    for (name, _) in std::env::vars_os() {
        let known = name.to_string_lossy();
        if known.starts_with("KOU_") || known == "CLAUDECODE" || known == "CLAUDE_CODE_ENTRYPOINT" {
            command.env_remove(&name);
        }
    }
    command
}

/// Ask Claude Code what its `/usage` screen shows, for the login in
/// `config_dir` (its `CLAUDE_CONFIG_DIR`; its own default without one).
pub fn claude(
    program: &Path,
    config_dir: Option<&Path>,
    cwd: &Path,
    patience: Duration,
) -> anyhow::Result<Limits> {
    let mut command = command(program, cwd);
    command.args([
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--input-format",
        "stream-json",
        "--no-session-persistence",
    ]);
    if let Some(dir) = config_dir {
        command.env("CLAUDE_CONFIG_DIR", dir);
    }
    let mut talk = Talk::start(command)?;
    let until = Instant::now() + patience;
    let request = |id: &str, request: Value| json!({"type": "control_request", "request_id": id, "request": request});
    talk.say(&request(
        "kou-init",
        json!({"subtype": "initialize", "hooks": null}),
    ))?;
    talk.hear(until, control("kou-init"))?
        .map_err(|error| anyhow!(error))?;
    talk.say(&request(
        "kou-usage",
        json!({"subtype": "get_usage", "skip_behaviors": true}),
    ))?;
    let answer = talk
        .hear(until, control("kou-usage"))?
        .map_err(|error| anyhow!(error))?;
    Ok(claude_limits(&answer))
}

/// The answer to control request `id` on Claude Code's stream: what it
/// answered, or why it refused.
fn control(id: &'static str) -> impl FnMut(&Value) -> Option<Result<Value, String>> {
    move |message| {
        if message.get("type")?.as_str()? != "control_response" {
            return None;
        }
        let response = message.get("response")?;
        if response.get("request_id")?.as_str()? != id {
            return None;
        }
        Some(match response.get("subtype").and_then(Value::as_str) {
            Some("success") => Ok(response.get("response").cloned().unwrap_or(Value::Null)),
            _ => Err(short(
                response
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("it refused the request"),
            )),
        })
    }
}

/// Claude Code's answer as limits: the five hours when there is such a
/// window, the week, and each model's week (`Fable weekly`).
pub fn claude_limits(answer: &Value) -> Limits {
    let plan = answer
        .get("subscription_type")
        .and_then(Value::as_str)
        .map(capitalized);
    let Some(limits) = answer
        .get("rate_limits")
        .filter(|limits| limits.is_object())
    else {
        return Limits {
            plan,
            windows: Vec::new(),
            note: Some("This login has no plan limits (an API key, or another provider)".into()),
        };
    };
    // How near each is, as Claude Code itself judges it, where it says.
    let listed: &[Value] = limits
        .get("limits")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);
    let judged = |kinds: &[&str], model: Option<&str>| -> Option<Level> {
        listed
            .iter()
            .find(|entry| {
                let kind = entry
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let of = entry
                    .pointer("/scope/model/display_name")
                    .and_then(Value::as_str);
                kinds.contains(&kind) && of == model
            })
            .and_then(|entry| entry.get("severity").and_then(Value::as_str))
            .map(|severity| match severity {
                "normal" => Level::Normal,
                "warning" => Level::Warning,
                _ => Level::Reached,
            })
    };
    let window = |name: String, of: &Value, judged: Option<Level>| -> Option<Window> {
        let used = of.get("utilization").and_then(Value::as_f64)? as f32;
        Some(Window {
            name,
            used: used.clamp(0., 100.),
            resets: of
                .get("resets_at")
                .and_then(Value::as_str)
                .and_then(instant),
            level: judged.unwrap_or_else(|| level(used)),
        })
    };
    let mut windows = Vec::new();
    if let Some(found) = limits.get("five_hour").and_then(|of| {
        window(
            "5h".into(),
            of,
            judged(&["session", "five_hour", "five_hour_all"], None),
        )
    }) {
        windows.push(found);
    }
    if let Some(found) = limits
        .get("seven_day")
        .and_then(|of| window("Weekly".into(), of, judged(&["weekly_all"], None)))
    {
        windows.push(found);
    }
    // Each model's own week: as a list, and under the older names.
    let mut models: Vec<String> = Vec::new();
    for of in limits
        .get("model_scoped")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
    {
        let Some(model) = of.get("display_name").and_then(Value::as_str) else {
            continue;
        };
        if let Some(found) = window(
            format!("{model} weekly"),
            of,
            judged(&["weekly_scoped"], Some(model)),
        ) {
            models.push(model.to_lowercase());
            windows.push(found);
        }
    }
    for (key, model) in [("seven_day_opus", "Opus"), ("seven_day_sonnet", "Sonnet")] {
        if models.contains(&model.to_lowercase()) {
            continue;
        }
        if let Some(found) = limits.get(key).and_then(|of| {
            window(
                format!("{model} weekly"),
                of,
                judged(&["weekly_scoped"], Some(model)),
            )
        }) {
            windows.push(found);
        }
    }
    let note = windows
        .is_empty()
        .then(|| "It reports no windows for this plan now".to_owned());
    Limits {
        plan,
        windows,
        note,
    }
}

/// Ask Codex for its account's rate limits, for the login in `home` (its
/// `CODEX_HOME`; its own default without one).
pub fn codex(
    program: &Path,
    home: Option<&Path>,
    cwd: &Path,
    patience: Duration,
) -> anyhow::Result<Limits> {
    let mut command = command(program, cwd);
    command.arg("app-server");
    if let Some(home) = home {
        command.env("CODEX_HOME", home);
    }
    let mut talk = Talk::start(command)?;
    let until = Instant::now() + patience;
    talk.say(&json!({
        "method": "initialize",
        "id": 0,
        "params": {"clientInfo": {
            "name": "kou-desktop",
            "title": "kou desktop",
            "version": env!("CARGO_PKG_VERSION"),
        }},
    }))?;
    talk.hear(until, answered(0))?
        .map_err(|error| anyhow!(error))?;
    talk.say(&json!({"method": "initialized"}))?;
    talk.say(&json!({"method": "account/rateLimits/read", "id": 1}))?;
    let answer = talk.hear(until, answered(1))?.map_err(|error| {
        // Signed in with an API key, or not at all: it has no plan to ask of.
        if error.contains("401") {
            anyhow!("Not signed in with a ChatGPT plan: `codex login` signs in")
        } else {
            anyhow!(error)
        }
    })?;
    Ok(codex_limits(&answer))
}

/// The answer to request `id` of Codex's app server.
fn answered(id: u64) -> impl FnMut(&Value) -> Option<Result<Value, String>> {
    move |message| {
        if message.get("id")?.as_u64()? != id {
            return None;
        }
        if let Some(result) = message.get("result") {
            return Some(Ok(result.clone()));
        }
        Some(Err(short(
            message
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("it refused the request"),
        )))
    }
}

/// Codex's answer as limits: its two windows by their length (`5h`,
/// `Weekly`), then those of its other metered limits under their names.
pub fn codex_limits(answer: &Value) -> Limits {
    let main = answer.get("rateLimits");
    let plan = main
        .and_then(|main| main.get("planType"))
        .and_then(Value::as_str)
        .map(capitalized);
    let mut windows = Vec::new();
    let mut take = |of: Option<&Value>, prefix: Option<&str>| {
        let Some(of) = of else { return };
        let reached = of
            .get("rateLimitReachedType")
            .is_some_and(|kind| !kind.is_null());
        for key in ["primary", "secondary"] {
            let Some(window) = of.get(key).filter(|window| window.is_object()) else {
                continue;
            };
            let Some(used) = window.get("usedPercent").and_then(Value::as_f64) else {
                continue;
            };
            let used = used as f32;
            let length = match window.get("windowDurationMins").and_then(Value::as_u64) {
                Some(300) => "5h".to_owned(),
                Some(10_080) => "Weekly".to_owned(),
                Some(minutes) if minutes % 1440 == 0 => format!("{}d", minutes / 1440),
                Some(minutes) if minutes % 60 == 0 => format!("{}h", minutes / 60),
                Some(minutes) => format!("{minutes}m"),
                None if key == "primary" => "Short".to_owned(),
                None => "Long".to_owned(),
            };
            windows.push(Window {
                name: match prefix {
                    Some(prefix) => format!("{prefix} {}", length.to_lowercase()),
                    None => length,
                },
                used: used.clamp(0., 100.),
                resets: window.get("resetsAt").and_then(Value::as_u64).map(|at| {
                    // Seconds; milliseconds if it is that large.
                    let seconds = if at > 100_000_000_000 { at / 1000 } else { at };
                    UNIX_EPOCH + Duration::from_secs(seconds)
                }),
                level: if reached && used >= 100. {
                    Level::Reached
                } else {
                    level(used)
                },
            });
        }
    };
    take(main, None);
    let main_id = main
        .and_then(|main| main.get("limitId"))
        .and_then(Value::as_str);
    if let Some(others) = answer.get("rateLimitsByLimitId").and_then(Value::as_object) {
        for (id, other) in others {
            if Some(id.as_str()) == main_id {
                continue;
            }
            let name = other
                .get("limitName")
                .and_then(Value::as_str)
                .unwrap_or(id.as_str());
            take(Some(other), Some(name));
        }
    }
    let note = windows
        .is_empty()
        .then(|| "It reports no windows for this plan now".to_owned());
    Limits {
        plan,
        windows,
        note,
    }
}

/// Ask Grok Build for its plan's usage limit. It answers only while its
/// background service runs, and says so otherwise.
pub fn grok(program: &Path, cwd: &Path, patience: Duration) -> anyhow::Result<Limits> {
    let mut command = command(program, cwd);
    command
        .args(["usage", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().context("starting it")?;
    let until = Instant::now() + patience;
    let status = loop {
        match child.try_wait().context("waiting for it")? {
            Some(status) => break status,
            None if Instant::now() >= until => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("it did not answer in time");
            }
            None => std::thread::sleep(Duration::from_millis(25)),
        }
    };
    let read = |pipe: Option<&mut dyn std::io::Read>| {
        let mut text = String::new();
        if let Some(pipe) = pipe {
            let _ = pipe.read_to_string(&mut text);
        }
        text
    };
    let out = read(child.stdout.as_mut().map(|pipe| pipe as _));
    let err = read(child.stderr.as_mut().map(|pipe| pipe as _));
    if status.success()
        && let Ok(answer) = serde_json::from_str::<Value>(&out)
    {
        return Ok(grok_limits(&answer));
    }
    let said = [err.as_str(), out.as_str()]
        .into_iter()
        .flat_map(str::lines)
        .map(|line| line.trim().trim_start_matches("Error:").trim())
        .find(|line| !line.is_empty())
        .unwrap_or("it gave no usage");
    bail!("{}", short(said))
}

/// Grok Build's answer as limits. Its shape is not written down anywhere
/// this was made from, so it is read for what such an answer usually has:
/// objects with a share used, under a name, with a time they start over.
pub fn grok_limits(answer: &Value) -> Limits {
    fn number(of: &Value, keys: &[&str]) -> Option<f64> {
        keys.iter().find_map(|key| of.get(*key)?.as_f64())
    }
    fn text<'a>(of: &'a Value, keys: &[&str]) -> Option<&'a str> {
        keys.iter().find_map(|key| of.get(*key)?.as_str())
    }
    fn gather(of: &Value, name: Option<&str>, windows: &mut Vec<Window>) {
        if windows.len() >= 8 {
            return;
        }
        match of {
            Value::Object(fields) => {
                let share = number(
                    of,
                    &[
                        "percent",
                        "used_percent",
                        "usedPercent",
                        "utilization",
                        "percent_used",
                    ],
                )
                .or_else(|| {
                    let used = number(of, &["used", "usage"])?;
                    let limit = number(of, &["limit", "max", "allowance"])?;
                    (limit > 0.).then(|| used / limit * 100.)
                });
                if let Some(share) = share {
                    let used = share as f32;
                    windows.push(Window {
                        name: text(of, &["display_name", "name", "label", "window", "kind"])
                            .or(name)
                            .map_or_else(|| "Usage".to_owned(), capitalized),
                        used: used.clamp(0., 100.),
                        resets: text(of, &["resets_at", "resetsAt", "reset_at", "resets"])
                            .and_then(instant),
                        level: level(used),
                    });
                    return;
                }
                for (key, inner) in fields {
                    gather(inner, Some(key), windows);
                }
            }
            Value::Array(items) => {
                for inner in items {
                    gather(inner, name, windows);
                }
            }
            _ => {}
        }
    }
    let mut windows = Vec::new();
    gather(answer, None, &mut windows);
    let plan = [
        "plan",
        "plan_name",
        "tier",
        "subscription",
        "subscription_type",
    ]
    .iter()
    .find_map(|key| answer.get(*key)?.as_str())
    .map(capitalized);
    let note = windows
        .is_empty()
        .then(|| "It reports no windows for this plan now".to_owned());
    Limits {
        plan,
        windows,
        note,
    }
}

/// How near its limit a window is, by how much of it is used, where the
/// harness does not judge it itself.
fn level(used: f32) -> Level {
    if used >= 100. {
        Level::Reached
    } else if used >= 80. {
        Level::Warning
    } else {
        Level::Normal
    }
}

/// `text` with a capital first letter: a plan's name as a harness writes it.
fn capitalized(text: &str) -> String {
    let mut letters = text.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => String::new(),
    }
}

/// The first line of what a harness said went wrong, at a length a window
/// can show.
fn short(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    let line = line.split("; content-type").next().unwrap_or(line);
    let mut cut: String = line.chars().take(160).collect();
    if line.chars().count() > 160 {
        cut.push('…');
    }
    cut
}

/// An RFC 3339 time (`2026-11-18T00:00:00.930040+00:00`, `...Z`) as a
/// moment; `None` for anything else.
pub fn instant(text: &str) -> Option<SystemTime> {
    let bytes = text.trim().as_bytes();
    if bytes.len() < 20 || !bytes.is_ascii() {
        return None;
    }
    let number = |from: usize, to: usize| -> Option<i64> {
        std::str::from_utf8(bytes.get(from..to)?).ok()?.parse().ok()
    };
    let mark = |at: usize, marks: &[u8]| bytes.get(at).is_some_and(|byte| marks.contains(byte));
    if !(mark(4, b"-") && mark(7, b"-") && mark(10, b"Tt ") && mark(13, b":") && mark(16, b":")) {
        return None;
    }
    let (year, month, day) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
    let (hour, minute, second) = (number(11, 13)?, number(14, 16)?, number(17, 19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    // Past the fraction of a second, which is dropped, the offset.
    let mut at = 19;
    if mark(at, b".") {
        at += 1;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
    }
    let offset = match bytes.get(at)? {
        b'Z' | b'z' => 0,
        sign @ (b'+' | b'-') => {
            let hours = number(at + 1, at + 3)?;
            let minutes = if mark(at + 3, b":") {
                number(at + 4, at + 6)?
            } else {
                number(at + 3, at + 5)?
            };
            let seconds = hours * 3600 + minutes * 60;
            if *sign == b'-' { -seconds } else { seconds }
        }
        _ => return None,
    };
    // Days since 1970-01-01, by the civil calendar.
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let of_era = shifted.rem_euclid(400);
    let of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let days = era * 146_097 + of_era * 365 + of_era / 4 - of_era / 100 + of_year - 719_468;
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset;
    u64::try_from(seconds)
        .ok()
        .map(|seconds| UNIX_EPOCH + Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn times_are_read_with_their_offsets() {
        assert_eq!(instant("1970-01-01T00:00:00Z"), Some(at(0)));
        assert_eq!(
            instant("2026-11-18T00:00:00.930040+00:00"),
            Some(at(1_794_960_000))
        );
        // An offset is taken off; a leap day counts.
        assert_eq!(
            instant("2026-11-18T03:00:00+03:00"),
            Some(at(1_794_960_000))
        );
        assert_eq!(instant("2026-11-17T19:00:00-0500"), Some(at(1_794_960_000)));
        assert_eq!(instant("2024-03-01T00:00:00Z"), Some(at(1_709_251_200)));
        for odd in [
            "",
            "soon",
            "2026-11-18",
            "2026-13-18T00:00:00Z",
            "2026-11-18T00:00:00",
            "2026-11-18T25:00:00Z",
            "день-недели-и-время-сброса",
        ] {
            assert_eq!(instant(odd), None, "{odd:?}");
        }
    }

    #[test]
    fn claude_s_answer_gives_the_week_and_each_model_s_week() {
        // As Claude Code 2.1.323 answered, with what names an account left out.
        let answer = json!({
            "subscription_type": "max",
            "rate_limits_available": true,
            "rate_limits": {
                "five_hour": null,
                "seven_day": {"utilization": 64, "resets_at": "2026-11-18T00:00:00.930040+00:00"},
                "seven_day_opus": null,
                "seven_day_sonnet": null,
                "extra_usage": {"is_enabled": false},
                "limits": [
                    {"kind": "weekly_all", "group": "weekly", "percent": 64, "severity": "normal", "scope": null},
                    {"kind": "weekly_scoped", "group": "weekly", "percent": 81, "severity": "warning",
                     "scope": {"model": {"display_name": "Fable", "id": null}, "surface": null}},
                ],
                "model_scoped": [
                    {"display_name": "Fable", "utilization": 81, "resets_at": "2026-11-18T00:00:00.930260+00:00"},
                ],
            },
            "session": {"total_cost_usd": 0},
        });
        let limits = claude_limits(&answer);
        assert_eq!(limits.plan.as_deref(), Some("Max"));
        assert_eq!(limits.note, None);
        assert_eq!(
            limits.windows,
            [
                Window {
                    name: "Weekly".into(),
                    used: 64.,
                    resets: Some(at(1_794_960_000)),
                    level: Level::Normal,
                },
                Window {
                    name: "Fable weekly".into(),
                    used: 81.,
                    resets: Some(at(1_794_960_000)),
                    level: Level::Warning,
                },
            ]
        );
    }

    #[test]
    fn claude_s_five_hours_come_first_and_older_names_still_count() {
        let answer = json!({
            "subscription_type": "pro",
            "rate_limits": {
                "five_hour": {"utilization": 100, "resets_at": "2026-11-13T18:00:00Z"},
                "seven_day": {"utilization": 12.5, "resets_at": null},
                "seven_day_opus": {"utilization": 85, "resets_at": "not a time"},
                // No utilization: nothing to draw.
                "seven_day_sonnet": {"utilization": null, "resets_at": null},
            },
        });
        let limits = claude_limits(&answer);
        let shown: Vec<(&str, f32, Level, bool)> = limits
            .windows
            .iter()
            .map(|window| {
                (
                    window.name.as_str(),
                    window.used,
                    window.level,
                    window.resets.is_some(),
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                ("5h", 100., Level::Reached, true),
                ("Weekly", 12.5, Level::Normal, false),
                ("Opus weekly", 85., Level::Warning, false),
            ]
        );
    }

    #[test]
    fn a_login_without_a_plan_or_an_answer_of_another_shape_says_so() {
        let key = claude_limits(&json!({
            "subscription_type": null,
            "rate_limits_available": false,
            "rate_limits": null,
        }));
        assert_eq!((key.plan, key.windows.len()), (None, 0));
        assert!(key.note.is_some());
        for odd in [
            json!(null),
            json!([]),
            json!("usage"),
            json!({"rate_limits": {}}),
        ] {
            let limits = claude_limits(&odd);
            assert!(limits.windows.is_empty(), "{odd}");
            assert!(limits.note.is_some(), "{odd}");
        }
    }

    #[test]
    fn codex_s_windows_are_named_by_their_length() {
        let answer = json!({
            "rateLimits": {
                "limitId": "codex",
                "limitName": null,
                "planType": "plus",
                "primary": {"usedPercent": 37, "windowDurationMins": 300, "resetsAt": 1_794_960_000u64},
                "secondary": {"usedPercent": 92, "windowDurationMins": 10080, "resetsAt": null},
                "rateLimitReachedType": null,
            },
            "rateLimitsByLimitId": {
                "codex": {"limitId": "codex", "primary": {"usedPercent": 37, "windowDurationMins": 300}},
                "codex_other": {
                    "limitId": "codex_other",
                    "limitName": "Spark",
                    "primary": {"usedPercent": 100, "windowDurationMins": 60},
                    "secondary": null,
                    "rateLimitReachedType": "rate_limit_reached",
                },
            },
        });
        let limits = codex_limits(&answer);
        assert_eq!(limits.plan.as_deref(), Some("Plus"));
        let shown: Vec<(&str, f32, Level, bool)> = limits
            .windows
            .iter()
            .map(|window| {
                (
                    window.name.as_str(),
                    window.used,
                    window.level,
                    window.resets.is_some(),
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                ("5h", 37., Level::Normal, true),
                ("Weekly", 92., Level::Warning, false),
                ("Spark 1h", 100., Level::Reached, false),
            ]
        );
        assert!(codex_limits(&json!({})).note.is_some());
    }

    #[test]
    fn grok_s_answer_is_read_for_shares_wherever_they_are() {
        let limits = grok_limits(&json!({
            "plan": "supergrok",
            "limits": [
                {"name": "weekly", "used_percent": 40, "resets_at": "2026-11-18T00:00:00Z"},
                {"label": "daily", "used": 30, "limit": 40},
            ],
        }));
        assert_eq!(limits.plan.as_deref(), Some("Supergrok"));
        let shown: Vec<(&str, f32)> = limits
            .windows
            .iter()
            .map(|window| (window.name.as_str(), window.used))
            .collect();
        assert_eq!(shown, [("Weekly", 40.), ("Daily", 75.)]);
        assert!(grok_limits(&json!({"ok": true})).note.is_some());
    }

    #[test]
    fn what_went_wrong_is_said_in_a_line() {
        let long = "failed to fetch codex rate limits: GET https://chatgpt.com/backend-api/wham/usage failed: 401 Unauthorized; content-type=text/plain; body={\n \"error\": {}}";
        assert_eq!(
            short(long),
            "failed to fetch codex rate limits: GET https://chatgpt.com/backend-api/wham/usage failed: 401 Unauthorized"
        );
        assert_eq!(short(&"я".repeat(400)).chars().count(), 161);
    }

    /// Asks the harnesses installed on this machine, with the logins they
    /// have: no prompt is sent. Run by hand:
    /// `cargo test -p kou-agents usage::tests::live -- --ignored --nocapture`.
    #[test]
    #[ignore = "starts the installed harnesses and asks their accounts for usage; run by hand"]
    fn live() {
        let cwd = std::env::temp_dir();
        let patience = Duration::from_secs(30);
        let said = |limits: anyhow::Result<Limits>| match limits {
            Ok(limits) => format!(
                "plan {:?}, {} windows: {:?}{}",
                limits.plan,
                limits.windows.len(),
                limits
                    .windows
                    .iter()
                    .map(|window| format!("{} {}% {:?}", window.name, window.used, window.level))
                    .collect::<Vec<_>>(),
                limits
                    .note
                    .map(|note| format!(" ({note})"))
                    .unwrap_or_default()
            ),
            Err(error) => format!("error: {error:#}"),
        };
        for (name, ask) in [
            (
                "claude",
                &(|program: &Path| claude(program, None, &cwd, patience))
                    as &dyn Fn(&Path) -> anyhow::Result<Limits>,
            ),
            ("codex", &|program: &Path| {
                codex(program, None, &cwd, patience)
            }),
            ("grok", &|program: &Path| grok(program, &cwd, patience)),
        ] {
            match which::which(name) {
                Ok(program) => println!("{name}: {}", said(ask(&program))),
                Err(_) => println!("{name}: not installed"),
            }
        }
    }
}
