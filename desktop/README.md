# kou desktop

A native window for running several coding agents at once, drawn by GPUI. The
agent itself stays in Go (`cmd/kou-conveyor-runner`); this is everything around it.

- **Themes.** Settings → Appearance offers Charcoal, AMOLED Black and White.
  Create a theme, edit its colours with a preview, then save it to your library.
  Copy a theme link to share it; import a link to preview and apply someone
  else's palette. Existing installations keep their system appearance until
  a theme is selected. [Architecture and theme format](docs/themes.md).

- **Spaces and sessions.** A space is a project folder. A session is a piece of
  work there, with the agents working on it.
- **Panes.** Each agent has a pane. An agent opens as a chat in the app's own
  conversation view wherever it can be driven that way: kou's own agent
  through its runner, the others through the Agent Client Protocol (OpenCode,
  Gemini and more speak it themselves; Claude Code and Codex through an
  adapter by default). Settings → General also offers a direct connection
  to either CLI. The harness's own terminal interface is one click away in the pane's header
  (Cmd+Shift+I), with the conversation carried over where the harness keeps
  one session for both; a chat nobody has said anything in yet has no
  conversation to carry, and its terminal starts anew. A plain shell is a
  terminal.
- **Earlier conversations.** `/resume` or `/sessions` in a chat lists the
  agent's sessions for the folder (asked of the agent itself, over the
  protocol it is talked to in) and opens the one picked; `/resume <id>`
  opens one by its id. A session Claude Code runs in the background can be
  opened in one place only, a terminal attached to it (`claude attach`):
  asked for in a chat, it opens there, and a pane showing one stays a
  terminal.
