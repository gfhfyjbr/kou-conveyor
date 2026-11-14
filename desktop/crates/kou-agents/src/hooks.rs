//! Turning a harness's own hook events into the few signals the app tracks.

use serde_json::Value;

/// What a hook says about the harness, in kou's terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Signal {
    /// The user's prompt went to the model: a turn began.
    PromptSubmitted,
    /// A tool call is about to run.
    ToolStarted,
    /// A tool call finished, well or badly.
    ToolFinished,
    /// The turn is over and the harness is free.
    TurnFinished,
    /// The harness waits for the user: a permission question or a form.
    NeedsInput,
    /// A session began or was resumed.
    SessionStarted,
    /// The session ended.
    SessionEnded,
}

/// Read one hook invocation: the harness it came from (a [`crate::Spec::id`]),
/// the event name it was registered under, and its payload.
///
/// `None` means the event says nothing about busy or free.
pub fn parse_hook(harness: &str, event: &str, payload: &Value) -> Option<Signal> {
    // The registered name is ours and reliable; the payload's own name covers
    // a hook that was registered by hand without one.
    let event = if event.is_empty() {
        payload.get("hook_event_name")?.as_str()?
    } else {
        event
    };
    match harness {
        "gemini" => gemini(event, payload),
        "cursor" => cursor(event),
        "copilot" => copilot(event, payload),
        "pi" => pi(event),
        "opencode" => opencode(event),
        "aider" => (event == "notification").then_some(Signal::TurnFinished),
        "codex" => codex(event, payload),
        // Claude Code set the vocabulary Droid and Qwen Code follow, and an
        // unknown harness most likely does too.
        _ => claude(event, payload),
    }
}

fn text<'a>(payload: &'a Value, key: &str) -> &'a str {
    payload.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// Claude Code, Droid, Qwen Code. Events and payloads:
/// <https://code.claude.com/docs/en/hooks>,
/// <https://docs.factory.ai/reference/hooks-reference>,
/// qwen-code `docs/users/features/hooks.md`.
fn claude(event: &str, payload: &Value) -> Option<Signal> {
    Some(match event {
        "SessionStart" => Signal::SessionStarted,
        "SessionEnd" => Signal::SessionEnded,
        "UserPromptSubmit" => Signal::PromptSubmitted,
        "PreToolUse" => Signal::ToolStarted,
        "PostToolUse" | "PostToolUseFailure" => Signal::ToolFinished,
        // StopFailure replaces Stop when an API error ends the turn.
        "Stop" | "StopFailure" => Signal::TurnFinished,
        "PermissionRequest" | "Elicitation" => Signal::NeedsInput,
        "Notification" => match text(payload, "notification_type") {
            "permission_prompt"
            | "elicitation_dialog"
            | "elicitation_url_dialog"
            | "agent_needs_input" => Signal::NeedsInput,
            // Sent a minute after a turn ended, and by Droid right after the
            // user cancels one, which fires no Stop.
            "idle_prompt" => Signal::TurnFinished,
            _ => return None,
        },
        _ => return None,
    })
}

/// Codex: Claude's names plus `Interrupt`, and the older `notify` program,
/// whose payload arrives as an argument.
/// <https://developers.openai.com/codex/hooks>,
/// <https://developers.openai.com/codex/config-advanced#notifications>.
fn codex(event: &str, payload: &Value) -> Option<Signal> {
    match event {
        "notify" => {
            (text(payload, "type") == "agent-turn-complete").then_some(Signal::TurnFinished)
        }
        "Interrupt" => Some(Signal::TurnFinished),
        _ => claude(event, payload),
    }
}

/// Gemini CLI: `docs/hooks/reference.md` in google-gemini/gemini-cli.
fn gemini(event: &str, payload: &Value) -> Option<Signal> {
    Some(match event {
        "SessionStart" => Signal::SessionStarted,
        "SessionEnd" => Signal::SessionEnded,
        "BeforeAgent" => Signal::PromptSubmitted,
        "BeforeTool" => Signal::ToolStarted,
        "AfterTool" => Signal::ToolFinished,
        "AfterAgent" => Signal::TurnFinished,
        "Notification" if text(payload, "notification_type") == "ToolPermission" => {
            Signal::NeedsInput
        }
        _ => return None,
    })
}

