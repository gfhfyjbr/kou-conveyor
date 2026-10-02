# kou-conveyor

An async-first agent harness.

- [harness/](harness/) — the library.
- [cmd/](cmd/) — executables that use the library.
- [benchmarks/](benchmarks/) — benchmark runners.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/gfhfyjbr/kou-conveyor/main/install.sh | sh
```

This installs `kou-conveyor-runner`, the terminal cockpit `kou-conveyor-tui`
and the browser cockpit `kou-conveyor-web` into `~/.local/bin`. Run
`sh install.sh` from a checkout to build that checkout instead. The cockpits
are built with Go 1.21 or newer, which fetches the Go release the project
needs; without Go the script installs the runner from the latest release.
`sh install.sh --help` lists the options (`--dir`, `--version`, `--method`,
`--uninstall`).

The runner talks to the OpenAI Responses API by default and to the Anthropic
Messages API with `KOU_CONVEYOR_LLM_PROVIDER=anthropic`; see
[`cmd/kou-conveyor-runner`](cmd/kou-conveyor-runner/README.md). Both cockpits
can also keep the connection themselves (below).

## Glossary

- **Input**: an event with a caller-supplied globally unique ID that remains
  stable across redeliveries. External input that arrives while the agent
  works either starts a turn at once, cutting short a response in progress, or
  is **delivered after tools**: it waits for the response and for the tool
  calls it makes, and goes out with their results.
- **Inbox**: session-scoped, in-memory deduplication of external, control, and
  crash inputs.
- **Session**: append-only persisted history that can be forked.
- **LLM turn**: the coordinator-managed sequence around one logical LLM request.
- **Compaction**: a turn that replaces the conversation with a summary the model
  writes of it, freeing the context it took; the session goes on from the
  summary.
- **Tool**: a capability described by a schema and bound to a translator.
- **Tool call**: a model-produced request to use a tool.
- **Tool translator**: validates a tool call and translates it into one or more
  operations. It runs synchronously on the coordinator's event loop and must not
  perform I/O or suspend the loop.
- **Tool call status**: the translation outcome: a validation error or references
  to submitted operations. Operation execution state is tracked separately;
  the translator formats these into a model-facing result.
- **Operation**: a serializable description of work produced by a tool translator
  for asynchronous execution. Implementations are encouraged to use the available
  [primitives](harness/primitives/).

## Components

| Component | Responsibility |
| --- | --- |
| Session inbox | Volatile, session-scoped input idempotency. |
| Coordinator | Persist accepted inputs, run LLM turns, compact the conversation before it outgrows the model's context window, resolve tool translators through the registry, and dispatch committed operations. |
| Session store | Persist canonical session history and operation state; support recovery and forks; atomically record tool-call status with operations. |
| Context builder | Statefully assemble model input in memory. Return the model input together with a record of anything omitted, truncated, or compacted. Perform no I/O and accept no persistence dependencies. |
| LLM Adapter | Send prepared model input to a provider and return a normalized completed response. Own authentication, cancellation, and provider errors. |
| Tool registry | Own the fixed Bash, ViewImage, and skill-use definitions and their translators; expose the host-selected set. |
| Tool translator | Validate a tool call and produce its status and operations. Format a recorded call status and prepared operation output into model results. Perform no I/O. |
| Operation manager | Actor runtime for durable operations. The local implementation is swappable. |

## Extending the harness

Harness components are composable, and alternative implementations of their interfaces are encouraged.

We intend to preserve these invariants:

- Session-store items are serializable, and the storage format is versioned.
- We'll do our best to maintain backwards compatibility for sessions.
  An unsupported session version will always cause an explicit error on resume.
- Operations are versioned and always serializable.

For example, a proxy operations manager can send serialized operations to a
local operations manager running in a process inside a remote sandbox, allowing
tools to execute there.

## Plugins

Everything the project adds is a plugin: a directory with a `plugin.json`
that gives the agent tools (commands that take their arguments as JSON on
standard input and print the result), skills and instructions, and gives
the cockpits slash commands and a script and style sheet. The harness's own
tools are the built-in `core` plugin, and the browser cockpit is nothing
but plugins: its page is a small plugin host, and the layout, sessions,
transcript, composer, models, accounts, inspector, palette and theme are
built-in plugins on the same API as yours. A plugin can add to any part of
the interface, take any part's place — a button, a panel, how an entry is
drawn, a whole built-in plugin — add views and panels of its own, and hook
into every prompt on its way to the agent.

Changes show at once, with nothing reloaded or restarted: a plugin whose
files change is loaded anew in open pages, the running agent takes up
changed tools, instructions and skills at its next turn, and the terminal
cockpit its commands. Run from a checkout, the browser cockpit serves its
own built-in plugins from it, live, and builds itself anew when its Go code
changes, restarting in place with the pages left open. Plugins live in the user's
configuration directory and in the workspace's `.harness/plugins` — a
project's plugins come from there alone, load only in that workspace, and
run only once the workspace is trusted. See
[docs/plugins.md](docs/plugins.md),
[examples/plugins/git-glance](examples/plugins/git-glance) and
[examples/plugins/scratchpad](examples/plugins/scratchpad).

## Skills

A skill is a directory with a `SKILL.md` whose frontmatter names it and
says when it helps; the agent sees the list and loads a skill with
`SkillUse` when a task matches. A run has two kinds:

| Scope | Directories |
| --- | --- |
| project — this workspace only | `.harness/skills` and `.agents/skills` in the workspace, and the skills of the workspace's plugins |
| system-wide — every workspace | `skills/` in the configuration directory (`~/Library/Application Support/kou-conveyor/skills` on macOS, `~/.config/kou-conveyor/skills` on Linux, or beside `KOU_CONVEYOR_CONFIG`), `~/.agents/skills`, the skills of the user's plugins, and the built-in ones |

A project's skill replaces a system-wide one of the same name, and within a
scope the first directory listed wins. One skill is built in:
`kou-conveyor-plugins`, the built-in `guide` plugin's, teaches the agent to
write plugins and skills and where each scope keeps them — ask it for a
plugin. A skill whose frontmatter says
`disable-model-invocation: true` is not offered to the model on its own: it
is named apart, and loads when you ask for it. Skills are read again as they
change: a running agent has a skill added, edited or removed from its next
turn. The browser cockpit's inspector lists them (the Skills bar, `/skills`),
the terminal cockpit's `/skills` too, and `kou-conveyor-runner -list-skills
-workspace .` prints them as JSON.

## Sidebar, terminals and files

The browser cockpit's side panel holds tabs: the inspector, terminals, the
workspace's files, settings, and kinds of tab plugins add. With none open
it says Hello, where tabs are opened. Terminals are drawn by
[restty](https://github.com/wiedymi/restty) (libghostty-vt, WebGPU) and
their shells run on the server, so they outlive reloads and the server's
own restarts with a new build; ⌘D splits one right, ⌘⇧D down. Shells get a
prompt theme of kou-conveyor's own without any change to `~/.zshrc`: zsh
starts with `ZDOTDIR` pointing at the terminal's files, which source the
user's own and then set the prompt — Settings turns it off. Files are
highlighted on the server with chroma.

## Terminal UI

The repository also ships `kou-conveyor-tui`, a terminal wrapper around
`kou-conveyor-runner`, either in a screen of its own or compact, below the
command, with the transcript in the terminal's scrollback. It provides a
streaming timeline of
reasoning and tool activity with collapsible output, a command palette,
session and history pickers, persistent prompt history, a multi-line
composer with an effort control and [file links](#files) (`$`), prompt
editing, each prompt's changes as
diffs beside the transcript, copying by mouse selection, cancellation and
slash commands while preserving the runner's JSONL session format.

```sh
make build
bin/kou-conveyor-tui -workspace .
bin/kou-conveyor-tui -continue          # pick up the most recent session
bin/kou-conveyor-tui -resume            # choose one from the list
bin/kou-conveyor-tui -inline            # below the command; Ctrl-F switches
```

See [`cmd/kou-conveyor-tui/README.md`](cmd/kou-conveyor-tui/README.md) for the
keybindings and non-interactive mode.

## Sessions

Every run appends to a session in `<workspace>/.harness/sessions`, and both
cockpits work with the same files:

- **Resume** any session, from a terminal (`-continue`, `-resume`,
  `-session <id or prefix>`, `/resume`) or a browser (`#/s/<id>` links).
