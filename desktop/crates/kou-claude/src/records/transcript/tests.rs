use super::*;
use kou_thread::{
    Capabilities, Entry, PlanStatus, Thread, ThreadState, ToolCall, ToolContent, ToolKind,
};
use serde_json::{Value, json};
use std::time::Duration;

/// A record made up here, line by line, as Claude Code writes one.
struct Lines {
    lines: Vec<Value>,
    last: Option<String>,
    count: usize,
}

impl Lines {
    fn new() -> Self {
        Lines {
            lines: Vec::new(),
            last: None,
            count: 0,
        }
    }

    /// A line after the last one, with its own id; the id.
    fn push(&mut self, mut line: Value) -> String {
        self.count += 1;
        let uuid = line
            .get("uuid")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("line-{}", self.count));
        let fields = line.as_object_mut().expect("an object");
        fields.insert("uuid".into(), json!(uuid));
        if !fields.contains_key("parentUuid") {
            fields.insert("parentUuid".into(), json!(self.last));
        }
        fields.entry("timestamp").or_insert(json!(format!(
            "2026-11-15T10:00:{:02}.000Z",
            self.count % 60
        )));
        fields.entry("sessionId").or_insert(json!("s"));
        fields.entry("cwd").or_insert(json!("/w/p"));
        self.lines.push(line);
        self.last = Some(uuid.clone());
        uuid
    }

    fn prompt(&mut self, text: &str) -> String {
        self.push(json!({"type": "user", "message": {"role": "user", "content": text}}))
    }

    fn says(&mut self, block: Value) -> String {
        self.push(json!({"type": "assistant", "message": {
            "id": format!("msg-{}", self.count + 1), "role": "assistant", "content": [block]}}))
    }

    fn calls(&mut self, id: &str, name: &str, input: Value) -> String {
        self.says(json!({"type": "tool_use", "id": id, "name": name, "input": input}))
    }

    fn answers(
        &mut self,
        id: &str,
        content: Value,
        structured: Option<Value>,
        failed: bool,
    ) -> String {
        let mut line = json!({"type": "user", "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": id, "content": content, "is_error": failed}]}});
        if let Some(structured) = structured {
            line["toolUseResult"] = structured;
        }
        self.push(line)
    }

    fn text(&self) -> String {
        self.lines
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    fn write(&self, path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, self.text()).unwrap();
    }
}

fn replayed(lines: &Lines) -> (tempfile::TempDir, Vec<Update>) {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("s.jsonl");
    lines.write(&path);
    let updates = replay(&path);
    (folder, updates)
}

/// The thread the updates make, after a session is ready as a backend says.
fn thread_of(updates: &[Update]) -> Thread {
    let mut thread = Thread::new();
    thread.apply(Update::Ready {
        session: "s".into(),
        capabilities: Capabilities::default(),
        modes: Vec::new(),
        mode: None,
        models: Vec::new(),
        model: None,
    });
    for update in updates {
        thread.apply(update.clone());
    }
    thread
}

fn tool_entries(thread: &Thread) -> Vec<&ToolCall> {
    thread
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            Entry::Tool(call) => Some(call),
            _ => None,
        })
        .collect()
}

fn user_text(entry: &Entry) -> String {
    match entry {
        Entry::User(blocks) => blocks
            .iter()
            .map(|block| match block {
                Block::Text(text) => text.clone(),
                Block::Image { .. } => "[picture]".into(),
                Block::File { path, .. } => format!("[file {}]", path.display()),
            })
            .collect(),
        other => panic!("not the user's: {other:?}"),
    }
}

#[test]
fn a_conversation_comes_back_as_its_prompts_answers_thinking_and_calls() {
    let mut lines = Lines::new();
    let first = lines.prompt("Run the tests");
    lines.says(json!({"type": "thinking", "thinking": "Let me run them.", "signature": "x"}));
    lines.says(json!({"type": "text", "text": "Running them."}));
    lines.calls(
        "toolu_1",
        "Bash",
        json!({"command": "cargo test", "description": "Run tests"}),
    );
    lines.answers(
        "toolu_1",
        json!("ok"),
        Some(json!({"stdout": "ok", "stderr": "", "interrupted": false})),
        false,
    );
    lines.says(json!({"type": "text", "text": "All pass."}));
    let second = lines.prompt("Thanks");
    lines.says(json!({"type": "text", "text": "You're welcome."}));
    let (_folder, updates) = replayed(&lines);

    assert_eq!(
        updates[0],
        Update::UserChunk(Block::Text("Run the tests".into()))
    );
    assert_eq!(updates[1], Update::PromptId(first.clone()));
    assert_eq!(updates[2], Update::ThoughtChunk("Let me run them.".into()));
    assert_eq!(updates[3], Update::AssistantChunk("Running them.".into()));
    let Update::Tool(call) = &updates[4] else {
        panic!("a call: {:?}", updates[4]);
    };
    assert_eq!(
        (call.title.as_str(), call.kind),
        ("Run cargo test", ToolKind::Execute)
    );
    assert!(
        matches!(&updates[5], Update::ToolPatch(patch) if patch.status == Some(ToolStatus::Done))
    );

    let thread = thread_of(&updates);
    assert_eq!(thread.state(), &ThreadState::Idle);
    let entries = thread.entries();
    assert_eq!(user_text(&entries[0]), "Run the tests");
    assert_eq!(entries[1], Entry::Thought("Let me run them.".into()));
    assert_eq!(entries[2], Entry::Assistant("Running them.".into()));
    let Entry::Tool(call) = &entries[3] else {
        panic!("a call");
    };
    assert_eq!(call.status, ToolStatus::Done);
    assert_eq!(
        call.content,
        [ToolContent::Terminal {
            id: "toolu_1".into(),
            output: "ok".into(),
            exit: Some(0)
        }]
    );
    assert_eq!(entries[4], Entry::Assistant("All pass.".into()));
    assert_eq!(user_text(&entries[5]), "Thanks");
    // Each prompt is known by its line's id, for an edit of it.
    assert_eq!(thread.prompt_id(0), Some(first.as_str()));
    assert_eq!(thread.prompt_id(5), Some(second.as_str()));
    assert_eq!(entries.len(), 7);
}

