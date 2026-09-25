# kou-conveyor-runner

Run an AI agent from a prompt or JSON request. It writes events to stdout as
JSONL and exits when the task finishes.

Install with Go 1.27+:

```sh
go install github.com/gfhfyjbr/kou-conveyor/cmd/kou-conveyor-runner@latest
```

Set an OpenAI API key and run a prompt in the current directory:

```sh
export OPENAI_API_KEY="..."
kou-conveyor-runner -p 'Inspect this project and explain how to run its tests.'
```

Or run from source at the repository root:

```sh
go run ./cmd/kou-conveyor-runner -p 'Inspect this project and explain how to run its tests.'
```

Choose a workspace and save the output:

```sh
kou-conveyor-runner -workspace ./my-project -p 'Summarize this project.' > run.jsonl
```

You can also pass a JSON request as an argument or through stdin:

```sh
kou-conveyor-runner '{"prompt":"Summarize this project."}'
kou-conveyor-runner < request.json
```

With `-steer`, stdin stays open after the request (with `-p` or a request
argument, stdin carries only messages): each further JSON object, one per
line, is a message for the agent while it works: `{"content": string}`, with
an optional `"message_id"` (a UUID) it is recorded under.

```sh
{ echo '{"prompt":"Run the tests and fix what fails."}'; sleep 30
  echo '{"content":"Use pnpm, not npm."}'; } | kou-conveyor-runner -steer
```