- **Continue** a run that was stopped, crashed or lost its connection: the
  cockpits notice the unfinished run and offer to pick it up.
- **Edit** any prompt: the session goes back to how it was before that
  prompt, and the edited prompt runs in its place under the same session ID.
  In both cockpits `Esc Esc` stops a run and then edits its prompt; the
  mouse edits any prompt (`Edit` in the browser, `✎ edit` in the terminal).
  Files the agent changed keep their changes, and commands started before the
  prompt keep their outcome, so none runs twice.
- **Branch** from before any prompt to try a different instruction, with
  that prompt ready to edit, or **duplicate** a whole session.
- **Rename**, **pin** to the top, **export** as Markdown and **delete**
  (with the output its commands left behind). Titles and pins live beside the
  sessions in `.meta`; the runner never reads them.

A running session can be followed from anywhere but not deleted, branched or
edited.

### Models

Every prompt runs with a model of its own choosing: any model the connection
reaches, whichever provider serves it. Through the [accounts
gateway](#accounts) that is every model of every account and endpoint it
has, so one session can go from Claude to Grok, to GPT or Gemini and back.
The browser cockpit shows the model of the next prompt beside the effort, in
the composer; a click, `M` or `/model` lists the models by provider, the
newest first, with their context windows and the accounts that serve them,
marks those cooling down on every account, and takes an ID the list does not
hold. The terminal cockpit shows it among the composer's controls, where a
click, `Ctrl-P` or `/model` lists them.

The model chosen stays with the session. Its next prompts run with it; a
session without a choice runs with the model its last prompt ran with, and a
new one with the connection's. A prompt queued while the agent works keeps
the model chosen when it was queued, while a forced one goes to the running
agent, which reads it with its own model. Each prompt shows the model that
answered it, marked where the model changed, and so does the Markdown
export.

The session records the model of every turn. One model's answers reach the
next as the conversation they are: messages, tool calls and their results.
The reasoning a provider encrypts or signs for its own model, and the IDs it
gives what its model wrote, stay with that model, which gets them back when
the session returns to it. A session that moves to a model with a smaller
context window is compacted before a turn that would not fit it, as any
session is.

### Images

A prompt can bring images. `Ctrl-V` pastes one from the clipboard in either
cockpit, and `⌘V` or dropping image files onto the composer do in the
browser; pasting the paths of image files, which is what dragging files onto
a terminal does, attaches those files. The prompt names each image by a
label where it was pasted, `[Image 1]` for the first, and the model sees
every image right after its label, beside the text, so the text can refer
to it. An image whose label leaves the text leaves the prompt: `Backspace`
right after a label takes the whole label out at once.

Above the composer a strip shows the images small. With the cursor right
after a label, the image shows large over the transcript while typing goes
on as ever; moving the cursor elsewhere closes it, and a click on an image
in the strip goes to its label. The terminal cockpit draws the pictures with
the kitty graphics protocol in terminals that speak it (kitty, Ghostty), in
true-colour half blocks elsewhere, and describes them without colours;
`KOU_CONVEYOR_IMAGES` (`kitty`, `blocks` or `text`) overrides what it
finds, for instance inside tmux with passthrough allowed. Sent prompts show
their images, which the browser opens large on a click.

The pictures the agent looks at show too: a `ViewImage` call shows the
image it read under its line, as the model got it — small while the call is
folded, large once it is open, in either cockpit — and the browser opens it
large on a click. The session keeps it with the call, so it shows whenever
the session does.

Images go to the model as PNG, JPEG, GIF or WebP, within 2,000 pixels a
side and the bytes providers take: larger ones are scaled down, and BMP and
TIFF converted. A prompt takes up to 20. They are part of the session: a
prompt's images come back with it when it is edited, branched or reused,
stay with it in the queue and go with it when it is forced in. A compaction
keeps the labels of earlier prompts' images in its summary, not the images.

### Files

A prompt can link files and folders with `$`: `$cmd/main.go`,
`$cmd/main.go:120-160` for those lines, `$cmd/main.go:120` for the lines
from 120 on, `$cmd/` for a folder's entries, `$~/notes.md` or `$/etc/hosts`
outside the workspace, and `$"a b.txt"` for a path with spaces. Typing `$`
lists the workspace's files and folders over the composer, in either
cockpit: typing filters them, `↑/↓` choose, `Tab` completes (so does
`Enter`, once a choice was made with the arrows; otherwise `Enter` runs the
prompt as it is), `Esc` closes, and a folder completed lists what is in it.
The list comes from git where the workspace is a work tree, leaving out what
`.gitignore` names, and elsewhere leaves dependencies and caches out. Above
the composer a strip shows what the text links. Only a reference to
something that exists is a link: `$HOME`, `$1` or `$(pwd)` stay text, and
so does a `$` in a code span or a fenced block or after a backslash;
punctuation that ends a sentence is no part of the path.

The model gets a slice of each file, not the whole of it. The runner reads
the links as the prompt runs, and the model reads, after the prompt's text,
each under its reference and numbered as `cat -n` numbers them, the lines
asked for, or else the beginning of the file — the first 100 lines within 8
KB, or the whole of a small one (up to 600 lines and 20 KB) — or a folder's
first 200 entries. It is told which lines were left out, how large the file
is, and a command that reads the next of them, and it reads the rest
itself, from the file as it is now, when it needs it. Lines longer than
2,000 bytes are cut short, binary files show nothing (an image is for
`ViewImage`), the links of a prompt show 160 KB at most together, and a
prompt takes up to 20.

The session records what the model saw with the prompt, so a resumed,
branched or edited session reads the same, while a prompt that is edited,
queued or forced in reads its files as they are when it goes to the agent.
Under a sent prompt its files say what the model saw of each, such as
`lines 1–100 of 345`, and a click shows those lines: the browser opens them
under the prompt, the terminal cockpit in the prompt's card (a click on the
files, or `Ctrl-O`). A compaction keeps the references of earlier prompts,
not what they showed, and the Markdown export lists them. A runner older
than the cockpit gets the prompt's text alone.