#[test]
fn a_rewound_conversation_does_not_show_its_abandoned_branch() {
    let mut lines = Lines::new();
    lines.prompt("first");
    let answer = lines.says(json!({"type": "text", "text": "one"}));
    lines.prompt("second, abandoned");
    lines.says(json!({"type": "text", "text": "two, abandoned"}));
    // The user edited the second prompt: the new branch goes on from `one`.
    lines.last = Some(answer);
    lines.prompt("second, edited");
    lines.says(json!({"type": "text", "text": "two"}));
    lines.push(json!({"type": "system", "subtype": "turn_duration", "durationMs": 5}));
    lines.push(json!({"type": "last-prompt", "lastPrompt": "second, edited", "leafUuid": "x"}));
    let (_folder, updates) = replayed(&lines);
    let thread = thread_of(&updates);
    let shown: Vec<String> = thread
        .entries()
        .iter()
        .map(|entry| match entry {
            Entry::Assistant(text) => text.clone(),
            entry => user_text(entry),
        })
        .collect();
    assert_eq!(shown, ["first", "one", "second, edited", "two"]);
}

#[test]
fn what_the_user_did_not_type_is_not_shown_as_theirs() {
    let mut lines = Lines::new();
    lines.push(json!({"type": "user", "isMeta": true, "message": {"role": "user",
        "content": "<local-command-caveat>Caveat: the messages below were generated by the user while running local commands.</local-command-caveat>"}}));
    lines.prompt("<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args>opus</command-args>");
    lines.prompt("<local-command-stdout>Set model to opus</local-command-stdout>");
    let typed = lines.prompt("<command-message>review</command-message>\n<command-name>/review</command-name>\n<command-args>the diff</command-args>");
    lines.says(json!({"type": "text", "text": "Reviewing."}));
    lines.calls("toolu_1", "Read", json!({"file_path": "/w/p/a.rs"}));
    lines.answers("toolu_1", json!("     1\tfn a() {}"), None, false);
    lines.push(json!({"type": "user", "message": {"role": "user", "content": [
        {"type": "text", "text": "and this<system-reminder>The user opened a.rs</system-reminder>"}]}}));
    lines.push(json!({"type": "user", "isMeta": true, "message": {"role": "user", "content": "Continue from where you left off."}}));
    lines.prompt(
        "<kou-desktop>\nA note from the app this session runs in, not from the user:\nhi\n</kou-desktop>",
    );
    let (_folder, updates) = replayed(&lines);
    let thread = thread_of(&updates);
    let users: Vec<String> = thread
        .entries()
        .iter()
        .filter(|entry| matches!(entry, Entry::User(_)))
        .map(user_text)
        .collect();
    assert_eq!(users, ["/review the diff", "and this"]);
    assert!(updates.contains(&Update::PromptId(typed)));
    // The read is a call, its file named from the folder the session ran in.
    assert_eq!(tool_entries(&thread)[0].title, "Read a.rs");
}

