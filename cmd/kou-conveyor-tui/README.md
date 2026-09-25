# kou-conveyor-tui

`kou-conveyor-tui` is a terminal cockpit for `kou-conveyor-runner`. The runner
remains the source of truth and the UI talks to it through its JSONL stream, so
all sessions, tool calls and logs stay compatible with the regular runner.

Install everything with the repository's `install.sh`, or build both binaries
from the repository root:

```sh
make build
```

Or only the terminal cockpit:

```sh
make build-tui
```

Run it from a project workspace:

```sh
bin/kou-conveyor-tui -workspace .
```

`go run ./cmd/kou-conveyor-tui` works too: the UI then builds the runner from
the same sources instead of using whatever runner is on `PATH`. A runner that
is older than the UI and lacks the chosen provider is reported by path before
the run starts.

Then choose the connection with `/settings` (or set `OPENAI_API_KEY`, or
`ANTHROPIC_API_KEY` with `-provider anthropic`).

The screen is laid out as the browser cockpit's stage, without its rail of
sessions: a bar with the workspace, the session's title and ID, the token
counts and the run's state; the transcript; and under it the composer in its
box, with the model, the effort and the run button in a row of its own (on a
short terminal, on the box's bottom edge) and the key hints beneath. A new
session shows a card with tasks to start with; a click on one puts it in the
composer.

The transcript is a timeline: prompts are numbered and framed, with the
accent along their edge; tool calls collapse to one line with their state,
duration and exit code, and open into a card whose sections are their
input, output and errors; thinking folds away. Click a tool call or a
thinking line to expand it, or press `Ctrl-O` for all of them. A
`ViewImage` call shows the picture the agent looked at under its line: small
while it is folded, large once it is open, and a click on the picture folds or
unfolds it. Pictures are drawn as described for images below, and in inline
mode a call goes to the scrollback with its picture.

## Changes

`Ctrl-G` (or `/changes`) opens the changes panel at the right of the
transcript: what the prompt in view changed in the workspace, as a tree of
files above the diff of one of them, with the lines' old and new numbers.
Scrolling the transcript to another prompt shows that prompt's changes. The
first file opens by default; while a run goes on, the panel is marked live and
shows the file the agent changed last, until another is picked. How changes
are recorded, and what is left out, is in the repository's
[README](../../README.md#changes).

The panel takes the keys when it opens, and `Tab` moves them between it and
the composer:

- `↑/↓` go along the tree, and the file the cursor lands on shows; `←/→`
  fold and unfold the folder under it.
- `PageUp/PageDown`, `Shift-↑/↓` and `Home/End` scroll the diff.
- `[` and `]` show the prompt before and after, and scroll the transcript to
  it.
- `f` follows the run again after a file was picked.
- `<` and `>` (or `Shift-←/→`) move the panel's edge four columns left or
  right, making it wider or narrower; `=` gives it its default width back.
- `Esc` or `Tab` go back to the composer, and so does any key the panel has
  no use for: typing just goes on in the composer. `Ctrl-G` closes the panel.

The mouse works too: a click picks a file or folds a folder, the wheel
scrolls the tree or the diff under it, dragging over the diff selects and
copies it (without the line numbers when the selection starts past them),
and `×` closes the panel. The panel's edge, the rule between it and the
transcript, drags: the panel takes the columns right of the pointer, the
transcript the rest, and each keeps room of its own (40 columns for the
transcript, 36 for the panel). A double click on the edge gives the panel its
default width back, a share of the terminal. Where the terminal is narrower
than 110 columns the panel covers the transcript, and has no edge. It stays
open for the next start, as wide as it was. Inline, where
there is no room for it, a line in the scrollback sums up what each run
changed, and `Ctrl-G` takes a screen of its own to show the panel.

## Layouts

There are two, and `Ctrl-F` (or `/inline` and `/fullscreen`) switches
between them at any time. The one used last is the one the next start takes;
`-inline` or `-fullscreen` chooses for one start (`-compact` still works too).

- **Fullscreen** takes a screen of its own, the terminal's alternate screen,
  as full-screen editors do. The transcript scrolls in a view of its own and
  the mouse belongs to the cockpit: clicks, selection and the pointer below.
- **Inline** stays below the command that started it. Entries go into the
  terminal's own scrollback as they settle; only what is still changing, the
  status line and the composer are redrawn beneath them. The terminal keeps
  its scrolling and its text selection, `Ctrl-L` clears the screen as in a
  shell, and the conversation stays on screen when the cockpit exits, with
  the command that resumes it. What is printed stays printed: after an edit,
  a line marks where the session went back to, and `Ctrl-O` applies to what
  comes next.

Keys:

- `Enter` runs the prompt. `Shift-Enter`, `Alt-Enter` or `Ctrl-J` adds a
  line, and so does `\` then `Enter`, in any terminal. (`Shift-Enter` needs a
  terminal that reports it: one that sends ESC CR for it, or CSI u.) The
  composer grows with its text and takes any number of lines; pasted text
  keeps its line breaks. `↑/↓` move between lines, and at the first or last
  line walk prompt history and bring the draft back.
- `Ctrl-V` pastes an image from the clipboard (terminals keep `⌘V` for
  text), and a paste of the paths of image files, as dragging files onto the
  terminal gives, attaches those files. The prompt names each by a label,
  `[Image 1]`, and brings the images whose labels it still holds. They show
  small above the composer (a click on one goes to its label); with the
  cursor right after a label the image shows large over the transcript while
  typing goes on, and `Backspace` there takes the label, and its image, out
  at once. kitty and Ghostty draw them with the kitty graphics protocol,
  other terminals in half blocks; `KOU_CONVEYOR_IMAGES=kitty|blocks|text`
  overrides that. See the repository's [README](../../README.md#images).
- While the agent works, `Enter` queues the prompt: it waits above the
  composer and runs as the next prompt once the run ends, one after
  another. `Ctrl-X` (or `Ctrl-Enter` in terminals that report it) forces it
  in instead: the agent reads it after the tool calls it is making, without
  waiting for the run to end and without cutting a response short, and it
  shows in the transcript right after the last tool call, marked
  `⚡ forced in`. `↑` in an empty composer gives the queue the keys: `↑/↓`
  select, `Enter` takes a message into the composer to edit (`Enter` puts it
  back where it was, `Esc` as it was), `Ctrl-X` forces it, `Backspace`
  drops it, `Shift-↑/↓` move it, and `Esc` or typing goes back to the
  composer; a click selects one too. A run that is stopped or fails pauses
  the queue, and `Enter` in the empty composer goes on with it. See the
  repository's [README](../../README.md#queue).
- `Esc Esc` stops a run. Between runs it edits the last prompt, and pressed
  while a run stops, it does so once the run has stopped; `✎ edit` on any
  prompt (a click) or `/edit n` edits another. The prompt goes to the
  composer and what comes after it fades: `Enter` takes the session back to
  before the prompt and runs the edited one in its place, in the same
  session; `Esc` keeps it as it was. Files the agent changed keep their
  changes. What the composer held comes back afterwards.
- `Ctrl-C` stops a run; when idle it clears the prompt, and twice quits.
  `Ctrl-D` quits; `Ctrl-Z` suspends.
- `Ctrl-K` opens the command palette, `Ctrl-S` saved sessions and `Ctrl-R`
  prompt history. All three filter as you type and accept the mouse.
- In the sessions list, `Ctrl-E` renames the selected session, `Ctrl-T` pins
  it to the top, `Ctrl-B` duplicates it and `Ctrl-D` twice deletes it.
- The composer's controls, under its text, are the model, the effort and
  the button that says what `Enter` does: run, or while the agent works
  queue (with force in beside it) or stop. A click on the button does it.
- The effort (the model's thinking level) shows among the composer's
  controls: `Ctrl-T` or `Shift-Tab` moves to the next level, `Alt-↑/↓` raise
  and lower it, and a click on one of its bars picks that level. It is shared
  with the web cockpit: a level chosen in either is the one both use, a
  cockpit that is open takes up the other's choice within seconds, and the
  next start begins with it (`-thinking-level` starts with another).
- The model of the next prompt shows before the effort, on terminals wide
  enough for it, with its provider's colour, and in the status line: any
  model the connection reaches,
  of any provider (through the accounts gateway, every model of its accounts
  and endpoints). `Ctrl-P`, `/model` or a click on it lists them, with their
  providers and context windows, marking those cooling down; typing filters
  them, and `Enter` on an ID the list does not hold takes it. `/model <id>`
  chooses one at once. The choice stays with the session: a session without
  one runs with the model its last prompt ran with, a new one with the
  connection's (or `-model`'s). A prompt queued while the agent works keeps
  the model chosen when it was queued; a forced one is read by the running
  agent, with its model. Every prompt's header shows the model that answered
  it, with `⇄` where it changed. See the repository's
  [README](../../README.md#models).
- The pointer shows what a click does: the row under it lights up and the
  status line says what, with a hand over what can be clicked and an I-beam
  over text in terminals that let applications set the pointer (kitty,
  Ghostty, foot, xterm and others). A click on a prompt's header edits it;
  one on a block's first line folds or unfolds it (a folded block takes a
  click anywhere); one in the composer puts the cursor there; the
  scrollbar scrolls to where it is clicked or dragged.
- Dragging with the mouse selects text, and letting go copies it; the
  bottom line says how many characters. A selection started in the
  composer stays in it and copies the text as typed; in the transcript it
  copies what the screen shows, leaving out the column of times and rails
  when it starts past it, and the cards' edges. Copies go to the system
  clipboard, or through OSC 52 over SSH.
- `Ctrl-Y` copies the last answer, `Ctrl-N` starts a new session.
- `PageUp/PageDown`, `Shift-↑/↓` and the wheel scroll; `Home/End` jump when
  the prompt is empty. New output below the fold is counted in the status line.

Commands (`Tab` completes):

| Command | |
| --- | --- |
| `/new`, `/sessions`, `/resume <id>` | start, choose or resume a session; IDs may be shortened to a unique prefix |
| `/continue` | pick up a run that was stopped or did not finish |
| `/queue [resume\|pause\|clear]` | give the queue the keys, or run, hold or drop what waits for the agent |
| `/compact [focus]` | summarize the conversation to free context; the next prompt starts from the summary, and `focus` says what the summary should keep in view |
| `/edit [n]` | edit prompt `n`, by default the last, and run it again from there |
| `/rename <title>` | title the session; `/rename` alone puts the current title up for editing, `/rename -` restores the first prompt |
| `/pin` | keep the session at the top of the list |
| `/fork <n>`, `/fork` | branch before prompt `n` with that prompt ready to edit, or duplicate the session |
| `/export [path]` | save the session as Markdown, by default in the workspace |
| `/delete` | delete the session, after asking |
| `/settings` | the connection: provider type, base URL, API key, model |
| `/effort <level>`, `/model [id]` | effort (`low` to `max`; `/think` too), and the model of the session's next prompts; `/model` alone lists the models |
| `/plugins [trust\|untrust\|reload]` | the workspace's [plugins](../../docs/plugins.md); `trust` runs the workspace's own, whose commands then join these. Plugins that come, change or go are taken up by themselves within a couple of seconds |
| `/changes` | open or close the changes panel, as `Ctrl-G` does |
| `/inline`, `/fullscreen` | switch the layout, as `Ctrl-F` does |
| `/history`, `/copy`, `/expand`, `/collapse`, `/clear`, `/help`, `/quit` | |

Long sessions compact on their own: before a turn that would come within
33,000 tokens of the model's context window, the runner has the model
summarize the conversation and goes on from the summary, and the transcript
shows `Compacting context` and then a notice that opens the summary (see the
runner's README for the settings). `/compact` does it on request.

Start-up flags pick the session: `-continue` resumes the most recently
updated one, `-resume` opens the list, and `-session <id>` resumes a session
by ID or unique prefix (an unknown ID starts a new session under it). A
session whose last run did not finish says so in the status line, and
`/continue` picks it up.

## Connection settings

`/settings` chooses the provider type: **Responses** (the OpenAI Responses
API, or an endpoint implementing it), **Messages** (the Anthropic Messages
API, or a gateway implementing it), or **Environment** (the runner's
`KOU_CONVEYOR_LLM_*` variables). `Tab` moves between the base URL, API key
and model, `←/→` changes the type, `Ctrl-T` checks the connection by listing
the endpoint's models, and `Enter` saves. Empty fields use the provider's
defaults; an empty key field keeps a saved key, and `Ctrl-D` there removes
it. The key is never shown, and it is only sent to the endpoint it was
entered for.

Settings are shared with `kou-conveyor-web` and stored with mode `0600` in
the user configuration directory, or at `-config` / `KOU_CONVEYOR_CONFIG`.
They apply from the next run; `-provider` takes precedence over them.

## Sessions and history

Only one run can use a session at a time. The UI takes an advisory lock on the
session for as long as its runner lives, so the web cockpit or a second
terminal gets a clear "session is busy" message instead of interleaving writes.
A session that another window is running opens as "in use" and is followed
until that run ends. Switching sessions waits until the current run has
stopped, and stopping a run also stops the commands it started. Titles and
pins are kept in the sessions directory's `.meta`, beside the runner's files.

Prompt history is stored at `<workspace>/.harness/ui-history.json` with mode
`0600`; terminals sharing a workspace merge their history instead of
overwriting it. Session files and runner JSONL logs are written by the runner
exactly as when it is invoked directly. Use `-runner` when the runner is not
on `PATH` or next to the UI binary.

For scripts and pipes, use one-shot mode:

```sh
bin/kou-conveyor-tui -workspace . -p 'Explain the test setup'
bin/kou-conveyor-tui -workspace . -continue -p 'And the build?'
bin/kou-conveyor-tui -workspace . -continue -model grok-4.7 -p 'Review that'
bin/kou-conveyor-tui -workspace . -continue -p '/compact the build'
```

A prompt goes on with the model the session's last prompt ran with unless
`-model` names another, of any provider the connection reaches. The last
one compacts the session and prints the summary; notices of compactions go
to stderr.