### Queue

While the agent works, both cockpits keep writing to it. What is written
waits above the composer, in the session's queue:

- **Queued** (`Enter`, or the composer's queue button): the message runs as
  the next prompt of its own once the run ends, however long it goes on, and
  the queued ones run one after another. Each is a prompt like any other: it
  has its changes, can be edited and branched from.
- **Forced** (`⌘Enter` or the ⚡ button in the browser, `Ctrl-X` in the
  terminal, or `Force` on a queued message): the running agent reads it after
  the tool calls it is making, in the same run. It never cuts a response
  short: it waits for the response the model is writing and for that
  response's tool calls to finish, and goes to the model with their results,
  so the transcript shows it right after the last tool call, marked
  `⚡ forced in`. With nothing running, it goes at once; calls that outlast
  two minutes, such as a server that never exits, do not hold it.

Queued messages can be edited in place, reordered (drag them, or `⌥↑`/`⌥↓`
in the browser and `Shift-↑`/`Shift-↓` in the terminal), forced in and
dropped; `↑` in an empty composer selects the last one. A forced message is
the agent's once it is sent, and can no longer change. A run that is
stopped or fails pauses the queue, with forced messages the agent never read
back at its head, so nothing runs behind the user's back: `Resume` (or
`Enter` in an empty terminal composer) goes on. `/queue` selects the queue,
and `/queue resume`, `/queue pause` and `/queue clear` act on it.

