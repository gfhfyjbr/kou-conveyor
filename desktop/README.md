# kou desktop

A native window for running several coding agents at once, drawn by GPUI. The
agent itself stays in Go (`cmd/kou-conveyor-runner`); this is everything around it.

- **Spaces and sessions.** A space is a project folder. A session is a piece of
  work there, with the agents working on it.
- **Panes.** Each agent has a pane. An agent opens as a chat in the app's own
  conversation view wherever it can be driven that way: kou's own agent
  through its runner, the others through the Agent Client Protocol (OpenCode,
  Gemini and more speak it themselves; Claude Code and Codex through an
  adapter, which the app offers to install into a folder of its own). The
  harness's own terminal interface is one click away in the pane's header
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
  the app's command line (the default: nothing sits in its context until it
  is needed), the app's tools through an MCP server (`kou-desktop mcp`), or
  nothing. Either is attached to that one launch; a harness's own
  configuration is never written to, and a harness started outside the app
  sees none of it.
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
- **Plugins.** Parts of the interface are drawn by scripts that the app reads
  from disk and reads again as they change, in a release build as in any
  other: see [Plugins](#plugins).
- **Told when it matters.** A notification when an agent you started finishes
  out of sight, and when any agent stops for an answer; each can be turned
  off. They are the system's own, in the app's name, and a click on one
  brings the app forward on the pane it is about. A space and a session show
  in the rail that something in them works or waits.
- **Several accounts of a harness.** Settings, Harnesses, "Add an account"
  takes a folder: the harness run with its home there (`CLAUDE_CONFIG_DIR` for
  Claude Code, `CODEX_HOME` for Codex, `GROK_HOME` for Grok Build) has a login,
  sessions and settings of its own. The app writes nothing into that folder;
  signing in is done once, in a pane of that account. Each account is a row of
  its own where an agent is chosen, a pane's header names the one it runs
  under, and a prompt goes to the account used last until another is chosen.
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

## The prompt field

A native conversation pane has what kou's web cockpit has:

- `$` links a file of the project (ranked as you type, `$path:120-160` for
  lines, folders open); linked files are chips over the text.
- `/` lists commands: the agent's own, then the view's (`/model`, `/mode`,
  `/queue`, `/stop`, `/edit`, `/history`, `/copy`, `/clear`, `/help`) and the
  app's (`/new`, `/sessions`, `/resume`, `/rename`, `/settings`).
- Cmd/Ctrl+V pastes a picture; so do a drop and the paperclip. Pictures are
  `[Image n]` in the text with thumbnails above it.
- Up and Down walk the prompt history kou's cockpits share; Ctrl+R searches it.
- Enter during a turn queues the prompt; Cmd/Ctrl+Enter forces it into the turn.
- Edit on a prompt you sent (or `/edit`, `/edit n`) opens it where it stands,
  with its pictures; what comes after it fades. Enter runs the edited prompt
  from there: the session goes back to before it, as in the web cockpit
  (files the agent changed keep their changes), and Esc leaves it as it
  was. kou's own agent does it in its session. Claude Code and Codex do it
  through their adapters, by branching: the session is forked just before
  that prompt and the conversation goes on in the fork, so the pane has a
  new session id and the old session stays in the harness's own list. An
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
elements; the properties it reads (flex layout, sizes, padding, colours by
the theme's names or `#rrggbb`, borders, corners, text, `hover`, `tooltip`,
`onClick`) are listed at the top of `kou-desktop/src/plugin_view.rs`. A
handler gets `api.setState(patch)`, which is kept across reloads, and
`api.act(name, args)` for what the app does itself: `usage.refresh`,
`pane.focus`, `page.show`, `page.close`, `notice`.

A script reaches nothing but this: no files, no network, no other programs.
It is stopped when it draws for longer than a quarter of a second or when
the plugins together hold more than 96 MB.

So far a plugin adds or replaces a page. The rest of the window (the rail,
panes, the chat, the terminal) is still the app's own code, which takes a
build to change.

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

An agent can also name what it works in, so that you can tell the tabs apart:

```sh
"$KOU_BIN" tab rename api server          # the tab this pane is in; --tab ID for another
"$KOU_BIN" tab icon server                # `tab icons` lists them; auto is what runs in it
"$KOU_BIN" tab rename --clear             # called after its pane again
"$KOU_BIN" session rename parser rewrite  # --session ID for another
"$KOU_BIN" space rename client work       # --space ID for another
"$KOU_BIN" tab list --all                 # and `session list`, `space list`
```

A `pane send` from one pane to another of the same tab travels the edge
between their nodes on that tab's canvas (one is made if they had none): the
board counts it, shows it on its way, and holds it on an edge you set to ask
first until you let it through from the node's badge. Between panes of
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
