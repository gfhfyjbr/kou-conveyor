---
name: kou-conveyor-plugins
description: >-
  How to write plugins for kou-conveyor, the harness this agent runs in —
  tools the agent calls, skills, system-prompt instructions, slash commands,
  and scripts and styles for the browser cockpit — and the scopes plugins
  and skills live in: a workspace's own (project) or system-wide (the
  user's, in every workspace). Use when asked to create, change, debug or
  explain a kou-conveyor plugin or skill, to give the agent a new tool, or
  to change the cockpit's interface.
---

# Writing kou-conveyor plugins

A plugin is a directory with a `plugin.json`. It can give the agent **tools**
(commands it calls), **skills** and **instructions** for its system prompt,
give both cockpits **slash commands**, and give the browser cockpit a
**script and a style sheet** that build part of its interface. Everything
kou-conveyor adds is a plugin: the agent's own tools are the built-in `core`
plugin, the browser cockpit is built-in plugins written against the same API
as yours, and this skill comes with the built-in `guide` plugin.

Nothing needs a restart. The runner looks at the plugins before every turn
of the agent and takes up what changed in its next request; open browser
pages load a changed plugin anew in place; the terminal cockpit takes up
changed commands within a couple of seconds.

## 1. Choose the scope

Where the plugin's directory is decides who has it and when it runs.

| Scope | Directory | Who has it | Runs |
| --- | --- | --- | --- |
| **workspace** (project) | `<workspace>/.harness/plugins/<name>/` | that workspace alone: its runs, and the browser cockpit while it is in view | only once the user trusts the workspace |
| **system-wide** (user) | `<config>/plugins/<name>/` | the user, in every workspace | at once, unless turned off |
| **built in** | compiled into kou-conveyor | everyone, in every workspace | unless turned off |

`<config>` is kou-conveyor's configuration directory: the directory of
`$KOU_CONVEYOR_CONFIG` when that is set — a cockpit sets it for the runs it
starts, so your shell usually has it — else `~/Library/Application
Support/kou-conveyor` on macOS and `${XDG_CONFIG_HOME:-~/.config}/kou-conveyor`
on Linux. The macOS path has a space: quote it.

```sh
if [ -n "$KOU_CONVEYOR_CONFIG" ]; then config=$(dirname "$KOU_CONVEYOR_CONFIG")
elif [ "$(uname)" = Darwin ]; then config="$HOME/Library/Application Support/kou-conveyor"
else config="${XDG_CONFIG_HOME:-$HOME/.config}/kou-conveyor"; fi
```

How to choose:

- **Workspace** for what belongs to the project: its build, tests, deploys,
  services, data and conventions. The plugin travels with the repository —
  commit `.harness/plugins/<name>/` so everyone who works on it gets the
  plugin (each trusts the workspace once). A workspace's plugins come from
  `.harness/plugins` alone: not from `.agents`, nor anywhere else in it.
  `.harness/sessions` beside it is state; if the repository ignores
  `.harness/`, un-ignore the plugins (`/.harness/*` then
  `!/.harness/plugins/`) to share them.
- **System-wide** for what belongs to the user: their own tools, services
  and habits, wanted in every project.
- When it is not clear, ask. Writing into `<config>` changes every
  workspace; say so when you do it.

**Trust.** A workspace's plugins run code from wherever the repository came
from, so until the user trusts the workspace they are listed but off, with
the reason "the workspace is not trusted". The user trusts it with the
**Trust this workspace** button in the Project bar of the browser cockpit's
Plugins section, or `/plugins trust` in either cockpit; trust is kept per
folder in `<config>/plugins.json`. Never trust a workspace on the user's
behalf — do not edit `plugins.json` — tell them the plugin waits for it.
`KOU_CONVEYOR_TRUST_WORKSPACE_PLUGINS=1` trusts the workspace for one
command without recording anything, which is how to check a workspace
plugin (below).