The browser cockpit keeps queues on the server, beside the runs: every tab
sees them and they outlive the tab, the next queued message starts as soon
as a run ends, and the session list counts what waits. The terminal cockpit
keeps them for as long as it is open. Forcing needs a runner built with
`-steer` (see [`cmd/kou-conveyor-runner`](cmd/kou-conveyor-runner/README.md));
with an older one, forced messages wait for the run to end.

### Changes

Both cockpits show what each prompt changed in the workspace, file by file,
in a panel at the right of the transcript: `D` or the `±` button in the
browser, `Ctrl-G` or `/changes` in the terminal. The panel follows the
transcript: scrolled back to an earlier prompt, it shows that prompt's
changes. A prompt forced in while the agent worked belongs to the run of
the prompt before it, and the panel stays on that run. The changed files
form a tree above the diff of one of them; the
first opens by default, and a click (or the arrow keys) picks another. While
a run goes on, the panel is live and shows the file the agent changed last,
until another is picked. Its edge drags, in the terminal too (or `<` and `>`
there), to make it wider for long lines; a double click gives it its default
width back, and the width chosen is kept.

Changes are found by taking snapshots of the workspace, since commands change
files however they like: one as a prompt's run starts, one after each tool
call, others every few seconds while a long command runs, and a last one when
the run ends. Snapshots go to a git repository of the cockpits' own in
`.harness/sessions/.changes`, with an index of its own, so they never touch
the workspace's repository, and they work in folders that have none. They
leave out what `.gitignore` does, and what would take the most room and show
the least: dependencies, caches, logs, archives, media, databases, model
weights, compiled objects, and new files over 5 MB. What changes at the same
time for other reasons (an editor, another run in the same workspace) shows
too. Recording needs `git` on `PATH`, and is off in a home directory and at a
file system's root, where a snapshot would copy everything. Deleting a
session deletes its records; deleting `.changes` frees the room the snapshots
take, and earlier prompts then show no changes.