- **Tabs that split.** A session's panes are tabs, and any pane can be divided
  with Cmd+D (to its right) or Cmd+Shift+D (below it): the new half shows what
  can be started there, and Escape gives the room back. The lines between
  panes drag, and Cmd+` goes round the panes of a tab. A program that exits
  takes its pane with it.
- **A prompt first.** An empty session and a new tab open on a prompt field
  and nothing else, the same field a conversation has (pictures, `$` file
  links, history): what is typed there is the first prompt of the agent
  Enter starts, and Enter on the empty field starts it with nothing to say.
  Which agent that is stands in the field's foot (the one started last, to
  begin with); a click on it opens the list of what can be started, each as
  a chat or in its terminal. Beside it is the model the agent will run
  with, chosen from what the harness offers a new session: kou says from
  its connection, another agent is asked through its chat adapter (a
  session is opened and left; nothing is said in it).
- **Each tab is also a canvas** of its own (Cmd+Option+2, and Cmd+Option+1
  back; Cmd with a digit alone shows the tab at that place, and Cmd+Left and
  Cmd+Right walk the tabs): the tab's
  panes, and no others, as nodes with their live interfaces inside them at any
  zoom at which they can be read; further out, nodes are their names. What is
  inside a node is drawn at the zoom, text included, on every frame of a
  zoom: a terminal keeps its grid and a conversation its line breaks. A node added on a tab's canvas is a
  pane of that tab.
- **A new tab before its agent.** The plus asks what the new tab is to be.
  Agent (Cmd+T) opens a tab with a prompt field, to say what is wanted and
  choose who gets it. It sits in the strip with the others, can be left and
  come back to, and there is only ever one; Escape or its cross closes it.
  Shell (Cmd+Option+T) opens a terminal at once: a prompt is nothing to give a
  shell, so the field never goes to one, and Shell in the field's own list
  starts it there and then.
- **Names.** Cmd+R, or a double click on a tab, names it and gives it one of
  sixteen icons. A double click on a session or a space in the rail renames
  it where its name stands. Agents can do the same from the command line.
- **A canvas that is wired.** Nodes have ports. An edge from one node's output
  to another's input carries what the first puts out into the second: at once,
  after you approve it, or not at all. Add nodes and whole templates from the
  board, rename, duplicate, undo.
- **Orchestration.** An agent in a pane starts, prompts, reads and waits for
  agents in other panes. How it is given the means is a setting: a skill and
  the app's command line (the default: the guide is read when a task calls
  for it), the app's tools through an MCP server (`kou-desktop mcp`), or
  nothing. Either is attached to that one launch; a harness's own
  configuration is never written to, and a harness started outside the app
  sees none of it. Whichever it is, an agent is told from the start, in a few
  lines, where it runs: that it is a pane of this app, that a tab is also a
  canvas whose nodes are the tab's panes, and how to look (`"$KOU_BIN"
  where`, or the `where_am_i` tool). A skill alone would not say so: a
  harness lists a skill by its description and reads it only when a task
  seems to fit, so an agent asked about "the canvas" had nothing to go on.
  This holds for an agent in any tab, shown as panes or as a canvas, and
  reaches a harness the way that harness takes it: with the guidance it is
  given at launch (Codex, OpenCode, Copilot, and the terminal interfaces of
  Cursor, Pi, Aider and Grok Build), as a few lines beside its skill (Claude
  Code, Grok Build's chat), and for any other chat as added instructions
  when its session opens or, where the agent takes none, as a marked note
  with its first prompt, which the chat does not show as yours. Not reached
  yet: kou's own agent, whose runner has no way to be given instructions for
  one run, and the terminal interfaces of Gemini CLI, Qwen Code, Amp, Droid,
  Goose and Kiro, which take none at launch either.
- **An agent's own agents.** An agent that a harness starts by itself (Claude
  Code's Agent tool) is shown in its chat as an agent, not as a tool call: a
  card with what it was started for, its kind, how many calls it has made, how
  long it has been at it and how it goes (running, in the background, needs
  you, done, failed), and under that its latest on one line. Open, the card
  holds what it was asked, what it did and said in order, and what it
  reported. Those at work are named on the line over the field and listed in
  the card beside the transcript. A call of such an agent that waits for your
  answer stands in the transcript with its answers. Claude Code's adapter is
  asked for these agents' words (`forwardSubagentText`); one left to run in
  the background counts as running until the turn that started it ends, which
  is when the adapter lets that turn end.
- **In the background.** A command an agent leaves running in the background
  (Claude Code's Bash with `run_in_background`) is not shown as a command that
  is over: its card stays in sight with "In background" and how long it has
  run, the line over the field says what runs in the background also while
  the agent is idle, and the card beside the transcript lists it. When it
  ends its card says that it ran in the background. For this Claude Code's
  adapter is told that the app shows commands as terminals (`terminal_output`),
  which is what makes it say when such a command ends; commands it runs in
  the ordinary way gain their exit status from the same. When the agent goes
  on by itself after something it left running ends, with no prompt of yours,
  that is a turn like any other: the chat shows it at work, and it can be
  stopped.
- **Claude Code, directly.** Settings has "Claude Code's chat": through its
  adapter (as it is by default), or directly. Directly, the app runs `claude`
  itself and speaks its stream of JSON lines, as the Agent SDK does
  (`kou-claude`): no adapter is needed or started. What the adapter had to be
  asked for, or the app had to guess, is said there outright: which calls
  went to the background and when each ends (an agent as exactly as a
  command), when the session goes on by itself after one ends, and when it is
  done for good, which is where the turn ends. Between the answer and that
  end a prompt goes at once and stops nothing. A message sent into a turn at
  work is taken into it; a sent prompt is edited by starting Claude Code anew
  from the message before it; earlier conversations are listed and shown from
  Claude Code's own records (all of a long conversation, across its
  compactions). The pane's account, proxy, the app's tools and its skill go
  to it as they go to its terminal. The choice holds for chats opened from
  then on. Its modes are Claude Code's own, "Auto" where the model takes it
  and "Bypass permissions" as through the adapter (Claude Code refuses it
  where its settings forbid it); the levels of effort the model takes have a
  control of their own in the field. What Claude Code asks you with its
  question tool, and what an MCP server asks with a form, is asked in the
  call's card and answered there or in the field; the name Claude Code gives
  the conversation is the chat's; a search or a page the API fetches itself
  is a call like any other.
- **Codex, directly.** Choose **Settings → General → Codex's chat → Directly**
  for new chats. The pane starts `codex app-server` with its working folder,
  account, proxy and orchestration settings. It uses `thread/start` or
  `thread/resume` for the conversation and `turn/start`, `turn/steer` and
  `turn/interrupt` for input. Messages, tool output, approvals, questions
  and token usage appear in the native chat. Pick **Permissions** in the
  launcher to set access before the first prompt. In a chat, the access
  control or `/permissions` changes it for subsequent turns. **Auto** allows
  workspace edits and asks about additional access. **Approve for me** uses
  Codex's automatic reviewer where available. **Read Only**, **Full Access**
  and named profiles follow the account and project policy. The separate
  **Default** and **Plan** control sets the collaboration mode. The CLI also
  supplies models, reasoning efforts and session history. Editing the
  first prompt of a turn branches the conversation at that turn. Codex
  accepts additional messages during a turn and can fork only at turn
  boundaries. On Unix, **With the app closed → Keeps working** lets a turn
  finish or wait for an answer while kou is closed. Reopening the pane
  restores the thread and any outstanding requests.
- **With the app closed.** A Claude Code the app talks to directly is
  started by a keeper, a small process of its own (`kou-desktop
  claude-host`, see `kou-claude`'s `host`), not as the app's child. Quitting
  the app, or rebuilding its window, only lets go of it: a turn under way is
  finished, what runs in the background runs on, a question waits. Opened
  again, the app finds each pane's keeper by the name the pane has for it
  (sockets beside the app's own, `<socket>.hosts/`), shows the conversation
  from Claude Code's record and stands where Claude Code stands: at work,
  waiting for an answer, or idle. Closing the pane, `/new`, another
  conversation or the terminal view end it. With nobody there, a Claude
  Code that only waits is let go after ten minutes (it is gone on with from
  its record) and any after a day. Settings, "With the app closed", turns
  this off: Claude Code is then the app's child and ends with it. Quitting
  does not warn of agents at work that are kept this way. A chat whose
  connection to its keeper is lost (the keeper died, or dropped an app that
  took nothing from it for a minute) is opened again by itself, once a
  minute at most: it finds the keeper if it still runs, else goes on from
  the record. Keepers run from a copy of the app's program that is that
  build's alone (`bin/claude-host-<size>-<time>` under the app's home), so
  another build of the app started under the same home cannot leave them
  unable to start. What befalls each keeper (started, a client came, left,
  was replaced or dropped, stopped, ended, and why) is written to
  `<socket>.hosts/keepers.log`.
- **Plugins.** Parts of the interface are drawn by scripts that the app reads
  from disk and reads again as they change, in a release build as in any
  other: see [Plugins](#plugins).
- **Surfaces.** What kou's agent shows with its `UI` tool (the
  [genui](../examples/plugins/genui) plugin of the runner: metrics, charts,
  tables, diffs, inputs, buttons, and boxes and text laid out as the agent
  likes) is drawn in the chat, in the card of the call that made it. The
  newest version of a surface is live: it reads its files again every second
  and a half while it is on screen, its fields are typed in where they stand,
  and a button tells the agent, as a message of yours
  (`[UI bench v1] apply {…}`). It stays in sight when the turn's work folds
  away; earlier versions fold with the work and open to what their call read.
  A chart says what is under the pointer. An `Html` component, a page of the
  agent's own, is not drawn (the app has no web view): it shows what the
  agent said it is. Only an agent with the `UI` tool makes surfaces, which
  today is kou's own with the plugin installed.
- **Told when it matters.** A notification when an agent you started finishes
  out of sight, and when any agent stops for an answer; each can be turned
  off. They are the system's own, in the app's name, and a click on one
  brings the app forward on the pane it is about. A space and a session show
  in the rail that something in them works or waits.
- **Several accounts of a harness, as one pool.** Settings, Harnesses, "Add
  an account" under a harness that keeps its home in a folder it can be told
  (Claude Code: `CLAUDE_CONFIG_DIR`; Codex: `CODEX_HOME`; Grok Build:
  `GROK_HOME`) opens a small form: a name, and a proxy if the account is to
  go out through one of its own. "Add and sign in" makes the account's folder
  in the app's own (`accounts/<harness>-<id>`), and opens the harness's own
  sign-in (`claude auth login`, `codex login`) in a pane, with that home and
  that proxy: the page it opens in the browser and the code it may ask for
  are yours to deal with there. "Use a folder I have" takes a home that is
  signed in to already. An account's row says what is left of its limits or
  that nobody is signed in, and has its sign-in, the pencil that changes its
  name and proxy, and the cross that forgets it (its folder stays).
  - *The proxy* is an HTTP or HTTPS one, written `host:port`,
    `host:port:login:password`, `login:password@host:port` or as an address;
    SOCKS is refused, since the agents do not take it. Everything started
    under the account gets it in its environment (`HTTPS_PROXY`, `HTTP_PROXY`,
    both cases, with `NO_PROXY` for what is local): the chat, the terminal
    interface, the question about its limits, the sign-in, and whatever
    those programs start. The address is shown and saved without its
    password; with one it is kept whole in `accounts/secrets.json`, a file
    only you can read, apart from the state. A program that ignores these
    variables goes out directly, and so does the page a sign-in opens in
    your browser.
  - *One store of conversations.* A folder the app makes for a Claude Code
    account has `projects`, `file-history` and `todos` linked to those of
    Claude Code's own home (nothing is written there by the app). A
    conversation begun under one account can so be gone on with under
    another: the account in a pane's header is a button that moves the pane
    to the account with the most left and opens the same conversation there.
    A folder of your own keeps its conversations to itself, and its panes
    stay where they are.
  - *Who runs what.* With "Accounts: One pool" (the default) a new agent,
    yours or one an agent starts with `pane spawn`, runs under the account
    with the most left of its tightest limit (the five hours, the week), less
    what that account already runs, so several started together are spread;
    an account that is spent or signed out is taken only when all are, and
    one named in the field's list is used as named. The empty page's field
    says `auto` then. With "As chosen" a prompt goes to the account you
    choose, else to the one the harness started last.
- **Usage.** Cmd+U, or Usage in the rail, shows a page where the workspace is,
  as Settings does (the same key, or Escape, goes back). It has a part for each
  provider: a card for each of the plan's limits (the 5-hour window where the
  plan has one, the week, the week for a model) and what the open chats have
  spent. A card says what is left of that limit over all the provider's
  accounts and what the first reset gives back, and draws a bar for each
  account: filled by what the account has left, the rest hatched, its name and
  share written in it, a line where the window's time stands (a fill that ends
  right of the line is spent slower than the time goes), and at its end the
  time until it starts over. The limits are asked of the agents' own programs,
  without a prompt (see `kou-usage`): when the page opens, every five minutes
  while it is open, and on Refresh.
- **Scrolling glides.** A wheel's steps glide everywhere: lists, a
  conversation, a terminal's history, and the canvas, where they pan and,
  with Cmd, zoom. A trackpad moves things as the fingers do.
- **Busy or free.** The rail lists every agent by what it is doing. Harnesses
  with hooks report it themselves; a shell is asked whether a command has its
  terminal; the rest are read from their output. Hooks do not tell all of it:
  a session whose turn has ended while agents it started in the background
  still work says it is done. So a harness's own status line is read too,
  about once a second while its screen moves, and a pane whose hooks say idle
  shows as working for as long as that line says so. The other way round
  too: a turn stopped by hand fires no hook, and is over once the line has
  been gone for three seconds. A hook only tells the app; it never waits
  for its answer. A shell the app started marks its commands, so a command
  handed to it (`pane send`) is over when the shell says so, not when the
  screen falls quiet.

GPUI comes from the gpuimm fork (`github.com/gfhfyjbr/gpuimm`) as a git
dependency: `Cargo.toml` names the commit and cargo fetches it, so a clone, a
worktree and the build service build with nothing set up. The fork has Glimm,
its distortion effects; the app uses none of them at present, and makes no
sound.

To work on the fork and the app at once, `script/gpuimm link` builds this
checkout of the repository against a checkout of the fork on the machine
(`~/Projects/Rust/gpuimm` unless another is given), uncommitted changes
included. It writes a cargo config that git ignores, `.cargo/gpuimm.local.toml`:
a `[patch]` taking the fork's crates from the checkout, and a lock file of its
own for such builds. `Cargo.toml` and `Cargo.lock` stay as committed.
`script/gpuimm` alone says which of the two the app is built against. Two
things follow from building linked:

- A dependency added or changed while linked lands in the linked lock file
  only. `script/gpuimm lock` writes it into `Cargo.lock`; `script/gpuimm
  status` says when that is due, and the build service refuses a `Cargo.lock`
  that is behind.
- What the app comes to need from the fork is nobody else's until it is
  pinned: commit it in the fork, push it, and run `script/gpuimm pin`, which
  moves the commit in `Cargo.toml` and `Cargo.lock` and refuses one GitHub
  does not have.

The terminal is Ghostty's: `libghostty-vt` 1.0.0 (terminal state, parsing and
the kitty graphics protocol), drawn by the app itself so that a terminal can
sit inside a canvas node, under a menu, at any zoom. Its source is vendored in
`crates/ghostty-vt-sys/vendor` and built by that crate's build script with Zig
0.17, found as `$ZIG`, as a `zig-*-0.17.*` folder under `desktop/vendor/`, or
as `zig` on the path. Nothing is downloaded during a build; a missing Zig
fails with a message that says what to put where.

A shell redraws its prompt whenever its pane changes size, and a terminal that
re-wraps its lines keeps what is left of the old copies, which then run
together beside the new prompt. Two things prevent that. zsh is started so that
it marks its prompts (OSC 133, with `redraw=1`), through a `ZDOTDIR` of the
app's own whose one file puts your `ZDOTDIR` back and reads your `.zshenv`
before anything else; the terminal then clears a marked prompt before it
resizes. And a pane being dragged tells its program a new size at most every
90 ms, not on every frame. Other shells are started as they are.

Harnesses are shown by their own marks where Simple Icons has one (Claude,
OpenAI for Codex, Gemini, OpenCode, Cursor, GitHub Copilot, Qwen; CC0, see
`crates/kou-ui/brands/README.md`), and by two letters otherwise.

The icons are Lucide's (ISC; `crates/kou-ui/icons/README.md` says which of
Lucide's each file is, and its licence is beside it): round caps and joins,
drawn at 16 pixels where an icon leads a row or fills a toolbar's button,
and at 14 where it is a detail of a row (what closes a tab, what opens a
group, a row's own action, the icon in a text button). The few Lucide has no
drawing for are drawn in the same line.

## The prompt field

A native conversation pane has what kou's web cockpit has:

- `$` links a file of the project (ranked as you type, `$path:120-160` for
  lines, folders open); linked files are chips over the text.
- `/` lists commands: the agent's own, then the view's (`/model`, `/mode`,
  `/effort`, `/queue`, `/stop`, `/edit`, `/history`, `/copy`, `/clear`,
  `/help`) and the app's (`/new`, `/sessions`, `/resume`, `/rename`,
  `/settings`).
- The model's and the mode's controls stand in the field's foot. An agent
  that offers levels of effort apart from its modes (Claude Code, directly)
  has a third between them: the level in force as bars filled to it, drawn
  as kou's own efforts are, which opens the list of levels (`/effort`, or
  `/effort high`). kou offers its efforts as its modes, and `/effort` is its
  `/mode`.
- A call that asks you questions (Claude Code's question tool, directly)
  shows them in its card, where a request's buttons stand: one at a time
  ("2 of 3"), what was answered before it a line each above it, with Back.
  A click on an option answers a question that takes one; one that takes
  several toggles them and has Confirm. Over the empty field, digits 1 to 9
  choose (or toggle) the option of that number and Enter confirms. Where a
  question takes your own words, what you send from the field is its answer
  (with the options chosen), and the field says so; the view's and the
  app's commands and the stop work as ever. Its refusals ("Skip") are
  quiet buttons. Once the last question is answered all the answers go
  back together; a request the agent takes back closes its card.
- Cmd/Ctrl+V pastes a picture; so do a drop and the paperclip. Pictures are
  `[Image n]` in the text with thumbnails above it.
- Up and Down walk the prompt history kou's cockpits share; Ctrl+R searches it.
- Enter during a turn queues the prompt; Cmd/Ctrl+Enter forces it into the turn.
  kou's own agent and Claude Code take it there (Claude's adapter through its
  steering request, `_session/steering`): nothing is stopped, the agent reads
  it at its next step, and what it left running goes on. An agent that takes
  nothing while it works is stopped to take it, and the prompt goes into the
  same session. Once the agent has given its answer and
  only waits for, or works on, what it left running in the background (the
  agents and commands Claude Code starts), a prompt goes at once, by Enter as
  by the force key, and nothing is stopped for it: that work goes on. Claude
  Code takes such a prompt into the turn it holds open, which stays open for
  that work afterwards: the chat goes on showing the agent at work and the
  agents it started as running. Stop still stops all of it.
- Edit on a prompt you sent (or `/edit`, `/edit n`) opens it where it stands,
  with its pictures; what comes after it fades. Enter runs the edited prompt
  from there: the session goes back to before it, as in the web cockpit
  (files the agent changed keep their changes), and Esc leaves it as it
  was. kou's own agent does it in its session. Claude Code and Codex do it
  through their adapters, by branching: the session is forked just before
  that prompt and the conversation goes on in the fork, so the pane has a
  new session id and the old session stays in the harness's own list (the
  "(fork)" Claude Code puts after the fork's title is not shown). An
  agent that can do neither (OpenCode, which only copies whole sessions,
  and the rest) offers Reuse instead, which puts the prompt back in the
  field.

## Run

```sh
cd desktop
export SDKROOT="$(xcrun --sdk macosx --show-sdk-path)"   # macOS
cargo run -p kou-desktop -- --open ~/some/project
```

`kou-desktop --help` lists the app's flags and the client's commands.

## Working on it while it runs

```sh
script/dev                       # or: script/dev --open ~/some/project
```

This starts the app under the Dioxus CLI's hot-patching (Subsecond). From then
on a saved change to the code of any crate of this workspace reaches the
running app by itself, in one of two ways. `script/dev` says which for each
change.

- **A change inside functions** is compiled into a patch and swapped into the
  running process, usually in about a second: the window redraws with the new
  code and nothing else happens.
- **A change to anything else** (a field added to a struct, a variant to an
  enum, a signature, a constant) changes the shape of what the window holds,
  which a patch cannot do under it. So the window's content is rebuilt around
  the patch: taken down by the code that made it, put up again by the new
  code, in the same window, which stays open. Tabs, splits, canvases and
  names come back as they were. A program in a terminal pane does not notice:
  its terminal lets go of it and a new one carries on with it, with the
  screen it had. A native conversation is started again where it left off,
  by its session; a turn that was running when the change came in is lost.
  The app tells the two kinds of change apart from the source
  (`crates/kou-desktop/src/outline.rs`).

An agent working on the app from inside one of its panes changes the app
around itself.

What still needs `r` in `script/dev` (rebuild and start again, which ends the
programs in the panes):

- **A new dependency, a changed `Cargo.toml` or build script, a change in the
  GPUI fork**: these are not patches at all.

And two things to know about patches:

- **Statics** of a patched crate start over in the patch (a cache is empty
  again, a counter restarts).
- A callback registered before the patch keeps its old body unless it goes
  through `hot!`; the app's own do. See `crates/kou-desktop/src/hot.rs`.

The CLI is expected at `desktop/.tools/bin/dx` (`cargo binstall
dioxus-cli@0.7.10 --install-path desktop/.tools/bin`; the folder is ignored by
git). Its version and the `subsecond` crate's are pinned together. The first
run compiles the dependencies once more, under the `hot` profile (about 3 GB).
A plain `cargo run` builds without any of this.

## Plugins

A plugin is a folder with a `plugin.json`, as the browser cockpit's are; what
it gives this app is named under `desktop`:

```json
{ "name": "hello", "desktop": { "script": "desktop/main.js" } }
```

The script is JavaScript, run by an interpreter that is part of the app
(QuickJS, through `rquickjs`), so nothing is compiled when it changes: the
app looks at its plugins' folders five times a second, and a saved file is
read again and takes the old version's place, with the window left as it is.
A script that fails to load leaves the plugin running as it was, and the
window says what is wrong and where; one that throws while it draws leaves
its page as it was last drawn.

Plugins come from three places, a later one of the same name taking an
earlier one's place: the app's own, compiled in (`desktop/plugins`); the same
folder of the checkout the app was built from, read live while it is still on
disk; and `plugins/` in the app's folder (`~/Library/Application
Support/kou-desktop/plugins` on macOS), which is the user's. The Usage page
is the app's own `usage` plugin (`desktop/plugins/usage`): put a folder named
`usage` in your plugins to draw it your way, from the same data.

What a script has:

```js
kou.page({
  id: "hello",            // a row of this name appears in the rail
  title: "Hello",
  icon: "note",           // an icon of the app's, by its file's name
  render(ctx) {           // ctx.data, ctx.theme (colours, metric), ctx.state
    const count = (ctx.state && ctx.state.count) || 0;
    return ui.col({ p: 24, gap: 8 }, [
      ui.text("Clicked " + count + " times", { color: "fg" }),
      ui.button("More", {
        onClick: (api) => api.setState({ count: count + 1 }),
      }),
    ]);
  },
});
```

`ui.box`, `ui.row`, `ui.col`, `ui.text`, `ui.icon`, `ui.button`,
`ui.monogram` and `ui.mark` build a tree that the app turns into its own
elements; the properties it reads (flex and grid layout, sizes, padding,
colours by the theme's names or `#rrggbb`, borders, corners, text, `hover`,
`tooltip`, `onClick`, `onPoint`) and the other kinds of element
(`ui.el("input", …)`, a field typed in where it stands; `"plot"`, a chart's
marks; `"image"`; `"markdown"` and `"code"`) are listed at the top of
`kou-desktop/src/plugin_view.rs`. A handler gets `api.setState(patch)`, which
is kept across reloads, and `api.act(name, args)` for what the app does
itself: `usage.refresh`, `pane.focus`, `page.show`, `page.close`, `notice`,
`clipboard.copy`.