/// Cursor: <https://cursor.com/docs/hooks>.
fn cursor(event: &str) -> Option<Signal> {
    Some(match event {
        "sessionStart" => Signal::SessionStarted,
        "sessionEnd" => Signal::SessionEnded,
        "beforeSubmitPrompt" => Signal::PromptSubmitted,
        "preToolUse" | "beforeShellExecution" | "beforeMCPExecution" => Signal::ToolStarted,
        "postToolUse"
        | "postToolUseFailure"
        | "afterShellExecution"
        | "afterMCPExecution"
        | "afterFileEdit" => Signal::ToolFinished,
        // `status` is completed, aborted or error; each ends the loop.
        "stop" => Signal::TurnFinished,
        _ => return None,
    })
}

/// GitHub Copilot CLI, camelCase event names:
/// <https://docs.github.com/en/copilot/reference/hooks-configuration>.
fn copilot(event: &str, payload: &Value) -> Option<Signal> {
    Some(match event {
        "sessionStart" => Signal::SessionStarted,
        "sessionEnd" => Signal::SessionEnded,
        "userPromptSubmitted" => Signal::PromptSubmitted,
        "preToolUse" => Signal::ToolStarted,
        "postToolUse" | "postToolUseFailure" => Signal::ToolFinished,
        "agentStop" => Signal::TurnFinished,
        "notification" => match text(payload, "notification_type") {
            "permission_prompt" | "elicitation_dialog" => Signal::NeedsInput,
            _ => return None,
        },
        // The VS Code compatible spelling is Claude's.
        _ => return claude(event, payload),
    })
}

/// Pi, through the extension kou generates: the names are Pi's event names
/// (`docs/extensions.md` in the pi-coding-agent package).
fn pi(event: &str) -> Option<Signal> {
    Some(match event {
        "session_start" => Signal::SessionStarted,
        "session_shutdown" => Signal::SessionEnded,
        "agent_start" => Signal::PromptSubmitted,
        "tool_execution_start" => Signal::ToolStarted,
        "tool_execution_end" => Signal::ToolFinished,
        "agent_end" => Signal::TurnFinished,
        _ => return None,
    })
}

