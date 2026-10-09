---
name: kou-canvas
description: How to work on a kou-conveyor canvas with the Canvas tools — ask another node and have its answer in one call, create terminals and agents beside you, wire them output to input, hand them work, wait for and read their answers — with recipes for a worker in a worktree, a fan-out to several workers and back, and a review loop. Use when you are a node on a canvas and the task needs other nodes.
---

# Working on a canvas

A canvas is a board of nodes wired output to input:

| Node | What it is | Ports |
|---|---|---|
| terminal (`shell`, `command`) | a shell, or one running a command line | in → out, exit |
| terminal (`claude-code`, `codex`, `opencode`) | a coding agent's TUI in a shell | in → out |
| agent | a kou agent like you, with a session of its own | in → out |
| source | events: a timer, a button, files that change, a webhook, a plugin's | outputs only |
| note | text on the board | none |

A shell that runs a coding agent (`claude`, `codex`, `gemini`…) the user started there counts as that agent: it takes prompts, and its screen once it falls quiet is its answer.

What a node puts out goes along its connections to other nodes' inputs, as messages:

- a shell's output is the output of each command it ran (between its prompts);
- an agent's output is the answer it ends a turn with — when the turn's message came from a node; what the user asks it, it answers to the user alone (unless its config's `output` is `all`; `explicit` puts out only what it emits);
- a source's outputs are its events.

A message is delivered when its target is idle, by default: typed into a terminal and Enter pressed, or given to an agent as a prompt that starts with `[canvas] from «Title» (id):`. A node that messaged another straight (CanvasSend) has the answer back: as the call's result when it waits, else as a prompt that starts with `[canvas] reply from «Title» (id):`. What an agent answers to a reply stays with it: the exchange ends there.

## The tools

- **CanvasSend** `{node, text, wait?, timeout_s?, when?, keys?}` — message a node, named by ID or title: prompt an agent, or type into a terminal. With `wait: true` the call waits for the node's answer and returns it; without, an agent's answer comes to you later as a reply. `when: "now"` gives it at once; otherwise it waits until the node is idle.
- **CanvasView** — the nodes (ID, kind, title, status, place, ports, worktree), the connections, free places beside you, and your access. Call it before you change anything.
- **CanvasSpawn** `{preset, title, worktree?, prompt?, command?, near?, side?, connect?: {from?, to?}}` — create a node beside yours (or `near` another, on `side`), wire it, send it a first prompt. It answers the node's ID.
- **CanvasConnect** `{from: "node:port", to: "node:port", template?, mode?}` — wire an output to an input; ports default to `out` and `in`. `mode: "approve"` holds each message for the user; a connection that closes a loop of agents starts that way.
- **CanvasDisconnect** `{edge}` or `{from, to}`, **CanvasMove** `{node, x, y, w, h}`, **CanvasRemove** `{node}`.
- **CanvasRead** `{node, what?, wait?, after?, timeout_s?}` — read a terminal's tail or screen, or a node's latest output; with `wait` (`idle`, `output`, `exit`) it waits first.
- **CanvasEmit** `{text, port?, data?}` — put something out of your own node now; your answer to a node's message needs none.

Nodes are named by ID (`n_…`) — or, to send, read and wait, by a title no other node has; `self` is you.

## Recipes

### Asking another node

`CanvasSend {"node": "reviewer", "text": "Is the change in auth/session.go safe to merge? Answer yes or what to fix.", "wait": true}` — one call: its answer is the result. Two nodes must not wait for each other: the canvas refuses a wait for a node that waits for you.

### A worker in a worktree

1. `CanvasView` — see where there is room.
2. `CanvasSpawn {"preset": "codex", "title": "fix-login", "worktree": "fix-login", "prompt": "Fix the login timeout in auth/session.go (issue #42). Run the tests, commit on your branch, and answer with a summary of the change."}`
3. `CanvasRead {"node": "<id>", "wait": "output", "timeout_s": 1800}` — its answer at the end of its turn. Later tasks: `CanvasSend {"node": "fix-login", "text": "…", "wait": true, "timeout_s": 1800}`.

The worker's branch is `kou/fix-login`; the user merges it.

### Fan-out and back

To spread independent tasks over workers and gather what they say:

1. Spawn a collector, or use yourself: the workers' outputs go to it.
2. For each task: `CanvasSpawn {"preset": "claude-code", "title": "mig-a", "worktree": "mig-a", "prompt": "…", "connect": {"to": "self"}}` — with `connect.to: "self"`, each worker's answer comes back to you as a message.
3. Wait for each: `CanvasRead {"node": "<id>", "wait": "idle"}`, or just end your turn: the answers arrive as prompts.
4. When all are in, write the summary.

### A review loop

1. A coder and a reviewer: `CanvasSpawn {"preset": "claude-code", "title": "coder", "worktree": "feature"}` and `CanvasSpawn {"preset": "agent", "title": "reviewer", "config": {"instructions": "Review what the coder sends. Answer LGTM when it is good, else what to change."}}`.
2. `CanvasConnect {"from": "<coder>", "to": "<reviewer>"}` and back `CanvasConnect {"from": "<reviewer>", "to": "<coder>", "mode": "approve"}` — the user approves each round.
3. `CanvasSend {"node": "<coder>", "text": "<the task>"}` starts it.

### Driving a shell

`CanvasSpawn {"preset": "command", "title": "tests", "command": "go test ./...", "near": "self", "side": "below"}`, then `CanvasRead {"node": "<id>", "what": "output", "wait": "exit", "timeout_s": 900}` reads the result. A command in a shell: `CanvasSend {"node": "<id>", "text": "go vet ./...", "wait": true}` answers what it printed. For a long-running server: `CanvasSpawn {"preset": "shell", "title": "dev server"}`, `CanvasSend {"node": "<id>", "text": "npm run dev"}`, `CanvasRead {"node": "<id>", "what": "tail", "lines": 40}`; stop it with `CanvasSend {"node": "<id>", "keys": ["C-c"], "when": "now"}`.

## Limits

The canvas bounds what agents do: how many nodes and terminals it holds, how fast agents create nodes, and how deep nodes made by agents make more. A message that went around a loop too many times is dropped, and the user is told. Nodes you make get `talk` access unless you give them `build` (and you have it). When an answer is too long, the message carries its first part and the path of a file with all of it.

## From a terminal

`kou-canvas` is the same on the command line, in any program of a node: `kou-canvas send <node> "text" --wait`, `kou-canvas view`, `kou-canvas spawn codex --title worker --worktree fix-a --right-of self --prompt "…"`, `kou-canvas read <node> --output --wait exit`, `kou-canvas keys <node> C-c`. `kou-canvas --help` lists the rest.