A plugin of several files lists them, `"desktop": { "scripts": ["shared/
data.js", "desktop/main.js"] }`: they run in that order as one program, and
a file written as a module of exports (one shared with the browser cockpit)
runs with its exports as plain names. A script cannot `import`.

Besides pages a plugin can add a view, `kou.view({ id, render(ctx) })`:
something the app draws where it has a place for it, once for each thing it
has to show, each with its own data and state. The surfaces in a chat are
the `genui` view of the app's own plugin `desktop/plugins/genui`: the app
gives it a surface's version and files, and it builds the card's inside.
Put a folder named `genui` in your plugins to draw surfaces your way.

The app's own interface is drawn the same way, by plugins of its own in
`desktop/plugins`, each a folder you replace with one of yours of the same
name. The app decides what is shown and does what is asked; the script
decides how it looks and where everything goes. What a script cannot draw
stays the app's and is placed with `ui.slot(name)`: what is typed, a
terminal, prose and code as they are shaped, and whatever moves as it is
watched (a mark that blinks, a card opening, the highlight a list's keys
move). Each script says at its top what its views are given in `ctx.data`,
which slots they have, and what they ask of the app with `api.act`.

| Plugin | Its views |
|---|---|
| `rail` | the rail (`rail`) |
| `workspace` | the window layout (`shell`), plugin page wrapper (`pluginPage`); the tab strip (`tabs`); a pane's header and frame (`paneHeader`, `paneFrame`); the panel a tab is named in (`tabEditor`); the header of a split's half that waits for its agent (`choosing`); the line the window says things on (`notice`); a tooltip's body (`tip`) |
| `screens` | what an empty window and a stopped pane say (`noSpaces`, `noSession`, `stopped`); what can be started (`picker`, `pickerScreen`, `askChooser`); a pane's earlier conversations (`sessions`); the settings and their navigation (`settings`, `settingsFrame`) |
| `canvas` | the bar in a canvas's corner (`canvasBar`); the empty board (`canvasEmpty`); menus (`canvasMenu`); the bar that takes a deletion back (`canvasToast`); the inspector (`canvasInspector`); a node's header, footer and own text, and the tab of what waits for it (`canvasHead`, `canvasFoot`, `canvasBody`, `canvasPending`); an edge's label (`canvasLabel`) |
| `composer` | the prompt field's frame and controls (`composer`); the line over it while the agent works (`composer-activity`); the queue's tray (`composer-queue`); the figures under it (`composer-usage`) |
| `chat` | a transcript's entries (`chat-user-head`, `chat-user-card`, `chat-answer-head`, `chat-thought`, `chat-tool`, `chat-plan`, `chat-notice`, `chat-worked`); a surface's card (`chat-app-card`); the line after the last entry and what `/clear` left (`chat-tail`, `chat-cleared`); the list that `/`, `$` and Ctrl+R open (`chat-menu`); the modes' list (`chat-choices`, `chat-choice`); the model picker's card and each of its lines (`chat-models`, `chat-model-line`); the help card (`chat-help`); the cards of agents the conversation started, by a call or in a pane (`chat-agent`, `chat-nested`); the card beside the transcript and the handle that pulls it out (`chat-beside`, `chat-beside-handle`); the ticks down the transcript's edge, one for each prompt (`chat-ticks`); the frames of what a tool call gave: a change's header and foot, the head of a command's output and of raw input or output (`chat-tool-piece`) |
| `appearance` | the theme library, colour editor, previews and theme links (`appearance`) |
| `studio` | Studio messages, answers, thought/tool headings, plans, notices, folded work, empty states, activity, usage and dock layout (`studio-*`) |
| `genui` | a surface's inside (`genui`) |
| `usage` | the Usage page |