/// OpenCode 2, through the plugin kou generates: the names are the server's
/// event types (<https://opencode.ai/docs/build/plugins/cli#events>).
fn opencode(event: &str) -> Option<Signal> {
    Some(match event {
        "session.execution.started" => Signal::PromptSubmitted,
        "session.execution.succeeded"
        | "session.execution.failed"
        | "session.execution.interrupted" => Signal::TurnFinished,
        "permission.asked" | "form.created" => Signal::NeedsInput,
        // Answered: the turn carries on.
        "permission.replied" | "form.replied" | "form.cancelled" => Signal::ToolStarted,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_code_events() {
        // Shapes from https://code.claude.com/docs/en/hooks ("Common input
        // fields" and each event's input section).
        let prompt = json!({
            "session_id": "abc123",
            "transcript_path": "/Users/.../.claude/projects/.../00893aaf.jsonl",
            "cwd": "/Users/...",
            "permission_mode": "default",
            "hook_event_name": "UserPromptSubmit",
            "prompt": "Write a function to calculate the factorial of a number"
        });
        assert_eq!(
            parse_hook("claude", "UserPromptSubmit", &prompt),
            Some(Signal::PromptSubmitted)
        );
        let tool = json!({
            "session_id": "abc123",
            "prompt_id": "550e8400-e29b-41d4-a716-446655440000",
            "transcript_path": "/home/user/.claude/projects/.../transcript.jsonl",
            "cwd": "/home/user/my-project",
            "permission_mode": "default",
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": "npm test", "description": "Run test suite", "timeout": 120000, "run_in_background": false },
            "tool_use_id": "toolu_01ABC123..."
        });
        assert_eq!(
            parse_hook("claude", "PreToolUse", &tool),
            Some(Signal::ToolStarted)
        );
        assert_eq!(
            parse_hook("claude", "PostToolUse", &tool),
            Some(Signal::ToolFinished)
        );
        assert_eq!(
            parse_hook("claude", "PostToolUseFailure", &tool),
            Some(Signal::ToolFinished)
        );
        let permission = json!({
            "session_id": "abc123",
            "transcript_path": "/Users/.../.claude/projects/.../00893aaf.jsonl",
            "cwd": "/Users/...",
            "permission_mode": "default",
            "hook_event_name": "PermissionRequest",
            "tool_name": "Bash",
            "tool_input": { "command": "rm -rf node_modules", "description": "Remove node_modules directory" },
            "permission_suggestions": [{
                "type": "addRules",
                "rules": [{ "toolName": "Bash", "ruleContent": "rm -rf node_modules" }],
                "behavior": "allow",
                "destination": "localSettings"
            }]
        });
        assert_eq!(
            parse_hook("claude", "PermissionRequest", &permission),
            Some(Signal::NeedsInput)
        );
        let notification = |kind: &str| {
            json!({
                "session_id": "abc123",
                "transcript_path": "/Users/.../.claude/projects/.../00893aaf.jsonl",
                "cwd": "/Users/...",
                "hook_event_name": "Notification",
                "message": "Claude needs your permission",
                "title": "Permission needed",
                "notification_type": kind
            })
        };
        for (kind, signal) in [
            ("permission_prompt", Some(Signal::NeedsInput)),
            ("elicitation_dialog", Some(Signal::NeedsInput)),
            ("elicitation_url_dialog", Some(Signal::NeedsInput)),
            ("agent_needs_input", Some(Signal::NeedsInput)),
            ("idle_prompt", Some(Signal::TurnFinished)),
            ("auth_success", None),
            ("agent_completed", None),
            ("quota_auto_resume_fired", None),
        ] {
            assert_eq!(
                parse_hook("claude", "Notification", &notification(kind)),
                signal,
                "{kind}"
            );
        }
        let stop = json!({
            "session_id": "abc123",
            "transcript_path": "/Users/.../.claude/projects/.../00893aaf.jsonl",
            "cwd": "/Users/...",
            "permission_mode": "default",
            "hook_event_name": "Stop",
            "stop_hook_active": false,
            "last_assistant_message": "Done.",
            "background_tasks": [],
            "session_crons": []
        });
        assert_eq!(
            parse_hook("claude", "Stop", &stop),
            Some(Signal::TurnFinished)
        );
        let failure = json!({
            "session_id": "abc123",
            "transcript_path": "/Users/.../.claude/projects/.../00893aaf.jsonl",
            "cwd": "/Users/...",
            "hook_event_name": "StopFailure",
            "error": "rate_limit",
            "error_details": "429 Too Many Requests",
            "last_assistant_message": "API Error: Rate limit reached"
        });
        assert_eq!(
            parse_hook("claude", "StopFailure", &failure),
            Some(Signal::TurnFinished)
        );
        let start = json!({
            "session_id": "abc123",
            "transcript_path": "/Users/.../.claude/projects/.../00893aaf.jsonl",
            "cwd": "/Users/...",
            "hook_event_name": "SessionStart",
            "source": "resume",
            "model": "claude-opus-5"
        });
        assert_eq!(
            parse_hook("claude", "SessionStart", &start),
            Some(Signal::SessionStarted)
        );
        let end = json!({
            "session_id": "abc123",
            "transcript_path": "/Users/.../.claude/projects/.../00893aaf.jsonl",
            "cwd": "/Users/...",
            "hook_event_name": "SessionEnd",
            "reason": "other"
        });
        assert_eq!(
            parse_hook("claude", "SessionEnd", &end),
            Some(Signal::SessionEnded)
        );
        // Events that say nothing about busy or free.
        for event in [
            "SubagentStop",
            "PreCompact",
            "ConfigChange",
            "MessageDisplay",
        ] {
            assert_eq!(parse_hook("claude", event, &Value::Null), None, "{event}");
        }
        // A hook registered without a name is read from its payload.
        assert_eq!(parse_hook("claude", "", &stop), Some(Signal::TurnFinished));
        assert_eq!(parse_hook("claude", "", &Value::Null), None);
    }

    #[test]
    fn codex_events() {
        // The notify program's single argument, as in
        // https://developers.openai.com/codex/config-advanced#notifications
        let notify = json!({
            "type": "agent-turn-complete",
            "thread-id": "b5f6c1c2-1111-2222-3333-444455556666",
            "turn-id": "12345",
            "cwd": "/Users/alice/projects/example",
            "input-messages": ["Rename `foo` to `bar` and update the callsites."],
            "last-assistant-message": "Rename complete and verified `cargo build` succeeds."
        });
        assert_eq!(
            parse_hook("codex", "notify", &notify),
            Some(Signal::TurnFinished)
        );
        assert_eq!(
            parse_hook("codex", "notify", &json!({ "type": "something-new" })),
            None
        );
        // Hook input: https://developers.openai.com/codex/hooks ("Common input
        // fields" plus each event's table).
        let common = |event: &str| {
            json!({
                "session_id": "019a1b2c",
                "transcript_path": null,
                "cwd": "/work",
                "hook_event_name": event,
                "model": "gpt-5.1-codex",
                "permission_mode": "default",
                "turn_id": "turn-7"
            })
        };
        for (event, signal) in [
            ("SessionStart", Signal::SessionStarted),
            ("UserPromptSubmit", Signal::PromptSubmitted),
            ("PreToolUse", Signal::ToolStarted),
            ("PostToolUse", Signal::ToolFinished),
            ("PermissionRequest", Signal::NeedsInput),
            ("Stop", Signal::TurnFinished),
            ("Interrupt", Signal::TurnFinished),
            ("SessionEnd", Signal::SessionEnded),
        ] {
            assert_eq!(
                parse_hook("codex", event, &common(event)),
                Some(signal),
                "{event}"
            );
        }
        assert_eq!(
            parse_hook("codex", "PreCompact", &common("PreCompact")),
            None
        );
    }

    #[test]
    fn droid_and_qwen_follow_claude() {
        // https://docs.factory.ai/reference/hooks-reference: "Hook input".
        let droid = json!({
            "session_id": "abc123",
            "transcript_path": "/Users/.../.factory/projects/.../session.jsonl",
            "cwd": "/Users/me/project",
            "permission_mode": "off",
            "hook_event_name": "PreToolUse",
            "tool_name": "Create",
            "tool_input": { "file_path": "/Users/me/project/file.txt", "content": "file content" }
        });
        assert_eq!(
            parse_hook("droid", "PreToolUse", &droid),
            Some(Signal::ToolStarted)
        );
        // Cancelling a turn sends this in place of Stop.
        let cancelled = json!({
            "session_id": "abc123",
            "hook_event_name": "Notification",
            "message": "Droid is waiting for your input",
            "notification_type": "idle_prompt"
        });
        assert_eq!(
            parse_hook("droid", "Notification", &cancelled),
            Some(Signal::TurnFinished)
        );
        // qwen-code docs/users/features/hooks.md: "Hook Input Structure".
        let qwen = json!({
            "session_id": "s",
            "transcript_path": "/t.jsonl",
            "cwd": "/work",
            "hook_event_name": "Notification",
            "timestamp": "2026-11-11T10:00:00Z",
            "permission_mode": "default",
            "notification_type": "permission_prompt"
        });
        assert_eq!(
            parse_hook("qwen", "Notification", &qwen),
            Some(Signal::NeedsInput)
        );
        assert_eq!(
            parse_hook("qwen", "PermissionRequest", &qwen),
            Some(Signal::NeedsInput)
        );
        assert_eq!(
            parse_hook("qwen", "StopFailure", &qwen),
            Some(Signal::TurnFinished)
        );
    }

    #[test]
    fn gemini_events() {
        // docs/hooks/reference.md: "Base input schema" plus each event's fields.
        let base = |event: &str| {
            json!({
                "session_id": "s",
                "transcript_path": "/abs/transcript.json",
                "cwd": "/work",
                "hook_event_name": event,
                "timestamp": "2026-11-11T10:00:00.000Z"
            })
        };
        for (event, signal) in [
            ("SessionStart", Signal::SessionStarted),
            ("BeforeAgent", Signal::PromptSubmitted),
            ("BeforeTool", Signal::ToolStarted),
            ("AfterTool", Signal::ToolFinished),
            ("AfterAgent", Signal::TurnFinished),
            ("SessionEnd", Signal::SessionEnded),
        ] {
            assert_eq!(
                parse_hook("gemini", event, &base(event)),
                Some(signal),
                "{event}"
            );
        }
        let mut notification = base("Notification");
        notification["notification_type"] = json!("ToolPermission");
        notification["message"] = json!("Allow run_shell_command?");
        assert_eq!(
            parse_hook("gemini", "Notification", &notification),
            Some(Signal::NeedsInput)
        );
        for event in [
            "BeforeModel",
            "AfterModel",
            "BeforeToolSelection",
            "PreCompress",
        ] {
            assert_eq!(parse_hook("gemini", event, &base(event)), None, "{event}");
        }
    }

    #[test]
    fn cursor_events() {
        // https://cursor.com/docs/hooks: "Input (all hooks)" plus each event.
        let base = |event: &str| {
            json!({
                "conversation_id": "c",
                "generation_id": "g",
                "model": "auto",
                "hook_event_name": event,
                "cursor_version": "2026.11.05",
                "workspace_roots": ["/work"],
                "user_email": null,
                "transcript_path": null
            })
        };
        let mut prompt = base("beforeSubmitPrompt");
        prompt["prompt"] = json!("fix the tests");
        prompt["attachments"] = json!([]);
        assert_eq!(
            parse_hook("cursor", "beforeSubmitPrompt", &prompt),
            Some(Signal::PromptSubmitted)
        );
        for status in ["completed", "aborted", "error"] {
            let mut stop = base("stop");
            stop["status"] = json!(status);
            stop["loop_count"] = json!(0);
            assert_eq!(
                parse_hook("cursor", "stop", &stop),
                Some(Signal::TurnFinished)
            );
        }
        for (event, signal) in [
            ("sessionStart", Signal::SessionStarted),
            ("preToolUse", Signal::ToolStarted),
            ("beforeShellExecution", Signal::ToolStarted),
            ("postToolUse", Signal::ToolFinished),
            ("afterFileEdit", Signal::ToolFinished),
            ("sessionEnd", Signal::SessionEnded),
        ] {
            assert_eq!(
                parse_hook("cursor", event, &base(event)),
                Some(signal),
                "{event}"
            );
        }
        assert_eq!(
            parse_hook("cursor", "afterAgentThought", &base("afterAgentThought")),
            None
        );
    }

    #[test]
    fn copilot_events() {
        // docs.github.com/en/copilot/reference/hooks-configuration, camelCase inputs.
        let prompt = json!({ "sessionId": "s", "timestamp": 1762855200000u64, "cwd": "/work", "prompt": "hi" });
        assert_eq!(
            parse_hook("copilot", "userPromptSubmitted", &prompt),
            Some(Signal::PromptSubmitted)
        );
        let stop = json!({
            "sessionId": "s", "timestamp": 1762855200000u64, "cwd": "/work",
            "transcriptPath": "/t.jsonl", "stopReason": "end_turn", "stop_hook_active": false
        });
        assert_eq!(
            parse_hook("copilot", "agentStop", &stop),
            Some(Signal::TurnFinished)
        );
        let notification = |kind: &str| {
            json!({
                "sessionId": "s", "timestamp": 1762855200000u64, "cwd": "/work",
                "hook_event_name": "Notification", "message": "Permission needed",
                "title": "Permission needed", "notification_type": kind
            })
        };
        assert_eq!(
            parse_hook(
                "copilot",
                "notification",
                &notification("permission_prompt")
            ),
            Some(Signal::NeedsInput)
        );
        assert_eq!(
            parse_hook("copilot", "notification", &notification("shell_completed")),
            None
        );
        assert_eq!(
            parse_hook("copilot", "preToolUse", &Value::Null),
            Some(Signal::ToolStarted)
        );
        assert_eq!(
            parse_hook("copilot", "sessionEnd", &Value::Null),
            Some(Signal::SessionEnded)
        );
    }

    #[test]
    fn events_forwarded_by_generated_extensions() {
        // Pi: the extension sends the event name and a small object of its own.
        for (event, signal) in [
            ("session_start", Signal::SessionStarted),
            ("agent_start", Signal::PromptSubmitted),
            ("tool_execution_start", Signal::ToolStarted),
            ("tool_execution_end", Signal::ToolFinished),
            ("agent_end", Signal::TurnFinished),
            ("session_shutdown", Signal::SessionEnded),
        ] {
            let payload = json!({ "toolName": "bash" });
            assert_eq!(parse_hook("pi", event, &payload), Some(signal), "{event}");
        }
        assert_eq!(parse_hook("pi", "turn_start", &Value::Null), None);
        // OpenCode: event types found in the 2.2.5 binary.
        for (event, signal) in [
            ("session.execution.started", Signal::PromptSubmitted),
            ("session.execution.succeeded", Signal::TurnFinished),
            ("session.execution.failed", Signal::TurnFinished),
            ("session.execution.interrupted", Signal::TurnFinished),
            ("permission.asked", Signal::NeedsInput),
            ("permission.replied", Signal::ToolStarted),
            ("form.created", Signal::NeedsInput),
        ] {
            assert_eq!(
                parse_hook("opencode", event, &Value::Null),
                Some(signal),
                "{event}"
            );
        }
        // Aider's notification command carries no payload at all.
        assert_eq!(
            parse_hook("aider", "notification", &Value::Null),
            Some(Signal::TurnFinished)
        );
        assert_eq!(parse_hook("aider", "Stop", &Value::Null), None);
    }
}
