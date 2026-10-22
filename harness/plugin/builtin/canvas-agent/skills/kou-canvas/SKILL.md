---
name: kou-canvas
description: How to work on a kou-conveyor canvas with the Canvas tools — ask another node and have its answer in one call, create kou agents, terminals and notes beside you, wire them output to input, hand them work, wait for and read their answers and transcripts — with recipes for a worker in a worktree, a fan-out to several workers and back, a review loop and a foreman's team. Use when you are a node on a canvas and the task needs other nodes.
---

# Working on a canvas

A canvas is a board of nodes wired output to input:

| Node | What it is | Ports |
|---|---|---|
| agent (the default) | a kou agent like you, with a session of its own | in → out |
| terminal (`shell`, `command`) | a shell, or one running a command line | in → out, exit |
| terminal (`claude-code`, `codex`, `opencode`) | a coding agent's TUI in a shell | in → out |
| source | events: a timer, a button, files that change, a webhook, a plugin's | outputs only |
| note | text on the board | none |

Make the nodes you create kou agents unless the user asks for another kind: the canvas prompts them directly, hands them messages mid-turn, and reads their transcripts. A Claude Code terminal in a new worktree first asks whether to trust the folder; the node waits for the user to answer it, and nothing is typed into it meanwhile.

A shell that runs a coding agent (`claude`, `codex`, `gemini`…) the user started there counts as that agent: it takes prompts, and its screen once it falls quiet is its answer.

What a node puts out goes along its connections to other nodes' inputs, as messages:

- a shell's output is the output of each command it ran (between its prompts);
- an agent's output is the answer it ends a turn with — when the turn's message came from a node; what the user asks it, it answers to the user alone (unless its config's `output` is `all`; `explicit` puts out only what it emits);
- a source's outputs are its events.

A message to a kou agent reaches it at once: between its tool calls when it works (it reads it before its next step), as a prompt when it is idle. A message to a terminal waits until the terminal is idle, then is typed in and Enter pressed. A connection with `deliver: "queue"` makes a kou agent wait until it is idle too; `CanvasSend` with `when: "now"` types into a busy terminal. A message comes as a prompt that starts with `[canvas] from «Title» (id):`. A node that messaged another straight (CanvasSend) has the answer back: as the call's result when it waits, else as a prompt that starts with `[canvas] reply from «Title» (id):` — but a foreman, and an agent whose config's `reply` is `off`, answer nodes only with CanvasSend: what they end a turn with goes to the user. What an agent answers to a reply stays with it: the exchange ends there.

## The tools

- **CanvasSend** `{node, text, wait?, timeout_s?, when?, keys?, replace?}` — message a node, named by ID or title: prompt an agent, or type into a terminal. With `wait: true` the call waits for the node's answer and returns it; without, an agent's answer comes to you later as a reply. `when: "now"` gives it at once even to a busy terminal, or to an agent whose queue is paused. `replace: true` puts your last prompt to a kou agent otherwise — one its model refused, say — once its run is over: its session goes back to before that prompt, and the new text runs in its place; no need to remove the agent and start over.
- **CanvasView** — the nodes (ID, kind, title, status, place, ports, worktree), the connections, free places beside you, and your access. A busy agent that has been quiet for long says so. Call it before you change anything.
- **CanvasSpawn** `{preset?, title, worktree?, include?, link?, cwd?, prompt?, config?, command?, near?, side?, connect?: {from?, to?}}` — create a node beside yours (or `near` another, on `side`), wire it, send it a first prompt. An agent you spawn (a kou agent, `claude-code`, `codex`, `opencode`) is wired to you — its output to your input — so what it answers comes back to you as a message; `connect` wires it otherwise, `connect: {}` not at all. With no preset it is a kou agent; `config` gives it `instructions` (its role), `effort` (`low`…`max`), `model`, `output`, `reply`. It answers the node's ID. A worktree has what git tracks and the submodules of the user's checkout; `include` copies in what else a build needs that git does not carry (`["vendor", "tools/oracle"]`), `link` links it instead (big, read-only things). `cwd` is the folder it works in, when not the workspace: absolute, or relative to the workspace.
- **CanvasConnect** `{from: "node:port", to: "node:port", template?, mode?, deliver?}` — wire an output to an input; ports default to `out` and `in`. `mode: "approve"` holds each message for the user; a connection that closes a loop of agents starts that way. `deliver: "queue"` holds messages until the target is idle; by default a kou agent gets them at once.
- **CanvasDisconnect** `{edge}` or `{from, to}`, **CanvasMove** `{node, x, y, w, h}`, **CanvasRemove** `{node}`.
- **CanvasRead** `{node, what?, wait?, after?, timeout_s?}` — read a terminal's tail or screen, or a node's latest output; with `wait` (`idle`, `output`, `exit`) it waits first.
- **CanvasTranscript** `{node, tail?, after?, limit?, kinds?, tools?, since?, until?, message_limit?, output_limit?, include?, format?}` — read a node's raw structured transcript: its prompts, reasoning, tool calls with their outcomes, answers. `tail: N` gives the last N entries and moves no cursor; `after: "last"` gives what is new since your previous read with the same `kinds` and `tools`; `after: <next>` reads on from a cursor. An answer holds about 30 KB at most: when it says `more`, read on with its `next`. Kinds: `user`, `peer` (messages from nodes), `assistant`, `reasoning`, `tool`, `notice`, `error`, `compaction`, `heartbeat`. `format: "json"` answers JSON instead of lines of text. Include `notes` or `summaries` only when needed.
- **CanvasSearch** `{node?, nodes?, q, limit?}` — search a persisted transcript with a case-insensitive regular expression; use `nodes: "children"` to search all children. Use this to locate evidence, not to poll.
- **CanvasWait** `{nodes?, until?, after?, timeout_s?}` — wait for any of several nodes (your children — the nodes you made and those you are wired to — when none are named): `until` `answer` (the default), `idle` (all of them), `exit` or `any`. It answers which node it was and the state of each, with the `next` cursor to pass as `after`.
- **CanvasEmit** `{text, port?, data?}` — put something out of your own node now; your answer to a node's message needs none.

Nodes are named by ID (`n_…`) — or, to send, read and wait, by a title no other node has; `self` is you.

## Recipes

### Diagnosing a stalled child

1. `CanvasView` or `CanvasWait` checks which children are busy, and which have been quiet for long; do not start a shell loop or `sleep`.
2. `CanvasTranscript {"node": "worker", "tail": 30}` shows what it did last. Later, `{"node": "worker", "after": "last"}` reads only what changed since then. Add `kinds: ["reasoning", "tool"]` or `tools: ["Read", "Bash"]` to focus it; tool outputs are bounded and carry the path of the full output when there is one.
3. `CanvasSearch {"node": "worker", "q": "gfxFont"}` locates an earlier read or error. Use `nodes: "children"` from a foreman to search all children.
4. Read the transcript with the tool, not with scripts over the session files: it is bounded, and its cursors keep your place.

### Asking another node

`CanvasSend {"node": "reviewer", "text": "Is the change in auth/session.go safe to merge? Answer yes or what to fix.", "wait": true}` — one call: its answer is the result. Two nodes must not wait for each other: the canvas refuses a wait for a node that waits for you.

### A worker in a worktree

1. `CanvasView` — see where there is room.
2. `CanvasSpawn {"title": "fix-login", "worktree": "fix-login", "config": {"effort": "high"}, "prompt": "Fix the login timeout in auth/session.go (issue #42). Run the tests, commit on your branch, and answer with a summary of the change."}` — a kou agent.
3. `CanvasRead {"node": "<id>", "wait": "output", "timeout_s": 1800}` — its answer at the end of its turn. Later tasks: `CanvasSend {"node": "fix-login", "text": "…", "wait": true, "timeout_s": 1800}`.

The worker's branch is `kou/<its worktree>`; the user merges it.

### Fan-out and back

To spread independent tasks over workers and gather what they say:

1. Spawn a collector, or use yourself: the workers' outputs go to it.
2. For each task: `CanvasSpawn {"title": "mig-a", "worktree": "mig-a", "prompt": "…"}` — wired to you, as an agent you spawn is, so each worker's answer comes back to you as a message. For a collector other than you: `"connect": {"to": "<collector>"}`.
3. End your turn: the answers arrive as prompts, one by one. To wait within a turn instead, `CanvasWait {}` returns at the first answer of any of your children, with the state of each; pass its `next` as `after` to the next `CanvasWait`, so that an answer given in between is not missed. `CanvasWait {"until": "idle"}` returns once they are all idle.
4. When all are in, write the summary. Whenever you are woken, look at every worker, not just the one that answered: one may be stuck without a word.

### A review loop

1. A coder and a reviewer, wired to each other only (`connect: {}`: else each answers you too): `CanvasSpawn {"title": "coder", "worktree": "feature", "connect": {}}` and `CanvasSpawn {"title": "reviewer", "connect": {}, "config": {"instructions": "Review what the coder sends. Answer LGTM when it is good, else what to change."}}`.
2. `CanvasConnect {"from": "<coder>", "to": "<reviewer>"}` and back `CanvasConnect {"from": "<reviewer>", "to": "<coder>", "mode": "approve"}` — the user approves each round.
3. `CanvasSend {"node": "<coder>", "text": "<the task>"}` starts it.

### A foreman's team

A foreman hands out work and decides; the nodes do the work:

1. One coder per independent piece of work, each in its worktree, wired to you (as CanvasSpawn wires an agent): `CanvasSpawn {"title": "coder: parser", "worktree": "parser", "config": {"effort": "high", "instructions": "…"}, "prompt": "…"}`.
2. A reviewer that did not write the code, for each change that is done: `CanvasSpawn {"title": "reviewer", "config": {"instructions": "Review the branch you are given: correctness, tests, risks. Answer LGTM or what to fix."}}`, then `CanvasSend {"node": "reviewer", "text": "Review kou/parser against main."}`.
3. A researcher for what needs reading, searching or measuring — docs, upstream changes, a bisect — instead of doing it yourself.
4. Answer each child with CanvasSend as soon as you have decided; what you end your turn with is your report to the user. When something you said no longer holds, tell the child at once.

### Driving a shell

`CanvasSpawn {"preset": "command", "title": "tests", "command": "go test ./...", "near": "self", "side": "below"}`, then `CanvasRead {"node": "<id>", "what": "output", "wait": "exit", "timeout_s": 900}` reads the result. A command in a shell: `CanvasSend {"node": "<id>", "text": "go vet ./...", "wait": true}` answers what it printed. For a long-running server: `CanvasSpawn {"preset": "shell", "title": "dev server"}`, `CanvasSend {"node": "<id>", "text": "npm run dev"}`, `CanvasRead {"node": "<id>", "what": "tail", "lines": 40}`; stop it with `CanvasSend {"node": "<id>", "keys": ["C-c"], "when": "now"}`.

## Limits

The canvas bounds what agents do: how many nodes and terminals it holds, how fast agents create nodes, and how deep nodes made by agents make more. A message that went around a loop too many times is dropped, and the user is told; each message an agent sends with CanvasSend starts the count anew. Nodes you make get `talk` access unless you give them `build` (and you have it). When an answer is too long, the message carries its first part and the path of a file with all of it.

## From a terminal

`kou-canvas` is the same on the command line, in any program of a node: `kou-canvas send <node> "text" --wait`, `kou-canvas view`, `kou-canvas spawn --title worker --worktree fix-a --right-of self --prompt "…"` (a kou agent, wired to answer you — `--no-connect` wires nothing; name a preset first for another kind, as in `kou-canvas spawn shell`), `kou-canvas read <node> --output --wait exit`, `kou-canvas transcript <node> --tail 30`, `kou-canvas keys <node> C-c`. `kou-canvas --help` lists the rest.