The prompt field's `composer` view, for a chat's field and the empty page's
alike, is told its `look` (`cockpit` or `studio`), how the field stands (`focused`, `busy`, `closed`, `narrow`,
`standalone`) and, in `ctx.data.controls`, how its controls do: which
buttons there are (`send`, `stop`, `force`), whether what is typed can be
sent, the model's name and its provider's colour, the mode's name or the
effort's level, and the level of effort offered apart from the modes
(`effort`, its list the slot `effortMenu`). An open list stands over its own
control, a few pixels above it: the model picker is the slot `modelMenu`,
the modes' list `modeMenu`. A control asks the field with
`api.act("field.send")` (queued while the agent works), `field.force`,
`field.stop`, `field.attach`, `field.focus`, `field.model`, `field.mode` or
`field.effort`. What is typed is `ui.slot("editor")`. A view of the chat's
asks it with `toggle`, `worked`, `permit`, `choose`, `confirm` and `back`
(an option of a question a call asks, its Confirm, the question before;
the card is told the questions as `asks`), `copy`, `edit`, `reuse`,
`unclear`, `unfold` (keep more of what a command printed), `nested` and
`show` (open an agent's pane, or its card), `beside` and `folder` (the card
beside, and its project's folder), `tick` (go to a prompt), or `back` from a
line of the model picker.

The app's own parts use more of the vocabulary than a page does, all of it
listed in the same place: `key` (which element this is from one drawing to
the next), `group` and `groupHover`, `float` (a panel over the rest),
`spring` and `drop`, `ring` (how a control looks while the keys are on it)
and `active` (while it is pressed), `onPress`, `onHover` and `onOutside`, a
border in the screen's pixels (`border: "1px"`, a hairline at any zoom; a
border given as a number grows and shrinks with a canvas's zoom, which the
app's own hairlines do not), colours written `mix(line, error, 0.6)`,
`alpha(accent, 0.12)` or `provider(openai)` (a provider's or a harness's own
colour), `ui.brand(harness, { code, size })` for a harness's mark, an
icon turned with `rotate` (degrees), and an icon button's two sizes: `size`,
its square (24 unless said), and `glyph`, the icon in it (16; 14, which is
`theme.metric.iconSmall`, in a square smaller than 24 unless said).

A script is run again only when what it is given changes: never at each
frame of a pan, a zoom or a scroll, and not as an answer's words come (what
streams is a slot). A canvas's parts are made once at 100% and drawn at the
board's zoom; the rail and the tab strip are drawn again only when what they
show changed. Required window and settings views fall back to their embedded
JavaScript when an override is missing or fails. Other views retain their
native fallback, so a broken script leaves the editor usable.
[JS UI architecture and action contracts](docs/js-ui.md).

