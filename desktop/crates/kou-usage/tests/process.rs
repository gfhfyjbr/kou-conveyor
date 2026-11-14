//! Asking programs that stand in for the agents': what is sent to them, in
//! what environment, and that none is ever left running.
#![cfg(unix)]

use kou_usage::{Ask, Kind, Reason, limits};
use std::{
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// A program made of a shell script, in `folder`.
fn program(folder: &Path, name: &str, script: &str) -> PathBuf {
    let path = folder.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).expect("a script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("executable");
    path
}

fn ask(program: PathBuf, env: &[(&str, &str)], timeout: Duration) -> Ask {
    Ask {
        program,
        env: env
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
        timeout,
    }
}

/// Whether a process with this id is still there.
fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[test]
fn claude_is_asked_over_its_control_protocol_in_the_account_s_home() {
    let folder = tempfile::tempdir().expect("a folder");
    let heard = folder.path().join("heard");
    // Answers the two requests it is sent, and notes how it was run.
    let script = format!(
        r#"echo "$@" > "{heard}.args"
echo "$CLAUDE_CONFIG_DIR|${{KOU_PANE:-unset}}|$PWD" > "{heard}.env"
read -r hello
echo 'not json at all'
echo '{{"type":"control_response","response":{{"subtype":"success","request_id":"kou-hello","response":{{}}}}}}'
read -r asked
echo "$hello" > "{heard}.hello"
echo "$asked" > "{heard}.asked"
echo '{{"type":"system","subtype":"status"}}'
echo '{{"type":"control_response","response":{{"subtype":"success","request_id":"kou-usage","response":{{"subscription_type":"max","rate_limits_available":true,"rate_limits":{{"limits":[{{"kind":"weekly_all","percent":66,"resets_at":"2026-11-18T00:00:01Z"}},{{"kind":"weekly_scoped","percent":84,"scope":{{"model":{{"display_name":"Fable"}}}}}}]}}}}}}}}'
cat > /dev/null"#,
        heard = heard.display()
    );
    let claude = program(folder.path(), "claude", &script);
    // The app's own pane is not the asked program's.
    // SAFETY: tests of this file set no other variable and read this one
    // only in the children they start.
    unsafe { std::env::set_var("KOU_PANE", "p1") };
    let answer = limits(
        "claude",
        &ask(
            claude,
            &[("CLAUDE_CONFIG_DIR", "/accounts/work")],
            Duration::from_secs(10),
        ),
    )
    .expect("claude can be asked")
    .expect("limits");
    assert_eq!(answer.plan.as_deref(), Some("Max"));
    let shown: Vec<(Kind, f32)> = answer
        .windows
        .iter()
        .map(|window| (window.kind.clone(), window.used))
        .collect();
    assert_eq!(
        shown,
        [(Kind::Weekly, 66.), (Kind::Model("Fable".into()), 84.)]
    );
    let read = |part: &str| std::fs::read_to_string(format!("{}.{part}", heard.display())).unwrap();
    // No prompt among what it was sent: a greeting, and the question.
    let asked: serde_json::Value = serde_json::from_str(&read("asked")).unwrap();
    assert_eq!(asked["type"], "control_request");
    assert_eq!(asked["request"]["subtype"], "get_usage");
    assert_eq!(asked["request"]["skip_behaviors"], true);
    let hello: serde_json::Value = serde_json::from_str(&read("hello")).unwrap();
    assert_eq!(hello["request"]["subtype"], "initialize");
    // Kept off disk, without hooks or servers of the user's.
    let args = read("args");
    for flag in [
        "--print",
        "--no-session-persistence",
        "--strict-mcp-config",
        "disableAllHooks",
    ] {
        assert!(args.contains(flag), "{flag} in {args}");
    }
    let env = read("env");
    let mut env = env.trim().split('|');
    assert_eq!(env.next(), Some("/accounts/work"));
    assert_eq!(env.next(), Some("unset"), "the app's pane is not passed on");
    let temp = std::env::temp_dir().canonicalize().unwrap();
    assert_eq!(
        Path::new(env.next().unwrap()).canonicalize().unwrap(),
        temp,
        "not run in a project"
    );
}

#[test]
fn codex_is_greeted_and_then_asked_for_the_account_s_limits() {
    let folder = tempfile::tempdir().expect("a folder");
    let heard = folder.path().join("heard");
    let script = format!(
        r#"echo "$1|$CODEX_HOME" > "{heard}.run"
read -r hello
echo '{{"id":1,"result":{{"userAgent":"fake"}}}}'
echo '{{"method":"configWarning","params":{{}}}}'
read -r ready
read -r asked
printf '%s\n%s\n%s\n' "$hello" "$ready" "$asked" > "{heard}.lines"
echo '{{"id":2,"result":{{"rateLimits":{{"planType":"pro","primary":{{"usedPercent":31,"windowDurationMins":300,"resetsAt":1794960001}},"secondary":{{"usedPercent":78,"windowDurationMins":10080}}}}}}}}'
cat > /dev/null"#,
        heard = heard.display()
    );
    let codex = program(folder.path(), "codex", &script);
    let answer = limits(
        "codex",
        &ask(
            codex,
            &[("CODEX_HOME", "/accounts/second")],
            Duration::from_secs(10),
        ),
    )
    .expect("codex can be asked")
    .expect("limits");
    assert_eq!(answer.plan.as_deref(), Some("Pro"));
    let shown: Vec<(Kind, f32)> = answer
        .windows
        .iter()
        .map(|window| (window.kind.clone(), window.used))
        .collect();
    assert_eq!(shown, [(Kind::Session, 31.), (Kind::Weekly, 78.)]);
    assert_eq!(
        std::fs::read_to_string(format!("{}.run", heard.display()))
            .unwrap()
            .trim(),
        "app-server|/accounts/second"
    );
    let lines = std::fs::read_to_string(format!("{}.lines", heard.display())).unwrap();
    let methods: Vec<String> = lines
        .lines()
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()["method"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    // No thread is started and no turn: a greeting, and the question.
    assert_eq!(
        methods,
        ["initialize", "initialized", "account/rateLimits/read"]
    );
}

#[test]
fn grok_is_run_once_and_its_refusal_is_passed_on() {
    let folder = tempfile::tempdir().expect("a folder");
    let refusing = program(
        folder.path(),
        "grok",
        r#"echo "Error: \`grok usage\` can't show your plan's usage here yet. Run /usage inside grok instead." >&2
exit 1"#,
    );
    let answer =
        limits("grok", &ask(refusing, &[], Duration::from_secs(10))).expect("grok can be asked");
    assert!(matches!(answer, Err(Reason::NotHere(_))), "{answer:?}");
    let answering = program(
        folder.path(),
        "grok-up",
        r#"[ "$1 $2" = "usage --json" ] || exit 9
echo '{"weekly": {"used_percent": 12}}'"#,
    );
    let answer = limits("grok", &ask(answering, &[], Duration::from_secs(10)))
        .expect("grok can be asked")
        .expect("limits");
    assert_eq!(answer.windows[0].kind, Kind::Weekly);
    assert!(limits("shell", &ask("/bin/sh".into(), &[], Duration::from_secs(1))).is_none());
}

#[test]
fn a_program_that_never_answers_is_ended_when_the_time_is_up_with_what_it_started() {
    let folder = tempfile::tempdir().expect("a folder");
    let pids = folder.path().join("pids");
    // It starts a child of its own and then both wait for ever.
    let script = format!(
        r#"sleep 600 &
echo "$$ $!" > "{pids}"
sleep 600"#,
        pids = pids.display()
    );
    for harness in ["claude", "codex", "grok"] {
        let _ = std::fs::remove_file(&pids);
        let silent = program(folder.path(), harness, &script);
        let started = Instant::now();
        let answer = limits(harness, &ask(silent, &[], Duration::from_millis(400))).expect("asked");
        let took = started.elapsed();
        let said = answer.expect_err("no answer").message();
        assert!(said.contains("did not answer in time"), "{harness}: {said}");
        assert!(took < Duration::from_secs(5), "{harness}: it took {took:?}");
        // Neither the program nor what it started is left.
        let noted = std::fs::read_to_string(&pids).expect("it ran");
        for pid in noted.split_whitespace() {
            let gone = (0..50).any(|_| {
                std::thread::sleep(Duration::from_millis(20));
                !alive(pid)
            });
            assert!(gone, "{harness}: process {pid} was left running");
        }
    }
}

#[test]
fn a_program_that_is_not_there_or_ends_early_is_a_failure_that_says_so() {
    let missing = limits(
        "claude",
        &ask("/no/such/claude".into(), &[], Duration::from_secs(2)),
    )
    .expect("asked")
    .expect_err("not there");
    assert!(
        missing
            .message()
            .starts_with("Claude Code could not be started"),
        "{}",
        missing.message()
    );
    let folder = tempfile::tempdir().expect("a folder");
    let quits = program(folder.path(), "claude", "exit 3");
    let ended = limits("claude", &ask(quits.clone(), &[], Duration::from_secs(5)))
        .expect("asked")
        .expect_err("it ended");
    // (Whether the question could still be written or not, nothing came back.)
    assert!(
        ended.message().contains("ended without an answer")
            || ended.message().contains("did not take"),
        "{}",
        ended.message()
    );
    let ended = limits("codex", &ask(quits, &[], Duration::from_secs(5)))
        .expect("asked")
        .expect_err("it ended");
    assert!(
        ended.message().contains("ended without an answer")
            || ended.message().contains("did not take"),
        "{}",
        ended.message()
    );
}