**Names and replacing.** A plugin replaces an earlier one of the same name,
in the order built in, system-wide, workspace (a workspace's only once it
is trusted): a workspace can bring its own version of a user's plugin, and
a plugin named `composer` replaces the cockpit's composer. So name yours
apart from the built-in ones unless you mean to replace one: `core`,
`guide`, `canvas-agent` (the canvas's tools, for agents on a canvas), and
the browser cockpit's `accounts`, `canvas`, `changes`, `commands`,
`composer`, `connection`, `edit`, `effort`, `explorer`, `files`, `header`,
`help`, `images`, `inspector`, `layout`, `markdown`, `models`, `palette`,
`plugins`, `queue`, `session`, `session-list`, `sidebar`, `skills`,
`terminal`, `theme`, `timeline`, `ui` and `workspaces`
(`kou-conveyor-runner -list-plugins` lists what is there now). Two active
plugins cannot share a tool or a command: the first keeps it and the other's
is left out, with an error.

**Turning off.** The Plugins section's **Turn off** writes the name under
`disabled` in `<config>/plugins.json`, for every workspace;
`KOU_CONVEYOR_DISABLED_PLUGINS=a,b` turns plugins off for one run.

### Skills have the same two scopes

A skill needs no plugin: it is a directory with a `SKILL.md` (see
[Skills](#4-skills)), and a run has those of these directories, the first
to name a skill winning:

| Scope | Directories, in order |
| --- | --- |
| **project** | `<workspace>/.harness/skills/`, `<workspace>/.agents/skills/`, the workspace's plugins' skills |
| **system-wide** | `<config>/skills/`, `~/.agents/skills/`, the user's plugins' skills, the built-in plugins' skills (this one's) |

A project's skill replaces a system-wide one of the same name. Skill
directories need no trust — a skill is text the agent reads, not code — but
a workspace plugin's skills come only with the plugin, once it runs.
`.agents/skills` is shared with the other agents that follow that
convention; `.harness/skills` and `<config>/skills` are kou-conveyor's
alone. Prefer a plain skill directory to a plugin when all you add is a
skill.

## 2. The manifest

```json
{
  "name": "todo-scan",
  "version": "0.1.0",
  "description": "Finds the TODO comments of the workspace.",
  "tools": [
    {
      "name": "TodoScan",
      "description": "List the TODO and FIXME comments of the workspace as file:line: text lines, at most 200.",
      "parameters": {
        "type": "object",
        "properties": {"path": {"type": "string", "description": "A folder to search, relative to the workspace; all of it by default."}}
      },
      "run": ["/bin/sh", "./scan.sh"],
      "max_output_length": 20000
    }
  ],
  "skills": "skills",
  "prompt": "prompt.md",
  "commands": [
    {"name": "todos", "args": "[folder]", "description": "Summarize the TODOs",
     "prompt": "Call TodoScan and group what it finds by theme. Folder: {{args}}"}
  ],
  "web": {"script": "web/plugin.js", "style": "web/plugin.css", "after": ["inspector"]}
}
```

Unknown fields are errors, and so is a path that leaves the plugin's
directory: a manifest that breaks a rule leaves the whole plugin out, and
`-list-plugins` says why.

| Field | |
| --- | --- |
| `name` | required: lowercase letters, digits and dashes, up to 64 |
| `version`, `description` | shown in the cockpits |
| `tools` | tools the agent can call ([3](#3-tools)) |
| `skills` | a directory of skills, one per subdirectory with a `SKILL.md` ([4](#4-skills)) |
| `prompt` | a file, up to 64 KiB, whose text joins the system prompt ([5](#5-instructions)) |
| `commands` | slash commands for both cockpits ([6](#6-commands)) |
| `web` | `script`, an ES module (`.js` or `.mjs`), and `style`, a `.css` file, for the browser cockpit; `after`, plugins to start before this one when they are there ([7](#7-the-browser-cockpit)) |
| `verify` | shell commands that check the work once the agent says it is done with a prompt ([5a](#5a-verification)) |
| `sandbox` | how the workspace's commands run apart from the machine: `image`, `setup`, `environment`, `mounts` ([5b](#5b-sandbox)) |
| `canvas` | what the plugin adds to the browser cockpit's canvas: `harnesses` (presets of terminal nodes), `sources` (nodes that bring events) and `templates` ([5c](#5c-canvas)) |
| `requires` | what the plugin needs to run: `env`, variables that must be set; without them it is not active |

## 3. Tools

A tool runs a command. For each call:

- `run` is the program and its arguments, without a shell. Elements that
  start with `./` or `../` are paths in the plugin's directory.
  `["/bin/sh", "./scan.sh"]` needs no executable bit; `["./scan"]` needs one
  and a shebang; a program on `PATH` works (`["python3", "./scan.py"]`) —
  if the machine has it.
- It runs **in the workspace**, not in the plugin's directory:
  `$KOU_CONVEYOR_PLUGIN_DIR` is where the plugin's own files are.
- The arguments arrive on **standard input** as one line of compact JSON —
  `{}` when there are none.
- What it prints on **standard output** is the result the model reads.
  Standard error follows it under `Stderr:`, a nonzero exit adds
  `Exit code: N`, and no output at all reads `(no output)`. Each stream is
  cut to `max_output_length` characters (40,000 by default, at most
  1,000,000), keeping its start and end and the path of the whole output.
- The environment is the runner's, with `KOU_CONVEYOR_PLUGIN_NAME`,
  `KOU_CONVEYOR_PLUGIN_DIR`, `KOU_CONVEYOR_WORKSPACE`,
  `KOU_CONVEYOR_SESSION_ID`, `KOU_CONVEYOR_TOOL_NAME` and
  `KOU_CONVEYOR_TOOL_CALL_ID`.
- A call is a durable operation, as a `Bash` command is: it runs in the
  background while the agent goes on, and stops with the run. A request's
  `disallowed_tools` leave plugin tools out too.

Declaring one:

- `name`: a letter, then letters, digits, `_` and `-`, up to 64; not a name
  another active plugin's tool has — core's own tools are taken.
- `description` (required) is all the model knows of the tool until it
  calls it: say what it does, what it returns, and when to use it.
- `parameters`: a JSON schema of `"type": "object"`; without it the tool
  takes none.

```sh
#!/bin/sh
# scan.sh: the arguments are on stdin; what this prints is the result.
path=$(sed -n 's/.*"path"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
grep -rnE 'TODO|FIXME' -- "${path:-.}" 2>/dev/null | head -200
```

Parse the arguments with what the machine has — `jq`, `python3 -c 'import
json, sys; …'`, or `sed` for a flat field as above — and check that it is
there: a program the machine lacks fails every call. Print what the model
needs, compactly (JSON or short lines), write errors as text it can act on,
and exit nonzero when the call failed. Try the command by hand before the
agent does:

```sh
printf '{"path":"src"}' | KOU_CONVEYOR_WORKSPACE="$PWD" /bin/sh .harness/plugins/todo-scan/scan.sh
```

## 4. Skills

`"skills": "skills"` names a directory whose subdirectories each hold a
skill — the same layout as `.harness/skills`, `.agents/skills` or
`<config>/skills`:

```markdown
---
name: release-notes
description: Write release notes from the commits since the last tag. Use when asked for release notes or a changelog.
---

1. Find the last tag with `git describe --tags --abbrev=0`.
…
```

- `name` and `description` are required; the description is all the agent
  sees of the skill until it loads it, so say what it is for and when to
  use it. Two skills of one directory cannot share a name.
- `disable-model-invocation: true` keeps the agent from loading the skill
  on its own: it loads it when the user asks for it by name.
- The agent loads a skill's `SKILL.md` with `SkillUse`, and resolves the
  relative paths it names against the skill's directory: keep long material
  in files beside it and name them.

## 5. Instructions

`"prompt": "prompt.md"`: the file's text joins the system prompt of every
run the plugin is active in, under `## <name> plugin`. It costs context on
every request, so keep it to what the agent must always know — for
instance when to use the plugin's tools — and put anything long or
occasional in a skill.

## 5a. Verification

```json
"verify": ["go build ./...", "go test ./..."]
```

Once the agent says it is done with a prompt, the commands run in the
workspace, one after another, and stop at the first that fails: its output
goes back to the agent as a message from the harness, which fixes what it
reports (or explains why it is expected) before the run ends. A prompt is
verified up to three times. The workspace's own `.harness/verify.json`
(`{"commands": [...], "timeout": seconds}`) and `KOU_CONVEYOR_VERIFY` add
commands the same way; `KOU_CONVEYOR_VERIFY=off` turns verification off.
Keep the checks fast and quiet: the agent reads their output.

## 5b. Sandbox

```json
"sandbox": {"image": "golang:1.27", "setup": ["go mod download"], "environment": {"GOFLAGS": "-mod=mod"}, "mounts": ["/home/me/.cache/go-build:/root/.cache/go-build"]}
```

With `KOU_CONVEYOR_SANDBOX=container` (or the cockpit's sandbox setting),
each session works in a git worktree of its own, on branch `kou/<session>`
in `.harness/worktrees/`, and its commands run in a container of `image`
with the workspace mounted at its own path, so paths mean the same inside
and out. The container is one per workspace and kept between runs; `setup`
runs once when it starts, `environment` is set in it, and `mounts` are
extra volumes in docker's `host:container` form. A workspace can instead
keep a Dockerfile in `.harness/sandbox/`, which is built into the image,
or set `KOU_CONVEYOR_SANDBOX_IMAGE`. `KOU_CONVEYOR_SANDBOX=worktree` gives
the worktree alone, with the commands on the machine.

## 5c. Canvas

The browser cockpit's canvas is a board of terminals, agents and sources of
events, wired output to input. A plugin adds to it with `canvas`:

```json
"canvas": {
  "sources": [{
    "id": "github-issues", "title": "GitHub issues", "run": ["python3", "./bin/github-issues"],
    "mode": "poll", "interval": "60s",
    "config": {"type": "object", "required": ["repo"], "properties": {"repo": {"type": "string", "title": "Repository"}}},
    "outputs": [{"id": "opened", "title": "Opened"}, {"id": "updated", "title": "Updated"}]
  }],
  "harnesses": [{"id": "aider", "title": "Aider", "command": ["aider"], "status": ["idle"], "idle_ms": 4000, "output": "none"}],
  "templates": "templates"
}
```

- A **source** runs in the workspace. Its standard input is one line of
  JSON — `{"config": {…}, "node": "…", "canvas": "…", "first_run": true}`
  — and its standard output lines of JSON:
  `{"type": "event", "port": "opened", "key": "42:2026-10-02T12:00:00Z", "title": "…", "text": "…", "data": {…}}`,
  `{"type": "status", "state": "ok" | "error", "text": "…"}` or
  `{"type": "log", "text": "…"}`. In `poll` mode it runs every `interval`
  (10 seconds at least) and exits; in `stream` mode it runs on, until its
  standard input closes. The canvas drops events whose `key` it has seen,
  and gives the source `KOU_CANVAS_STATE_DIR`, a directory of its own for
  a cursor, besides `KOU_CANVAS_URL`, `KOU_CANVAS_TOKEN`, `KOU_CANVAS_ID`
  and `KOU_CANVAS_NODE`. An event's `port` is one of its `outputs` (`out`
  when it names none); its `template` (`{{title}}`, `{{text}}`,
  `{{data.<path>}}`) is how an edge from it words its events, unless the
  edge has a template of its own.
- A **harness** is a preset of a terminal node: `command` and `args` (with
  `{{brief}}`, `{{files.<name>}}`, `{{node.title}}`, `{{config.<key>}}`),
  `files` written before it starts, `env`, `launch` (`launcher`, `type`,
  `shell`), `input` (`paste`, `newline`, `submit`, `submit_delay_ms`),
  `status` (`hooks`, `notify`, `osc133`, `idle`) with `idle_ms` and `ready`,
  `output` (`hooks`, `notify`, `osc133`, `screen`, `none`), `session_arg`,
  `resume`, `config` (a JSON schema of the node's settings) and `check`.
- `config` schemas are objects of `string`, `number`, `integer`, `boolean`
  and arrays of strings, with `enum`, `default`, `title`, `description`,
  `pattern` and `required`: the canvas makes a form of them.
- `templates` is a directory of canvases saved as JSON (⋯ → Save as
  template), shown when a canvas is made.

A workspace's sources and harnesses run only in a trusted workspace, as its
tools do.

## 6. Commands

```json
"commands": [{"name": "todos", "args": "[folder]", "description": "Summarize the TODOs", "prompt": "Call TodoScan and group what it finds. Folder: {{args}}"}]
```

A command is typed in either cockpit's composer: `/todos src`. `prompt` is
what it asks the agent; `{{args}}` stands for the text after the command,
which is otherwise added at the end after a blank line. `name` is a
lowercase letter, then lowercase letters, digits and dashes, up to 32;
`description` and `prompt` are required; `args` documents the argument, as
`[optional]` or `<required>`. Commands show in the `/` suggestions, the
command palette and the keyboard sheet. A web plugin can register commands
with logic of their own.

## 7. The browser cockpit

The page is a small plugin host: it imports the `script` of every active
plugin of the workspace in view and calls its default export (or
`activate`) with the cockpit API. Everything a plugin adds through the API
belongs to it, and goes when the plugin is loaded anew, turned off, or its
workspace is left.

```js
// web/plugin.js
export default function activate(cockpit) {
  const { h } = cockpit;
  // A section of the inspector: a bar that opens onto its body.
  cockpit.inspector.register({
    id: 'todos', title: 'TODOs', order: 60, fold: true,
    meta: () => 'scan',
    render: () => h('div', null,
      h('p', { class: 'none', text: 'Ask the agent for the TODOs of the workspace.' }),
      h('div', { class: 'ins-actions' }, cockpit.ui.button('Scan now', () => cockpit.prompt('Call TodoScan and summarize it.')))),
  });
  // How the tool's calls look in the transcript.
  cockpit.tools.register('TodoScan', { summary: (entry) => `${(entry.tool?.output || '').split('\n').filter(Boolean).length} found` });
  cockpit.palette.register({ icon: '☐', label: 'Scan the TODOs', run: () => cockpit.prompt('Call TodoScan.') });
  return () => {}; // runs when the plugin goes; what the API added goes by itself
}
```

- Add everything through the API — `ui.mount`, `contribute`, `on`, `listen`,
  `timeout`, `interval`, `styles.add` — so it goes with the plugin. Keep
  state that should outlive a version in `cockpit.hot.data`, and undo
  anything else in the function `activate` returns (or an exported
  `deactivate`).
- Build elements with `h(tag, { class, text, data, onclick, … }, …children)`.
  What comes from the runner — tool output, file names — is untrusted: never
  put it into `innerHTML`.
- Files are served from `/api/w/<workspace>/plugins/<name>/v/<version>/<path>`,
  so a module imports its neighbours relatively (`import './lib.js'`) and
  the kernel's helpers from `/kernel/dom.js`. Scripts, styles, JSON, images,
  fonts, WebAssembly and text are served; hidden files are not.
- Style sheets cascade in the order plugins start. Use the theme's tokens
  (`--bg`, `--bg-2`…`--bg-4`, `--fg`, `--fg-2`…`--fg-4`, `--line`,
  `--line-2`, `--accent`, `--ok`, `--warn`, `--err`, `--mono`, `--sans`) and
  classes (`act` for buttons, `label`, `none` for empty states,
  `ins-actions` for a row of buttons, `kv` rows) so the plugin fits both
  themes.
- `after` names plugins whose services yours uses at start, such as
  `inspector` or `layout`.

Any part of the page can be changed: mount a node with a part's id in its
slot to take its place, provide a service under a built-in's name, or name
a plugin like a built-in one to replace it whole. The whole API — slots,
contribution points, services, events and hooks — is in
[reference/web.md](reference/web.md).

## 8. Check it

1. **Is it read?** `kou-conveyor-runner -list-plugins -workspace "$PWD"`
   prints one JSON line per plugin — whether it is `active`, the `reason`
   it is not, and what it adds (`tools`, `commands`, `skills`,
   `instructions`, `web`) — then an `{"error": …}` line for each manifest
   that could not be read. Prefix `KOU_CONVEYOR_TRUST_WORKSPACE_PLUGINS=1`
   to see a workspace plugin as it will run once trusted. `-list-skills`
   does the same for skills. The runner is installed beside the cockpits
   (`~/.local/bin` by default); `command -v kou-conveyor-runner` finds it.
2. **Does the tool work?** Run its command by hand, as above.
3. **Does the agent have it?** An active plugin's tool is the agent's from
   its next turn — call it. The runner says `plugin> the plugins changed…`,
   `plugin error> …` and `skill error> …` on stderr, which the browser
   cockpit shows in the inspector's Runner log.
4. **Does the page load it?** The inspector's Plugins section shows each
   plugin, the version loaded and why one failed; in the browser console,
   `cockpitKernel.instances()`, `cockpitKernel.failures()`,
   `cockpitKernel.services()` and `cockpitKernel.slots()`. `/?safe` opens
   the page with the built-in plugins only.

When the plugin is done, tell the user where it is, what it adds, and — for
a workspace's — that it runs once they trust the workspace.

## Pitfalls

- A typo in `plugin.json` leaves the whole plugin out: check `-list-plugins`
  after every edit.
- A workspace's plugin does nothing until the workspace is trusted.
- A plugin named like a built-in replaces it whole: a `theme` plugin that
  sets one colour takes the entire theme's place.
- A tool or command another active plugin has is left out.
- Tools run in the workspace; the plugin's files are under
  `$KOU_CONVEYOR_PLUGIN_DIR`.
- Output beyond `max_output_length` is cut: print what the model needs.
- A `prompt` costs context on every request of every run.
- In the kou-conveyor source, `docs/plugins.md` documents all of this, and
  `examples/plugins/git-glance` (every part) and
  `examples/plugins/scratchpad` (a view of its own) are working examples.