A script reaches nothing but this: no files, no network, no other programs.
It is stopped when it draws for longer than a quarter of a second or when
an interpreter holds more than 96 MB.

What is still the app's own code, which takes a build to change: the insides
of panes (the terminal, what is said in a transcript, what is typed), the
lines of a change, of a command's output and of an agent's steps, the rows
of the queue, the lines between split panes and the rail's edge (they are
dragged), and on a canvas its zoomed-out placeholders, ports, edges and the
grid. The question asked before busy
panes are closed is the system's own dialog.

## Agents talking to agents

The binary is also the client. Inside a pane `KOU_SOCKET`, `KOU_PANE`,
`KOU_SESSION` and `KOU_BIN` are set, so an agent can run:

```sh
"$KOU_BIN" harness list
worker=$("$KOU_BIN" pane spawn codex --prompt "review the diff in src/" --json | jq -r .id)
"$KOU_BIN" pane wait "$worker" --timeout 600
"$KOU_BIN" pane read "$worker" --lines 80
"$KOU_BIN" pane send "$worker" "now fix what you found"
```

A pane started from another pane is its child. When its parent's tab is shown
as a canvas it joins that canvas, beside its parent and linked to it; otherwise
it opens as a tab of its own. Either way it does not take the keys from where
you are; `--split` gives it half of its parent's room instead, to the
parent's right (`--split-down` below it, `--share 30` that many percent of the
room rather than half), and `--tab` makes it a tab and brings it to the front. `pane wait` exits 4 when it runs
out of time.