Such a message never cuts a response short. It waits for the model to
finish the response it is writing and for the tool calls of that response to
finish, and goes out with their results (or with any request that goes out
before); a message that arrives with nothing running starts a turn at once.
Calls that outlast `-steer-wait` (2 minutes by default, `0` for no limit),
such as a server that never exits, do not hold it: it goes out then, and
those calls show as still running. The session records it when it goes out, after
those results, with `"Delivery":"after_tools"`, so the transcript shows it
where the model read it. A message the run ends before is not recorded: the
cockpits send it again as the next prompt. The cockpits use `-steer` to force
messages into a running agent (see the repository's
[README](../../README.md#queue)).

A request's `"model"` chooses the model of its run. A session may run each
prompt with another model, of another provider too: every turn records the
model its request went to, and what a provider attached to its model's
output for that model alone — encrypted or signed reasoning, the IDs it
gives items — is replayed to that model only.

OpenAI is the default provider. Set `KOU_CONVEYOR_LLM_PROVIDER` to `openai`,
`anthropic`, `openai-codex`, `openrouter`, `fireworks`, or `ollama`, and
`KOU_CONVEYOR_LLM_MODEL` to choose a model. `KOU_CONVEYOR_LLM_BASE_URL`
points a provider at another endpoint that speaks the same API, and
`KOU_CONVEYOR_LLM_API_KEY` overrides the provider's own key variable. The
runner removes `KOU_CONVEYOR_LLM_API_KEY` from its environment once it has
the key, so the commands the agent runs cannot read it.

### Images

A message can bring images, which the model sees right after their labels,
beside the message's text: `images` lists `{"label": "[Image 1]",
"media_type": "image/png", "data": "<base64>"}`, or `{"url": "https://…"}`
in place of the bytes. PNG, JPEG, GIF and WebP go, up to 20 a message and
20 MB each; a label defaults to `[Image n]`. Messages sent with `-steer`
take them too, and the session records them with the message.

```sh
printf '{"messages":[{"content":"What is off on [Image 1]?","images":[{"media_type":"image/png","data":"%s"}]}]}' \
  "$(base64 < shot.png | tr -d '\n')" | kou-conveyor-runner
```

### Anthropic Messages API

```sh
export ANTHROPIC_API_KEY="..."
KOU_CONVEYOR_LLM_PROVIDER=anthropic kou-conveyor-runner -p 'Summarize this project.'
```

The default model is `claude-opus-5`. Thinking levels become adaptive
thinking with an effort level (`xhigh` falls back to `high` on the 4.6
generation, and older models get a thinking budget instead), thinking
summaries appear in the timeline, and the system prompt and conversation are
cached between turns. Responses are streamed and capped at 64,000 tokens, or
128,000 at `xhigh` and `max`; set `KOU_CONVEYOR_LLM_MAX_TOKENS` to change
that. Transient API errors, overloads and stalled streams are retried.

On Anthropic's API, Claude Opus 5 and Fable requests opt into server-side
refusal fallbacks: if a safety classifier declines a request, Anthropic
re-runs it on its recommended fallback model instead of returning a refusal.
A refusal that still happens ends the turn without acting on partial output.

A base URL other than Anthropic's, for example a gateway, gets the key both
as `x-api-key` and as a bearer token. Models whose IDs do not name a Claude
model get a plain request without thinking or cache controls.

### Compaction

Before a turn whose request would come within 33,000 tokens of the model's
context window, the runner compacts the conversation: the model writes a
summary of it in a turn of the `compaction` type, the summary replaces the
conversation, and the turn that was due continues from it. The model is asked
to think the conversation through in an `<analysis>` block first and then
write a `<summary>` in nine sections (the requests and intent, key concepts,
files and code, errors and fixes, problem solving, all the user's messages,
pending tasks, current work, and the next step, with quotes); only the
summary is kept. Unanswered prompts stay as they were after the summary, the
user's earlier prompts are kept word for word as far as 40,000 characters
allow, and tool calls still running are listed; a result that arrives after
its call was summarized reaches the model as a message. The message with the
summary names the session file, whose JSON lines keep the full conversation
for details the summary left out. The session file records the compaction,
so a resumed or branched session starts from the summary.

The runner knows the context windows of the OpenAI and Claude models;
`KOU_CONVEYOR_CONTEXT_WINDOW` sets it for others (default `128000`, and
`200k` or `1m` work too). `KOU_CONVEYOR_AUTO_COMPACT` sets when to compact:
a number of tokens (`150000`, `150k`), a share of the window (`80%`), or
`off`. By default it is 33,000 tokens short of the window (167k of 200k, 967k
of 1M), or a quarter short of a window under 132,000 tokens (96k of 128k).
The window counts the request's input; the size is the provider's count for
the latest turn plus an estimate of what came after it.

Automatic compaction backs off where it cannot help. After three compactions
in a row that produced no summary it stops until a compaction succeeds (a
requested one still runs). When the conversation
reaches the threshold again within three turns of a compaction for the third
time in a row, which a tool output or a file too large for the window does,
the request goes out as it is instead of being compacted once more, until
three turns have passed since the last compaction. A request that the
unanswered input alone makes large is not compacted either: the summary
would not replace it.

A request with `"compact": true` compacts an existing session on demand, and
`compact_instructions` tell the summary what to focus on:

```sh
kou-conveyor-runner '{"session_id":"…","compact":true,"compact_instructions":"the API changes"}'
```

Messages in the same request run after the compaction.

A conversation that has already outgrown the window, after a batch of large
tool results or in a session from before compaction, is cut to fit for its
compaction: the largest tool results are shortened first, then the oldest
items are left out, keeping up to 20,000 tokens of room for the summary; the
model is asked to say in the summary that the earliest part is missing.

The size of a request is an estimate, and the provider may find a request
too large for the window after all. The runner then compacts the
conversation at once, if automatic compaction may run, and the turn follows
the summary. A compaction request the provider finds too large is cut down
further, by as much as the provider's count exceeded the estimate or by a
fifth when it gives no count, and sent again in the same turn, up to three
times.

Claude models count `max_tokens` against the context window, so when a long
conversation leaves less room than the usual maximum, the request asks for a
shorter response instead of failing.

### Plugins

The runner loads [plugins](../../docs/plugins.md): the built-in `core`
plugin, which brings `Bash`, `ViewImage` and `SkillUse`, the user's in the
configuration directory beside `KOU_CONVEYOR_CONFIG`, and the workspace's in
`.harness/plugins` when the workspace is trusted (or
`KOU_CONVEYOR_TRUST_WORKSPACE_PLUGINS=1`). Their tools, skills and
instructions join the run; `KOU_CONVEYOR_DISABLED_PLUGINS` turns plugins
off by name, and `-list-plugins` prints what a run would find.

A run follows its plugins while it goes on: before every turn it looks at
where they come from, and a tool added, changed or removed, other
instructions or skills, or a plugin turned on or off reach the agent in its
next request (`plugin>` on stderr says so). Calls already made bring their
results back. `KOU_CONVEYOR_WATCH_PLUGINS=0` keeps the plugins a run
started with.

Run `kou-conveyor-runner -h` for options and the JSON request fields.

## Docker

The `gfhfyjbr/kou-conveyor` image supports Linux on AMD64 and ARM64. Run it
with a project mounted as the workspace:

```sh
docker run --rm -i --user "$(id -u):$(id -g)" \
  -e OPENAI_API_KEY -v "$PWD:/workspace" \
  gfhfyjbr/kou-conveyor:latest -p 'Summarize this project.'
```

Each release also publishes its Git tag (for example, `v0.1.0`) for version pinning.