#[test]
fn pictures_in_a_prompt_come_back_as_pictures() {
    let mut lines = Lines::new();
    lines.push(json!({"type": "user", "message": {"role": "user", "content": [
        {"type": "text", "text": "what is this?"},
        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw=="}}]}}));
    lines.says(json!({"type": "text", "text": "A picture."}));
    let (_folder, updates) = replayed(&lines);
    let thread = thread_of(&updates);
    assert_eq!(
        thread.entries()[0],
        Entry::User(vec![
            Block::Text("what is this?".into()),
            Block::Image {
                mime: "image/png".into(),
                data: vec![0x89, b'P', b'N', b'G'].into()
            }
        ])
    );
}

#[test]
fn an_interruption_an_api_error_and_a_turn_of_thinking_only_are_told_as_such() {
    let mut lines = Lines::new();
    lines.prompt("go");
    lines.calls("toolu_1", "Bash", json!({"command": "sleep 100"}));
    lines.answers(
        "toolu_1",
        json!("The user doesn't want to proceed with this tool use. The tool use was rejected."),
        None,
        true,
    );
    lines.push(json!({"type": "user", "interruptedMessageId": "m",
        "message": {"role": "user", "content": [{"type": "text", "text": "[Request interrupted by user for tool use]"}]}}));
    lines.prompt("again");
    lines.push(
        json!({"type": "assistant", "isApiErrorMessage": true, "error": "overloaded",
        "message": {"id": "e", "model": "<synthetic>", "role": "assistant",
            "content": [{"type": "text", "text": "API Error: 529 Overloaded"}]}}),
    );
    lines.prompt("think about it");
    lines.says(json!({"type": "thinking", "thinking": "Hmm.", "signature": "s"}));
    lines.prompt("well?");
    let (_folder, updates) = replayed(&lines);
    let thread = thread_of(&updates);
    let entries = thread.entries();
    assert_eq!(user_text(&entries[0]), "go");
    let Entry::Tool(call) = &entries[1] else {
        panic!("the call");
    };
    assert_eq!(call.status, ToolStatus::Cancelled, "refused, not failed");
    assert_eq!(
        entries[2],
        Entry::Notice(Level::Info, "Stopped by the user".into())
    );
    assert_eq!(user_text(&entries[3]), "again");
    assert_eq!(
        entries[4],
        Entry::Notice(Level::Error, "API Error: 529 Overloaded".into())
    );
    assert_eq!(user_text(&entries[5]), "think about it");
    assert_eq!(entries[6], Entry::Thought("Hmm.".into()));
    assert_eq!(user_text(&entries[7]), "well?");
    assert_eq!(entries.len(), 8);
    assert_eq!(thread.state(), &ThreadState::Idle);
}

#[test]
fn a_broken_last_line_an_empty_file_and_one_that_is_no_record_replay_what_can_be_read() {
    let mut lines = Lines::new();
    lines.prompt("hello");
    lines.says(json!({"type": "text", "text": "hi"}));
    let mut text = lines.text();
    // The file is being written while it is read.
    text.push_str(
        "{\"type\":\"assistant\",\"uuid\":\"half\",\"message\":{\"content\":[{\"type\":\"te",
    );
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("s.jsonl");
    fs::write(&path, text).unwrap();
    let thread = thread_of(&replay(&path));
    assert_eq!(thread.entries().len(), 2);
    assert_eq!(thread.entries()[1], Entry::Assistant("hi".into()));

    let empty = folder.path().join("empty.jsonl");
    fs::write(&empty, "").unwrap();
    assert!(replay(&empty).is_empty());
    let garbage = folder.path().join("garbage.jsonl");
    fs::write(
        &garbage,
        b"\x89PNG\r\n\x1a\n\x00\x00 not json {[}\n[1,2]\n\"x\"\n",
    )
    .unwrap();
    assert!(replay(&garbage).is_empty());
    assert!(replay(&folder.path().join("missing.jsonl")).is_empty());
}

#[test]
fn records_and_fields_nobody_knows_are_skipped() {
    let mut lines = Lines::new();
    lines.push(json!({"type": "session-owner", "accountUuid": "a"}));
    lines.push(json!({"type": "atis-latch", "atis": "x"}));
    lines.prompt("hello");
    lines.push(json!({"type": "attachment", "attachment": {"type": "instructions", "files": []}}));
    lines.push(json!({"type": "system", "subtype": "something_new", "content": "?"}));
    lines.push(
        json!({"type": "assistant", "brandNewField": {"deep": [1, 2]}, "isSidechain": "maybe",
        "message": {"id": "m", "role": "assistant", "content": [
            {"type": "a_block_from_the_future", "x": 1}, {"type": "text", "text": "hi"}]}}),
    );
    lines.push(json!({"type": "api-request-blob", "hash": "h", "message": {"role": "user", "content": "x"}}));
    lines.push(json!({"type": "queue-operation", "operation": "enqueue", "content": "x"}));
    let (_folder, updates) = replayed(&lines);
    let thread = thread_of(&updates);
    assert_eq!(thread.entries().len(), 2);
    assert_eq!(thread.entries()[1], Entry::Assistant("hi".into()));
}

#[test]
fn a_compaction_is_said_and_what_came_before_it_is_still_shown() {
    let mut lines = Lines::new();
    lines.prompt("before");
    let answered = lines.says(json!({"type": "text", "text": "old answer"}));
    lines.push(
        json!({"type": "system", "subtype": "compact_boundary", "parentUuid": null,
        "logicalParentUuid": answered, "content": "Conversation compacted",
        "compactMetadata": {"trigger": "auto", "preTokens": 100}}),
    );
    lines.push(json!({"type": "user", "isCompactSummary": true, "isVisibleInTranscriptOnly": true,
        "message": {"role": "user", "content": "This session is being continued from a previous conversation..."}}));
    lines.prompt("after");
    lines.says(json!({"type": "text", "text": "new answer"}));
    let (_folder, updates) = replayed(&lines);
    let thread = thread_of(&updates);
    let entries = thread.entries();
    assert_eq!(user_text(&entries[0]), "before");
    assert_eq!(entries[1], Entry::Assistant("old answer".into()));
    assert_eq!(
        entries[2],
        Entry::Notice(Level::Info, "Context compacted".into())
    );
    assert_eq!(user_text(&entries[3]), "after");
    assert_eq!(entries[4], Entry::Assistant("new answer".into()));
    assert_eq!(entries.len(), 5);
}

#[test]
fn a_subagent_s_own_record_is_shown_under_the_call_that_started_it() {
    let mut lines = Lines::new();
    lines.prompt("check it");
    lines.calls(
        "toolu_a",
        "Agent",
        json!({"description": "Check the tests", "prompt": "Run all the tests", "subagent_type": "Explore"}),
    );
    lines.answers(
        "toolu_a",
        json!([{"type": "text", "text": "All pass.\nagentId: abc (use SendMessage to go on)"}]),
        Some(
            json!({"status": "completed", "agentId": "abc", "prompt": "Run all the tests",
            "content": [{"type": "text", "text": "All pass."}], "totalTokens": 900,
            "totalToolUseCount": 1, "totalDurationMs": 10, "usage": {}}),
        ),
        false,
    );
    lines.says(json!({"type": "text", "text": "Done."}));

    let mut agent = Lines::new();
    agent.push(
        json!({"type": "user", "isSidechain": true, "agentId": "abc",
        "message": {"role": "user", "content": "Run all the tests"}}),
    );
    agent.says(json!({"type": "thinking", "thinking": "private", "signature": "s"}));
    agent.says(json!({"type": "text", "text": "Looking."}));
    agent.calls("toolu_s1", "Bash", json!({"command": "cargo test"}));
    agent.answers("toolu_s1", json!("ok"), None, false);
    agent.says(json!({"type": "text", "text": "They pass."}));

    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("s.jsonl");
    lines.write(&path);
    let agents = folder.path().join("s").join("subagents");
    agent.write(&agents.join("agent-abc.jsonl"));
    fs::write(
        agents.join("agent-abc.meta.json"),
        json!({"toolUseId": "toolu_a", "agentType": "Explore"}).to_string(),
    )
    .unwrap();

    let updates = replay(&path);
    assert!(updates.contains(&Update::AgentSaid {
        call: "toolu_a".into(),
        text: "Looking.".into()
    }));
    let thread = thread_of(&updates);
    let calls = tool_entries(&thread);
    assert_eq!(calls.len(), 2);
    let launch = calls[0];
    assert_eq!(launch.agent.as_deref(), Some("Explore"));
    assert_eq!(launch.status, ToolStatus::Done);
    assert_eq!(launch.content, [ToolContent::Text("All pass.".into())]);
    assert_eq!(launch.work.steps, 1);
    assert_eq!(launch.work.tokens, Some(900));
    let said: Vec<&str> = launch
        .work
        .said
        .iter()
        .map(|note| note.text.as_str())
        .collect();
    assert_eq!(said, ["Looking.", "They pass."]);
    assert_eq!(calls[1].parent.as_deref(), Some("toolu_a"));
    assert_eq!(calls[1].status, ToolStatus::Done);
    // The agent's work does not cut the answer that follows it in two.
    assert_eq!(
        thread.entries().last(),
        Some(&Entry::Assistant("Done.".into()))
    );
    assert!(
        !thread
            .entries()
            .iter()
            .any(|entry| matches!(entry, Entry::Thought(_))),
        "a subagent's reasoning is its own"
    );
}

#[test]
fn an_older_record_s_subagent_lines_are_found_among_its_own_by_the_prompt() {
    let mut lines = Lines::new();
    lines.prompt("look");
    let launched = lines.calls(
        "toolu_t",
        "Task",
        json!({"description": "Look around", "prompt": "Find the bug"}),
    );
    // The subagent's lines, in the same file, a tree of their own.
    lines.last = None;
    lines.push(json!({"type": "user", "isSidechain": true, "message": {"role": "user", "content": "Find the bug"}}));
    lines.push(json!({"type": "assistant", "isSidechain": true, "message": {"id": "sm1", "role": "assistant",
        "content": [{"type": "tool_use", "id": "toolu_g", "name": "Grep", "input": {"pattern": "bug"}}]}}));
    lines.push(
        json!({"type": "user", "isSidechain": true, "message": {"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "toolu_g", "content": "a.rs"}]}}),
    );
    lines.last = Some(launched);
    lines.answers("toolu_t", json!("It is in a.rs."), None, false);
    let (_folder, updates) = replayed(&lines);
    let thread = thread_of(&updates);
    let calls = tool_entries(&thread);
    assert_eq!(calls.len(), 2);
    assert_eq!(
        (calls[0].agent.as_deref(), calls[0].work.steps),
        (Some(""), 1)
    );
    assert_eq!(calls[1].parent.as_deref(), Some("toolu_t"));
    assert_eq!(calls[1].title, "grep \"bug\"");
    assert_eq!(calls[1].status, ToolStatus::Done);
    assert_eq!(
        thread.entries().len(),
        3,
        "the subagent's prompt is not the user's"
    );
}

#[test]
fn a_replay_leaves_no_turn_open_and_nothing_running() {
    let mut lines = Lines::new();
    lines.prompt("build it");
    lines.calls(
        "toolu_bg",
        "Bash",
        json!({"command": "cargo build", "run_in_background": true}),
    );
    lines.answers(
        "toolu_bg",
        json!("Command running in background with ID: b1"),
        Some(json!({"stdout": "", "stderr": "", "interrupted": false, "backgroundTaskId": "b1"})),
        false,
    );
    lines.calls(
        "toolu_ag",
        "Agent",
        json!({"description": "Watch", "prompt": "watch it"}),
    );
    lines.answers(
        "toolu_ag",
        json!("Async agent launched successfully."),
        Some(
            json!({"status": "async_launched", "isAsync": true, "agentId": "x",
            "description": "Watch", "prompt": "watch it", "outputFile": "/tmp/x"}),
        ),
        false,
    );
    // The command's end was told; the agent's never was.
    lines.push(json!({"type": "user", "isMeta": true, "message": {"role": "user", "content":
        "<task-notification>\n<task-id>b1</task-id>\n<tool-use-id>toolu_bg</tool-use-id>\n<status>completed</status>\n<summary>Build done</summary>\n</task-notification>"}}));
    // The session ended while this one ran.
    lines.calls("toolu_open", "Read", json!({"file_path": "/w/p/x.rs"}));
    let (_folder, updates) = replayed(&lines);
    assert!(
        !updates
            .iter()
            .any(|update| matches!(update, Update::TurnStarted | Update::TurnEnded(_)))
    );
    let ended = |id: &str| {
        updates
            .iter()
            .filter(|update| matches!(update, Update::ToolPatch(patch) if patch.id == id && patch.ended == Some(true)))
            .count()
    };
    assert_eq!((ended("toolu_bg"), ended("toolu_ag")), (1, 1));
    let thread = thread_of(&updates);
    assert_eq!(thread.state(), &ThreadState::Idle);
    let calls = tool_entries(&thread);
    assert!(
        calls
            .iter()
            .all(|call| !matches!(call.status, ToolStatus::Running | ToolStatus::Pending))
    );
    assert!(
        calls
            .iter()
            .all(|call| !call.work.background || call.work.ended)
    );
    assert_eq!(calls[2].status, ToolStatus::Cancelled);
    assert!(calls[0].work.background && calls[1].work.background);
}

#[test]
fn a_replay_of_a_conversation_still_going_on_leaves_open_what_is_open() {
    let mut lines = Lines::new();
    lines.prompt("build it");
    lines.calls(
        "toolu_ag",
        "Agent",
        json!({"description": "Watch", "prompt": "watch it"}),
    );
    lines.answers(
        "toolu_ag",
        json!("Async agent launched successfully."),
        Some(
            json!({"status": "async_launched", "isAsync": true, "agentId": "x",
            "description": "Watch", "prompt": "watch it", "outputFile": "/tmp/x"}),
        ),
        false,
    );
    lines.calls("toolu_done", "Read", json!({"file_path": "/w/p/a.rs"}));
    lines.answers("toolu_done", json!("1\tfn a() {}"), None, false);
    // Claude Code is at this one right now.
    lines.calls("toolu_open", "Read", json!({"file_path": "/w/p/x.rs"}));
    let folder = tempfile::tempdir().unwrap();
    let path = folder
        .path()
        .join("0b6f7f3e-2a51-4c0e-9d6b-6f1c2c0e8a11.jsonl");
    lines.write(&path);
    let thread = thread_of(&replay_live(&path));
    let calls = tool_entries(&thread);
    // The agent left in the background still runs, the call under way is
    // under way, and what is over is over.
    assert!(calls[0].work.background && !calls[0].work.ended);
    assert_eq!(calls[1].status, ToolStatus::Done);
    assert_eq!(calls[2].status, ToolStatus::Running);
    // The same record as history is final.
    let thread = thread_of(&replay(&path));
    let calls = tool_entries(&thread);
    assert!(calls[0].work.ended);
    assert_eq!(calls[2].status, ToolStatus::Cancelled);
}

#[test]
fn the_to_do_tool_and_the_task_tools_are_the_plan_and_no_calls() {
    let mut lines = Lines::new();
    lines.prompt("plan it");
    lines.calls(
        "toolu_todo",
        "TodoWrite",
        json!({"todos": [
        {"content": "Read", "status": "in_progress", "activeForm": "Reading"},
        {"content": "Write", "status": "pending", "activeForm": "Writing"}]}),
    );
    lines.answers(
        "toolu_todo",
        json!("Todos have been modified successfully."),
        None,
        false,
    );
    lines.says(json!({"type": "text", "text": "Planned."}));
    lines.calls(
        "toolu_c",
        "TaskCreate",
        json!({"subject": "Ship it", "description": "now"}),
    );
    lines.answers(
        "toolu_c",
        json!("Task #1 created successfully: Ship it"),
        Some(json!({"task": {"id": "1", "subject": "Ship it"}})),
        false,
    );
    let (_folder, updates) = replayed(&lines);
    let thread = thread_of(&updates);
    assert!(tool_entries(&thread).is_empty(), "{:?}", thread.entries());
    let plans: Vec<&Update> = updates
        .iter()
        .filter(|update| matches!(update, Update::Plan(_)))
        .collect();
    assert_eq!(plans.len(), 2);
    let Some(Entry::Plan(steps)) = thread
        .entries()
        .iter()
        .find(|entry| matches!(entry, Entry::Plan(_)))
    else {
        panic!("a plan");
    };
    assert_eq!(steps.len(), 1);
    assert_eq!(
        (steps[0].text.as_str(), steps[0].status),
        ("Ship it", PlanStatus::Pending)
    );
}

#[test]
fn a_message_taken_into_a_running_turn_is_the_user_s() {
    let mut lines = Lines::new();
    lines.prompt("start");
    lines.calls("toolu_1", "Bash", json!({"command": "make"}));
    lines.answers("toolu_1", json!("made"), None, false);
    let taken = lines.push(json!({"type": "attachment", "attachment": {
        "type": "queued_command", "prompt": "also check the docs", "commandMode": "prompt"}}));
    lines.says(json!({"type": "text", "text": "Checking the docs too."}));
    let (_folder, updates) = replayed(&lines);
    assert!(updates.contains(&Update::PromptId(taken)));
    let thread = thread_of(&updates);
    let entries = thread.entries();
    assert_eq!(user_text(&entries[2]), "also check the docs");
    assert_eq!(
        entries[3],
        Entry::Assistant("Checking the docs too.".into())
    );
}

#[test]
fn calls_made_at_once_whose_lines_hang_side_by_side_all_come_back_with_their_results() {
    let mut lines = Lines::new();
    lines.prompt("read both");
    let first = lines.push(json!({"type": "assistant", "message": {"id": "m1", "role": "assistant",
        "content": [{"type": "tool_use", "id": "toolu_a", "name": "Read", "input": {"file_path": "/w/p/a.rs"}}]}}));
    // The second call of the same answer and the first one's result both
    // hang off the first call's line; the conversation goes on from that
    // result, so the second call is on no path to the end.
    lines.push(json!({"type": "assistant", "message": {"id": "m1", "role": "assistant",
        "content": [{"type": "tool_use", "id": "toolu_b", "name": "Read", "input": {"file_path": "/w/p/b.rs"}}]}}));
    lines.answers("toolu_b", json!("b"), None, false);
    lines.last = Some(first);
    lines.answers("toolu_a", json!("a"), None, false);
    lines.says(json!({"type": "text", "text": "Both read."}));
    let (_folder, updates) = replayed(&lines);
    let thread = thread_of(&updates);
    let calls = tool_entries(&thread);
    let shown: Vec<(&str, ToolStatus)> = calls
        .iter()
        .map(|call| (call.title.as_str(), call.status))
        .collect();
    assert_eq!(
        shown,
        [
            ("Read a.rs", ToolStatus::Done),
            ("Read b.rs", ToolStatus::Done)
        ]
    );
    assert_eq!(
        thread.entries().last(),
        Some(&Entry::Assistant("Both read.".into()))
    );
}

#[test]
fn two_prompts_with_nothing_between_them_are_two_paragraphs() {
    let mut lines = Lines::new();
    let first = lines.prompt("one");
    lines.prompt("two");
    let (_folder, updates) = replayed(&lines);
    let thread = thread_of(&updates);
    assert_eq!(thread.entries().len(), 1);
    assert_eq!(user_text(&thread.entries()[0]), "one\n\ntwo");
    assert!(updates.contains(&Update::PromptId(first)));
}

#[test]
fn the_name_the_conversation_was_given_comes_with_it() {
    let mut lines = Lines::new();
    lines.prompt("hello");
    lines.push(json!({"type": "ai-title", "aiTitle": "Greeting"}));
    lines.push(json!({"type": "custom-title", "customTitle": "My chat"}));
    let (_folder, updates) = replayed(&lines);
    assert_eq!(updates.last(), Some(&Update::Title("My chat".into())));
}

#[test]
fn a_live_session_asks_a_record_for_its_name_without_reading_it_whole() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder
        .path()
        .join("0b6f7f3e-2a51-4c0e-9d6b-6f1c2c0e8a11.jsonl");
    // Nothing names it yet: a prompt is no name.
    let mut lines = Lines::new();
    lines.prompt("hello");
    lines.write(&path);
    assert_eq!(title(&path), None);
    // Claude Code's own name for it, then the one the user gave: theirs
    // stands, wherever in the record it is.
    lines.push(json!({"type": "ai-title", "aiTitle": "Greeting"}));
    lines.write(&path);
    assert_eq!(title(&path).as_deref(), Some("Greeting"));
    lines.push(json!({"type": "custom-title", "customTitle": "My chat"}));
    lines.push(json!({"type": "ai-title", "aiTitle": "A later guess"}));
    lines.write(&path);
    assert_eq!(title(&path).as_deref(), Some("My chat"));
    // A long record is read at its two ends only: the name near its end is
    // found past far more than one window of other lines.
    for _ in 0..600 {
        lines.says(json!({"type": "text", "text": "x".repeat(400)}));
    }
    lines.push(json!({"type": "custom-title", "customTitle": "Renamed since"}));
    lines.write(&path);
    assert!(std::fs::metadata(&path).unwrap().len() > 3 * WINDOW);
    assert_eq!(title(&path).as_deref(), Some("Renamed since"));
    // No record, no name.
    assert_eq!(title(&folder.path().join("gone.jsonl")), None);
}

/// A record under a made-up home, for the folder `cwd`, last changed
/// `age` ago.
fn keep(home: &Path, cwd: &str, id: &str, lines: &Lines, age: u64) -> PathBuf {
    let path = home
        .join("projects")
        .join(slug(Path::new(cwd)))
        .join(format!("{id}.jsonl"));
    lines.write(&path);
    let when = SystemTime::now() - Duration::from_secs(age);
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(when)
        .unwrap();
    path
}

const A: &str = "0b1f6f4e-0000-4000-8000-00000000000a";
const B: &str = "0b1f6f4e-0000-4000-8000-00000000000b";
const C: &str = "0b1f6f4e-0000-4000-8000-00000000000c";
const D: &str = "0b1f6f4e-0000-4000-8000-00000000000d";

#[test]
fn a_folder_s_conversations_are_listed_newest_first_by_what_they_are_called() {
    let home = tempfile::tempdir().unwrap();
    let cwd = "/w/p";

    let mut named = Lines::new();
    named.prompt("first prompt");
    named.push(json!({"type": "custom-title", "customTitle": "Named one"}));
    keep(home.path(), cwd, A, &named, 300);

    let mut asked = Lines::new();
    asked.push(json!({"type": "user", "isMeta": true, "message": {"role": "user", "content": "<local-command-caveat>x</local-command-caveat>"}}));
    asked.prompt("fix the\nbuild please");
    asked.says(json!({"type": "text", "text": "ok"}));
    keep(home.path(), cwd, B, &asked, 100);

    let mut last = Lines::new();
    last.prompt("an old prompt");
    last.push(json!({"type": "last-prompt", "lastPrompt": "the latest prompt", "leafUuid": "x"}));
    keep(home.path(), cwd, C, &last, 200);

    let listed = sessions(home.path(), Path::new(cwd));
    let shown: Vec<(&str, &str)> = listed
        .iter()
        .map(|session| (session.id.as_str(), session.title.as_str()))
        .collect();
    assert_eq!(
        shown,
        [
            (B, "fix the build please"),
            (C, "the latest prompt"),
            (A, "Named one")
        ]
    );
    assert_eq!(listed[0].cwd, PathBuf::from("/w/p"));
    assert!(listed[0].updated > listed[1].updated);

    // Another folder whose path makes the same name: its records are not
    // this one's.
    let elsewhere = tempfile::tempdir().unwrap();
    let mut other = Lines::new();
    other.push(json!({"type": "user", "cwd": elsewhere.path(),
        "message": {"role": "user", "content": "theirs"}}));
    keep(home.path(), cwd, D, &other, 1);
    assert_eq!(sessions(home.path(), Path::new(cwd)).len(), 3);
}

#[test]
fn side_records_empty_ones_and_ones_without_a_prompt_are_no_conversations() {
    let home = tempfile::tempdir().unwrap();
    let cwd = "/w/p";
    let mut side = Lines::new();
    side.push(json!({"type": "user", "isSidechain": true, "message": {"role": "user", "content": "task"}}));
    keep(home.path(), cwd, A, &side, 10);
    keep(home.path(), cwd, B, &Lines::new(), 10);
    let mut silent = Lines::new();
    silent
        .push(json!({"type": "user", "isMeta": true, "message": {"role": "user", "content": "x"}}));
    silent.push(json!({"type": "system", "subtype": "local_command", "content": "y"}));
    keep(home.path(), cwd, C, &silent, 10);
    let mut real = Lines::new();
    real.prompt("hello");
    keep(home.path(), cwd, D, &real, 10);
    // Neither is a file of the list's: not a session's id, and a folder.
    let folder = home.path().join("projects").join(slug(Path::new(cwd)));
    fs::write(folder.join("notes.jsonl"), real.text()).unwrap();
    fs::create_dir_all(folder.join("0b1f6f4e-0000-4000-8000-00000000000e.jsonl")).unwrap();

    let listed = sessions(home.path(), Path::new(cwd));
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (listed[0].id.as_str(), listed[0].title.as_str()),
        (D, "hello")
    );

    assert!(sessions(home.path(), Path::new("/nowhere/at/all")).is_empty());
    assert!(sessions(&home.path().join("missing"), Path::new(cwd)).is_empty());
}

#[test]
fn a_large_record_is_listed_by_its_head_and_its_tail() {
    let home = tempfile::tempdir().unwrap();
    let cwd = "/w/p";
    let mut lines = Lines::new();
    lines.prompt("the first prompt");
    for at in 0..400 {
        lines.says(json!({"type": "text", "text": format!("{at}: {}", "x".repeat(500))}));
    }
    lines.push(json!({"type": "ai-title", "aiTitle": "A long one"}));
    let path = keep(home.path(), cwd, A, &lines, 10);
    assert!(fs::metadata(&path).unwrap().len() > 2 * WINDOW);
    let listed = sessions(home.path(), Path::new(cwd));
    assert_eq!(listed[0].title, "A long one");

    // Without a name, a record whose head holds no prompt is named by its
    // last prompt.
    let mut late = Lines::new();
    late.push(
        json!({"type": "attachment", "attachment": {"type": "instructions",
        "files": [{"content": "y".repeat(70_000)}]}}),
    );
    late.prompt("buried");
    late.push(json!({"type": "last-prompt", "lastPrompt": "buried", "leafUuid": "x"}));
    keep(home.path(), cwd, B, &late, 5);
    let listed = sessions(home.path(), Path::new(cwd));
    assert_eq!(listed[0].title, "buried");
}

#[test]
fn a_conversation_carried_on_in_another_session_is_listed_once() {
    let home = tempfile::tempdir().unwrap();
    let cwd = "/w/p";
    let mut old = Lines::new();
    old.prompt("old");
    old.push(json!({"type": "continued-in", "continuedInSessionId": B}));
    keep(home.path(), cwd, A, &old, 20);
    let mut new = Lines::new();
    new.prompt("new");
    keep(home.path(), cwd, B, &new, 10);
    let listed = sessions(home.path(), Path::new(cwd));
    let ids: Vec<&str> = listed.iter().map(|session| session.id.as_str()).collect();
    assert_eq!(ids, [B]);
}

#[test]
fn a_conversation_s_record_is_found_by_its_folder_and_id() {
    let home = tempfile::tempdir().unwrap();
    let mut lines = Lines::new();
    lines.prompt("hello");
    let path = keep(home.path(), "/w/p", A, &lines, 1);
    assert_eq!(
        record(home.path(), Path::new("/w/p"), A),
        Some(path.clone())
    );
    assert_eq!(record(home.path(), Path::new("/w/p/"), A), Some(path));
    assert_eq!(record(home.path(), Path::new("/w/p"), B), None);
    assert_eq!(record(home.path(), Path::new("/w/q"), A), None);
    for trick in ["../x", "", ".hidden", "a/b"] {
        assert_eq!(
            record(home.path(), Path::new("/w/p"), trick),
            None,
            "{trick}"
        );
    }
}

#[test]
fn a_folder_s_records_are_kept_under_its_path_with_dashes() {
    assert_eq!(
        slug(Path::new("/Users/me/my.project")),
        "-Users-me-my-project"
    );
    assert_eq!(slug(Path::new("/w/naïve")), "-w-na-ve");
    // A character JavaScript counts as two is two dashes.
    assert_eq!(slug(Path::new("/w/🦀")), "-w---");

    // A path too long for a name: the folder starts with its first part.
    let home = tempfile::tempdir().unwrap();
    let long = format!("/w/{}", "deep/".repeat(60));
    let cut = &slug(Path::new(&long))[..SLUG_LIMIT];
    let folder = home.path().join("projects").join(format!("{cut}-1x9z"));
    let mut lines = Lines::new();
    lines.push(json!({"type": "user", "cwd": long.trim_end_matches('/'),
        "message": {"role": "user", "content": "deep"}}));
    lines.write(&folder.join(format!("{A}.jsonl")));
    let listed = sessions(home.path(), Path::new(&long));
    assert_eq!(listed.len(), 1);
    assert!(record(home.path(), Path::new(&long), A).is_some());
}