A harness that has a native chat opens as one; `--terminal` asks for its own
terminal interface, `--native` for the chat. `--title` names the pane, and
the name stays: what the agent later calls its session does not replace it.
`--model` takes one of the ids `harness models <harness>` lists, and an id
the harness does not have is refused; `pane info`, `pane list` and `where`
show the model a pane was given.

An agent can also name what it works in, so that you can tell the tabs apart:

```sh
"$KOU_BIN" tab rename api server          # the tab this pane is in; --tab ID for another
"$KOU_BIN" tab icon server                # `tab icons` lists them; auto is what runs in it
"$KOU_BIN" tab rename --clear             # called after its pane again
"$KOU_BIN" session rename parser rewrite  # --session ID for another
"$KOU_BIN" space rename client work       # --space ID for another
"$KOU_BIN" tab list --all                 # and `session list`, `space list`
```

An agent can ask where it is, which is what it does first when you speak of
the canvas, a node or an edge:

```sh
"$KOU_BIN" where          # or: where p7, for another pane; --json for the data
```

It prints the pane itself, its tab and how you see it (as panes, or as a
canvas), the other panes of the tab, and each edge of the tab's canvas with
what it does: made when one agent started another (it shows that and
carries nothing by itself), or wired, with its mode and whose answers it
carries to whom, and how many messages wait on it for you. Notes and sources
on the canvas are listed with what they say.