### Compaction

A long session outgrows the model's context window. Before a turn whose
request would come within 33,000 tokens of the window (a quarter of a window
under 132,000 tokens), the runner **compacts** the conversation: the model
summarizes it in a turn of its own (the user's requests, what was done and
found, the files and code involved, the errors and their fixes, the current
state and the next step), the summary replaces everything before it, and the
turn that was due goes on from the summary, so a task in progress continues
on its own. Prompts the model has not answered yet follow the summary word
for word, as do the user's earlier prompts as far as they fit; tool calls
still running are listed, and their results arrive as messages. The message
with the summary also names the session file, where the model can look up
details the summary left out. The compaction is part of the session, so
resuming, branching and editing prompts after it all start from the summary,
and both cockpits show it as a notice that opens the summary.

Automatic compaction backs off where it cannot help: after three compactions
in a row without a summary, and when the context fills up again within three
turns of a compaction for the third time in a row, as a tool output or a file
too large for the window makes it do. The size of a request is an estimate:
when the provider finds a request too large for the window after all, the
runner compacts right then, and a compaction request it finds too large is
cut down further and sent again.

`/compact` in either cockpit compacts on request, between runs; text after it
says what the summary should focus on (`/compact the API changes`). The
browser cockpit also offers it in the command palette and the inspector.
`KOU_CONVEYOR_AUTO_COMPACT` moves the threshold (`70%`, `150k`) or turns
automatic compaction `off`, and `KOU_CONVEYOR_CONTEXT_WINDOW` sets the
window of a model the runner does not know (see
[`cmd/kou-conveyor-runner`](cmd/kou-conveyor-runner/README.md)).

## Connection settings

Runs connect to one of:

- the **gateway**: the [accounts gateway](#accounts) the browser cockpit
  runs, the single point runs go through to every model its accounts and
  endpoints serve. The settings name only the model; the API follows it —
  Claude models speak the Messages API, the others the Responses API, which
  the gateway translates — unless one is chosen. The browser cockpit sets
  this up on the Accounts tab (the ⚙ button and `,` lead there), the
  terminal cockpit as `GATEWAY` in `/settings`.
- the runner's **environment**: the `KOU_CONVEYOR_LLM_*` variables and the
  workspace's `.env`.
- directly, around the gateway, an endpoint of the **Responses** API
  (OpenAI and compatible endpoints) or the **Messages** API (Anthropic and
  compatible gateways): the base URL, API key and model set in `/settings`
  replace the `KOU_CONVEYOR_LLM_*` variables for the next run; empty
  fields fall back to the provider's defaults and its own key variable, such
  as `ANTHROPIC_API_KEY`. A connection check lists the endpoint's models
  without spending tokens. The Accounts tab moves such a connection into
  the gateway, as one of its endpoints.

The browser cockpit publishes where the gateway listens, and its key, in
`gateway.json` beside the settings while it runs; a run through the gateway,
started from either cockpit, reads it as it starts, and says so when the
gateway is not running.

Settings are stored with mode `0600` in the user configuration directory
(`~/Library/Application Support/kou-conveyor/settings.json` on macOS,
`~/.config/kou-conveyor/settings.json` on Linux) or at `KOU_CONVEYOR_CONFIG`,
and both cockpits share them. So they do the effort runs think with, kept
beside them in `preferences.json`: a level chosen in either cockpit is the
one both use, and an open cockpit takes up the other's choice. The browser never receives the saved key, only
its last four characters, and a saved key is only ever sent to the endpoint it
was entered for: pointing the settings elsewhere requires entering it again.
Runs get the key in `KOU_CONVEYOR_LLM_API_KEY`, which the runner removes
from its environment before the agent can run a command.
`-provider` still selects an environment-configured provider and takes
precedence over the settings.


## Web UI

Run the browser cockpit alongside the agent runner:

```sh
make build-web
bin/kou-conveyor-web -workspace . -address 127.0.0.1:8080
# open http://127.0.0.1:8080
```

The browser cockpit works with several workspaces. The switcher at the top of
the session list (or `W`) moves between them and adds folders, with path
completion as you type. Each workspace keeps its own sessions in
`<folder>/.harness/sessions` and runs with its own `.env`; the server's
`-workspace` folder is always listed first. Added folders are remembered in
`workspaces.json` beside the connection settings, so every server you start
offers them, and removing one from the list never touches the folder. Links
name the workspace: `#/w/<workspace>/s/<session>`.

`make build-tui` and `make build-web` rebuild the runner next to the cockpit,
and `go run ./cmd/kou-conveyor-web` builds it from the same sources, so a
cockpit never drives an older runner by accident.

The web UI streams the transcript as a timeline of prompts, answers, thinking
and collapsible tool calls, with a command palette (`⌘K`), slash commands in
the composer, keyboard shortcuts (`?`), an inspector for tokens, tools and runner diagnostics, each prompt's
[changes](#changes) as diffs (`D`), the [model](#models) of each prompt
(`M`), an effort control shared with the terminal cockpit and light/dark
themes. The composer's row ends in buttons with an icon: run, stop while the
agent works, and queue and force once something is written while it works.
It fits a phone too, where `Enter` adds a line and the buttons send.
The sessions rail and the panel at the right (the inspector or the
changes) are as wide as their edges are dragged, and keep their widths:
`←`/`→` move an edge that has the focus, and a double click gives it its
default width back. Runs belong to the server, not the tab: closing
or reloading the page does not stop a run, and every tab that opens the
session (`#/s/<id>` links work) follows it live and resumes after a dropped
connection without losing events. One run can use a session at a time; a
second prompt gets a clear conflict instead of corrupting the session file,
and the lock also covers `kou-conveyor-tui` running in a terminal. A session
another process is running is marked "in use" and followed as it grows.
Stopping a run stops the commands it started before the session is released.

The runner remains the source of truth; session JSONL files are stored in
`.harness/sessions` just as they are when using the CLI. Configure the
connection in the settings (the ⚙ button, or `,`), or set
`KOU_CONVEYOR_LLM_API_KEY` and optionally `KOU_CONVEYOR_LLM_MODEL` before
starting the server. Sessions can be renamed, pinned, branched, exported and
deleted from their `⋯` menu. `Edit` on any prompt rewinds the session to
before it and runs the edited prompt in its place (`Esc Esc` when no run is
active edits the last one, and pressed while a run stops, opens it once the
run has stopped); `Branch` starts a new session from the history before it
instead, keeping the original.

`$` in the composer links a file of the workspace, as [above](#files). The
composer takes the terminal cockpit's commands: `/` lists the ones that
apply to the session in view, with what they take, and typing filters them;
`↑`/`↓` choose, `Tab` completes and `Enter` runs, and `Esc` closes the list.
`/effort`, `/model`, `/resume` and `/workspace` complete their arguments as
well: the effort levels, the models the connection reaches, the sessions by
title and the workspaces. `/compact`,
`/continue`, `/edit n`, `/fork n`, `/rename`, `/pin`, `/export`, `/delete`,
`/copy`, `/settings`, `/theme` and the rest are in the keyboard sheet (`?`).
Text that starts with a path such as `/usr/bin`, or with a space, goes to the
agent as a prompt.

The server runs shell commands on behalf of whoever can reach it, so it binds
to loopback by default, rejects cross-site requests and unknown `Host`
headers (DNS rebinding), and serves the page with a strict content security
policy. Behind a reverse proxy, list its host name with `-allowed-hosts`.
Interrupting the server stops its runs before it exits.

## Accounts

The browser cockpit runs [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI)
inside itself, through its Go SDK: an **accounts gateway** on
`127.0.0.1:8318` that serves the Responses and Messages APIs from two kinds of
upstream, and spreads requests over them, trying another when one fails:

- **accounts**: subscriptions signed in with OAuth — Claude, Codex,
  Antigravity, Grok, Kimi, Devin and Meta;
- **endpoints**: API keys — of Anthropic, OpenAI, Gemini or xAI, or of any
  provider that speaks the Chat Completions API (OpenRouter, DeepSeek, Groq,
  a local Ollama or vLLM), each with the models it serves under their names
  or aliases.

The **Accounts** tab (`A`, the tab at the top of the session list, or
`#/accounts`) is where runs connect, and its [Usage](#usage) tab what they
used and would have cost as API credits. Its **Connection** sends them through
the gateway with a model it serves, which makes the gateway the single point
they go through; the ⚙ button, `,` and `/settings` lead there. That model is
the default: any prompt can [pick another](#models), of any account or
endpoint. **Browse** (or `↓` in the model's field) lists them by provider. A
connection
straight to an endpoint, saved before, shows as **Direct**, and **Move into
the gateway** turns it into one of the gateway's endpoints, with its key and
its models, and sends runs to the same model through the gateway.

Below come the accounts and the endpoints, each with:

- **State**: an account is ready, cooling down until a rate limit resets,
  failing (a revoked or expired sign-in) or disabled, and shows which models
  cool down, why and until when; an endpoint is ready, failing while its
  latest request failed, or disabled.
- **Uptime**: its requests over the last day or week (`24h`/`7d`) as a bar
  of slots, green when every request succeeded, amber when some failed and
  red when most did, with the success rate, the tokens and the latency, and
  the latest errors with their status and the provider's message. The
  gateway reports every request it sends upstream, retries on other
  accounts included; the cockpit keeps a week of them in `history.json`, so
  uptime outlives restarts.
- **Limits** (accounts): what is left of each rate-limit window — Claude's
  5-hour and weekly windows, Codex's primary, secondary and per-model
  windows and credits, Antigravity's model groups, Kimi's request limits,
  Grok's credits — asked of the provider with the account's own token, and
  otherwise read from the rate-limit headers of the account's latest
  response.
- **Models**: those the gateway serves with it. An account lists them a
  moment after it signs in or is imported, once the gateway has taken it
  up; an endpoint lists those it was given, or serves its provider's known
  ones. A model that cools down stays in every list, marked, instead of
  dropping out until an account can take it again.

**+ Add** signs an account in, adds an endpoint or imports credentials. For
an account, a provider's page opens in a new tab and sends the browser back
to a callback on this machine, or, for Grok, Kimi and Meta, it shows a code
to enter there; when the browser runs on another machine, or the callback's
port is taken, paste the address the provider ended on into the dialog.
**Import** takes credential files another CLIProxyAPI signed in. An
endpoint takes a kind, a base URL, a key and its models, which **Fetch
models** lists from the endpoint itself. The `⋯` menu of an account
refreshes its limits or its token, and of an endpoint edits it or checks its
key; both disable and remove.

### Usage

The Accounts view's second tab, **Usage** (`U`, `/usage`,
`#/accounts/usage`, or a click on the gateway's API cost), turns tokens into
API credits: what the gateway's requests used, and what the same tokens
would have cost through the providers' APIs. The gateway reports every
request it sends upstream with its account, its model and its tokens by the
kinds APIs price apart — fresh input, cache reads, cache writes and output —
and the cockpit keeps them hour by hour for 92 days, in `usage.json` beside
the history. Over the last day, week, month or quarter (`1`, `7`, `3`, `9`)
the tab shows:

- the **API cost**, against the span before and at its pace for a month,
  beside the tokens, the requests and how much of the input a cache served,
  with what caching saved;
- a chart of the cost, the tokens or the requests, a bar an hour, four hours
  or a day, stacked by model or by kind of token, with what a bar holds
  under the pointer;
- the **models** and the **accounts**, the costliest first: what each
  subscription or key would have cost as API credits;
- **Tokens → credits**: each kind's share of the tokens beside its share of
  the cost, and what a million of it came to;
- the hours of the week, darker where more was spent.

The rail narrows all of it down to a provider, an account or a model.

Prices are per million tokens of each kind. The cockpit fetches
[OpenRouter's](https://openrouter.ai/api/v1/models) model list, which prices
the models of every provider it resells at the providers' own prices and
follows new ones within days, when the gateway starts and twice a day after
(**Update** fetches it now), keeps it in `openrouter-prices.json` for when
OpenRouter cannot be reached, and prices a model it lists by it: Claude's
cache writes at its one-hour price. `KOU_CONVEYOR_OPENROUTER_PRICES=off`
keeps the cockpit from asking. For a model OpenRouter does not list, the
cockpit knows the list prices of the Anthropic, OpenAI, Google, xAI,
Moonshot, DeepSeek, Zhipu and MiniMax models it knows, and prices a newer
model of a family it knows as
the latest of the family it knows, marked as an estimate (`≈`):
`claude-opus-5-5` as Claude Opus 4.5. Claude's cache writes count at the
one-hour rate the gateway asks for; long-context and priority tiers are left
out. The price table, or `P`, sets the price of a model, or of every model a
pattern matches (`gpt-6-*`), borrowing a list price if you like; yours
override OpenRouter's and the cockpit's and are kept in `prices.json`. A model without a
price says so, and its tokens are left out of the cost until it has one.
Amounts show in US dollars, or in credits of your own worth a set amount
each (**Amounts in**). The Accounts tab shows the last day's cost in the
gateway's strip, and beside each account and endpoint what its requests of
the span would have cost.

### Updates

The gateway is the CLIProxyAPI release `go.mod` requires
(`github.com/router-for-me/CLIProxyAPI/v8`). Once a page of the browser
cockpit opens, it asks the server whether a newer release is out, and the
server asks the module proxies `GOPROXY` names, as `go` does, at most once an
hour. A newer release opens a dialog — `v8.0.6 → v8.0.10`, how many
releases that is, when it came out and a link to what changed — and shows as
`↑ v8.0.10` beside the version in the Accounts view, which opens the dialog
again, as do `/accounts update` and the palette. **Update**, on a server that
builds itself from its checkout (see [Plugins](#plugins)), runs `go get` on
copies of `go.mod` and `go.sum` and builds the programs with them; only once
they build do the copies take the place of `go.mod` and `go.sum`, and the
server builds itself anew and restarts with the new gateway as soon as no
agent is at work, the dialog following it there. A release the programs do
not build with leaves both files as they were, and the dialog shows the
compiler's errors. A server that cannot update itself — installed without a
checkout, started with `-rebuild=false`, or with no `go` on the `PATH` —
gives the `go get` to run instead. **Skip this version** stops offering that
release and **Later** waits a day. A newer major version (`/v9`) is another
Go module, with an API of its own: the badge beside the version and the
dialog say it is out, but moving to it takes changes to the code.

### Where it lives

The gateway keeps its `config.yaml`, where the endpoints live too, the
credentials (`auths/`), its log, the history, the usage (`usage.json`) and
your prices (`prices.json`) in `cliproxy/` beside the
connection settings (`-accounts-dir` moves it). The cockpit keeps the listen
address, the credentials folder, an API key for runs and a management secret
in the file and leaves everything else to the user; its management API
answers only on loopback, to a password the cockpit makes up at every start.
Neither tokens nor keys reach the browser. `-accounts 127.0.0.1:<port>`
moves the gateway to another port — for instance beside a CLIProxyAPI of its
own on 8317, which can then be one of its endpoints — and `-accounts off`
turns it off. Two gateways must not share credentials: each refreshes tokens
on its own, and providers revoke a refresh token used twice.

```sh
bin/kou-conveyor-web -workspace .                          # gateway on 127.0.0.1:8318
bin/kou-conveyor-web -accounts 127.0.0.1:9318 -accounts-dir ~/cliproxy
bin/kou-conveyor-web -accounts off
```