Edges are yours to draw, remove and switch. An agent sees them (`edge list`)
and cannot change them, unless you make its node the tab's orchestrator: a
switch in the node's inspector on the canvas, which you can turn off again.
An orchestrator wires the nodes of its own tab:

```sh
"$KOU_BIN" edge add p7 me --mode approve   # what p7 answers waits on me for you; me is its own pane
"$KOU_BIN" edge set p7 me --mode auto      # auto, approve or off
"$KOU_BIN" edge rm p7 me
```

A wire an agent drew says so on the canvas ("by lead" on the edge, and in the
edge's inspector), so you can tell it from your own. A wire that closes a ring
is made with a warning, and the ring cannot run away: a message knows how
many times it was passed on, agent to agent, says so from the second time
(`[canvas] from «lead» (p3), hop 3 of 6:`), and past six waits on its target
for you, as on an edge that asks first.

To hear one answer from another agent without a wire:

```sh
"$KOU_BIN" pane ask p7 "Which files did you change?"
```

The question goes to the pane, and what its agent answers at the end of that
turn comes back to the asker once, as a message beginning `[canvas] answer
from`. Nothing is wired, and the asker does not wait.

A `pane send` from one pane to another of the same tab travels the edge
between their nodes on that tab's canvas: the board counts it, shows it on
its way, and holds it on an edge you set to ask first until you let it
through from the node's badge. If they had no edge, a line is drawn that
shows one wrote to the other; it carries nothing by itself, so the sender's
later answers do not start going to the receiver. Between panes of
different tabs there is no edge, and the message simply arrives. An edge wired by hand also carries by itself: when a native
conversation ends a turn, its answer goes to every node its output is wired to.
The edge made when one agent starts another only shows who made whom.

## Crates

| crate | what |
|---|---|
| `kou-desktop` | the app: the window, the rail, layouts, state, and the socket's answers |
| `kou-ui` | theme tokens (with contrast tests), the icon set, small controls |
| `kou-canvas` | the board model and its view: nodes with ports, wired edges and their modes, templates, undo |
| `kou-term` | pseudo-terminal, Ghostty's emulator, and the view that draws it: text, pictures, the smooth cursor, zoom |
| `ghostty-vt-sys` | `libghostty-vt`, vendored and built with Zig, and its bindings |
| `kou-agents` | which harnesses are installed, how to launch them, hooks, busy tracking |
| `kou-ipc` | the socket protocol, its transport, and the command line client |
| `kou-thread` | the conversation model a native pane renders, the backend channels, and the project file index |
| `kou-thread-view` | the native conversation view and its prompt field: links, commands, pictures, history, queue |
| `kou-acp` | an Agent Client Protocol client as a `kou-thread` backend |
| `kou-claude` | Claude Code as a `kou-thread` backend with no adapter: its own stream protocol, and its records read for the session list and history |
| `kou-codex` | Codex app-server as a `kou-thread` backend, including live events, thread history and the model catalog |
| `kou-native` | kou's Go runner as a `kou-thread` backend |
| `kou-usage` | an account's plan limits, asked of the agent's own program (Claude Code's control protocol, Codex's app server, `grok usage`), and several accounts put together |
| `kou-plugins` | plugins: scripts read from disk, run in an interpreter inside the app, loaded anew as they change |

## Checking it

```sh
cargo test --workspace                      # unit and integration tests
script/e2e                                  # the real app, hidden, driven through its client
cargo run -p kou-desktop -- --scene shell-split --capture /tmp/frame.png
cargo run -p kou-desktop -- --scene motion-split --capture /tmp/split.png --strip 6 --every 40
```

A scene is a scripted state of the interface; `--capture` draws it in a hidden
window and writes a picture. The `canvas-*` scenes stop their transitions at
`--time` milliseconds; the `motion-*` scenes run in real time, and `--strip N`
writes N frames `--every` milliseconds apart. `kou-desktop --scene x` lists the
scenes.

The build directory is the repository's `tmp/rust-rewrite-target`, set by the
root cargo config.

Anything that runs unattended (an agent's test runs, a loop of runs, a
benchmark) goes through `script/capped -- <command>`. macOS limits no process's
memory, so a test that allocates in a loop swaps until the disk is full and the
machine stops. `script/capped` ends the command and everything it started when
one process passes 8 GB, all of them pass 24 GB, 30 minutes go by, the command
leaves processes behind, or the shell that ran it is killed. `--mem`, `--total`
and `--time` change the limits.

It also keeps the command out of the way of whoever is at the machine: at a
low priority, and with cargo running half as many compilers and tests at once
as there are cores (`--nice`, `--jobs`). A build takes longer and the machine
stays usable; `--eager` gives it the whole machine.

## State

`~/Library/Application Support/kou-desktop/state.json` on macOS (the platform's
data directory elsewhere), or `--home DIR`. Processes are not part of it: a
pane restored after a restart offers to start its harness again, resuming the
harness's own session where the harness can.

The accent (links, the focus ring, what is selected, an agent at work) is
orange unless another is chosen in Settings, Accent. The choice is kept there
as `"accent": "#rrggbb"`, and any colour written by hand is taken: on the
light ground it is darkened, and on the dark one lightened, until it reads as
text. The other states keep their colours: blue is done, amber needs you, red
failed.

## Notifications on macOS

macOS posts notifications only for an app it knows by its bundle. So a
window started from a bare program (`cargo run`, the build directory) first
puts itself into a bundle under its home, `KouDesktop.app` (a copy of the
program, an `Info.plist`, an ad-hoc signature), registers it with the system
and carries on as the program inside: same process, same terminal. A new
build is copied in at its first start, which takes a second or so longer.

The system asks once whether kou may show notifications. It asks in a banner
that goes away by itself, and an unanswered question counts as a no; the
settings then say that the system is not letting kou show them, with a
button to where that is changed (System Settings, Notifications, kou).

`KOU_NOTIFY_TRACE=1` prints what the system answers. To check any of this
without touching what the system remembers about the app, start the instance
with another identifier and only look: `KOU_BUNDLE_ID=dev.kou-conveyor.check
KOU_NOTIFY_TRACE=status kou-desktop --hidden --home DIR`, with `DIR` under your
home folder (the system refuses a bundle under `/tmp`), and take the bundle
out of the system's register afterwards (`lsregister -u DIR/KouDesktop.app`).

## The icon

One sign, a frame with the orange core inside it, drawn twice in
`assets/icon/`:

- `AppIcon.icon` is an Icon Composer document, for macOS. Its layers (the
  core, the frame, the core's glow on the ground) are plain SVG files; the
  glass is the system's, in every appearance (default, dark, tinted, clear).
  Open it in Icon Composer (Xcode, Open Developer Tool) to change it.
- `kou.svg` is the icon for Windows and Linux, where an icon is shown as it
  is given: the same sign with the light painted in.

`script/icons` makes from them what the app and its packages carry, kept in
the repository under `crates/kou-desktop/resources/`: the compiled document
and an `.icns` for macOS, an `.ico` for Windows, a hicolor icon folder and
the desktop entry for Linux. Sizes below 64 pixels are drawn by the script on
whole pixels instead of being scaled down. Run it after changing a drawing;
`script/icons --check` says whether the kept files are behind. It needs
`rsvg-convert`, Pillow, and on macOS Xcode 26 or later; building the app
needs none of them.

On macOS the icon goes into the app's bundle, the one it makes for itself
and the one in a package. On Linux a window names itself `kou-desktop`,
which is how a desktop finds the entry and the icon installed with it; under
X11 the window carries the icon as well. On Windows `build.rs` puts the icon
and the name into the program file.

## Packages and the build service

`script/package` builds the app for release and puts it into the file
people download, in `dist/`: on macOS `KouDesktop.app` in a disk image, on
Linux a `.tar.gz` with the program, its desktop entry, its icons and an
`install.sh` that copies them under `~/.local` or a prefix given to it.
`--skip-build` packages the release build already there.

The Linux program goes into its package without its debugging sections: a
release build keeps them inside the program there, 245 MB of a 297 MB file
when this was written, and the package is a quarter of the size without
them. The names a backtrace shows stay. The build service makes it on
Ubuntu 24.04, so it needs glibc 2.39 or later, and `libxkbcommon`,
`libxkbcommon-x11` and `libxcb` installed.

The macOS app is signed without a certificate unless `--sign` or
`KOU_SIGN_IDENTITY` names one. macOS refuses to open such an app once it
has been downloaded, until the mark the download left on it is removed:
`xattr -dr com.apple.quarantine KouDesktop.app`.

The `Desktop` workflow (`.github/workflows/desktop.yml`) runs that script on
macOS (Apple silicon) and on Linux (x86-64) for every push to `main` or
`rust-rewrite` and every pull request that touches `desktop/`, and keeps the
packages with the run for two weeks. A tag `desktop-v<version>`, where the
version is the one in `Cargo.toml`, also publishes them as a release with
their checksums. Cargo fetches the GPUI fork at the commit `Cargo.toml` pins;
the workflow fetches Zig, and builds with the Rust the fork's
`rust-toolchain.toml` names (`RUST_TOOLCHAIN` in the workflow).

There is no Windows package: the app does not build there yet. The terminal
(`kou-term`) has a pty on unix only, and the build script of
`ghostty-vt-sys` knows no Windows target (Zig did build the library for
`x86_64-windows-gnu` when it was tried, under the name `ghostty-vt.lib`).
