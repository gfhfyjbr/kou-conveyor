# Plugins

Everything the project adds is a plugin. A plugin is a directory with a
`plugin.json`: it can give the agent **tools** it calls, **skills** and
**instructions** for its system prompt, give the cockpits **slash
commands**, and give the browser cockpit a **script and a style sheet** that
build its interface.

The project's own features come the same way:

- The agent's `Bash`, `ViewImage` and `SkillUse` tools are the built-in
  **core** plugin, which can be turned off like any other.
- The browser cockpit is nothing but plugins. Its page is a small plugin
  host (the *kernel*, [`static/kernel`](../cmd/kou-conveyor-web/static/kernel))
  that builds nothing itself: the layout, the sessions, the transcript, the
  composer, the model picker, the accounts, the inspector, the palette, even
  the colours are built-in plugins
  ([`cmd/kou-conveyor-web/plugins`](../cmd/kou-conveyor-web/plugins)),
  written against the same API as yours. A plugin of yours can add to any of
  them, take any part's place, or replace a whole built-in plugin.

**Changes show at once.** A plugin whose files change is loaded anew while
the page stays open — its new version in, the old one taken away, the rest
untouched — and a running agent takes up changed tools, instructions and
skills at its next turn. Nothing needs a reload or a restart: not a user's
plugin, not a workspace's, not a built-in one (see [Changes while things
run](#changes-while-things-run)).

[`examples/plugins/git-glance`](../examples/plugins/git-glance) uses every
part of a plugin; [`examples/plugins/scratchpad`](../examples/plugins/scratchpad)
adds a view of its own to the cockpit, with a page, a panel and a hook.

## Where plugins live

| Source | Directory | Runs |
| --- | --- | --- |
| built in | compiled into the programs (the web cockpit's are also served from a checkout, live) | always, unless turned off |
| user | `plugins/` in the configuration directory: `~/Library/Application Support/kou-conveyor/plugins` on macOS, `~/.config/kou-conveyor/plugins` on Linux, or beside `KOU_CONVEYOR_CONFIG` | always, unless turned off |
| workspace | `.harness/plugins/` in the workspace | once the workspace is trusted |

Each plugin is a subdirectory with a `plugin.json`; a plugin being worked on
elsewhere can be linked in. A later source's plugin replaces an earlier one
of the same name: a workspace can bring its own version of a user plugin,
and a user plugin named `core` replaces the built-in tools — or one named
`composer` the cockpit's composer. Two active plugins cannot share a tool or
a command: the first keeps it and the conflict is reported.

The built-in plugins are `core` (the agent's tools) and the browser
cockpit's `accounts`, `changes`, `commands`, `composer`, `connection`,
`edit`, `effort`, `header`, `help`, `images`, `inspector`, `layout`,
`markdown`, `models`, `palette`, `plugins`, `queue`, `session`,
`session-list`, `theme`, `timeline`, `ui` and `workspaces`. Give a plugin a
name of its own unless you mean it to replace the built-in one of that name
— a plugin named `theme` that only sets colours takes the whole theme's
place.

**Trust.** A workspace's plugins run code from wherever the workspace came
from, so they stay off until you trust the workspace: the **Trust this
workspace** button of the browser cockpit's Plugins panel (or `/plugins
trust` in either cockpit). Trust is kept per folder in `plugins.json` in the
configuration directory, which the runner reads too;
`KOU_CONVEYOR_TRUST_WORKSPACE_PLUGINS=1` trusts the workspace for one run.

**Turning plugins off.** The Plugins panel's **Turn off**, which writes the
plugin's name under `disabled` in `plugins.json`, or
`KOU_CONVEYOR_DISABLED_PLUGINS=name,other` for one run. Built-in plugins
turn off too, but for those the Plugins panel itself needs (`layout`,
`session`, `ui`, `inspector`, `plugins`, `theme`), which only `plugins.json`
turns off. A page no plugin builds says so and offers them back, and
`/?safe` opens the cockpit with its built-in plugins only.

`kou-conveyor-runner -list-plugins -workspace .` prints what a run would find,
one JSON object per plugin, with the reason for each that does not run, and
the manifests that could not be read. The runner reports those on stderr as
`plugin error>` lines too.

## plugin.json

```json
{
  "name": "git-glance",
  "version": "1.0.0",
  "description": "Git at a glance",
  "tools": [
    {
      "name": "GitGlance",
      "description": "Show the branch, the changed files and the latest commits, as JSON.",
      "parameters": {"type": "object", "properties": {"commits": {"type": "integer"}}},
      "run": ["/bin/sh", "./glance.sh"],
      "max_output_length": 40000
    }
  ],
  "skills": "skills",
  "prompt": "prompt.md",
  "commands": [
    {"name": "review", "args": "[focus]", "description": "Review the uncommitted changes",
     "prompt": "Review the uncommitted changes. Focus: {{args}}"}
  ],
  "web": {"script": "web/plugin.js", "style": "web/plugin.css", "after": ["timeline"]}
}
```

Unknown fields are errors. Paths are relative to the plugin's directory and
may not leave it.

| Field | |
| --- | --- |
| `name` | required: lowercase letters, digits and dashes |
| `version`, `description` | shown in the cockpits |
| `tools` | tools the agent can call (below) |
| `skills` | a directory of skills, one per subdirectory with a `SKILL.md`, as in `.harness/skills` |
| `prompt` | a file, up to 64 KiB, whose text joins the system prompt under `## <name> plugin` |
| `commands` | slash commands for both cockpits (below) |
| `web` | `script`, an ES module, and `style`, a style sheet, for the browser cockpit; `after`, plugins to start before this one when they are there (below) |

## Tools

A tool runs a command. Its `name` is what the model calls (letters, digits,
`_` and `-`, starting with a letter), `description` tells the model what it
does, and `parameters` is the JSON schema of its arguments, an object.

`run` is the program and its arguments, without a shell. Arguments that start
with `./` or `../` are paths in the plugin's directory. For a call:

- The command runs in the workspace, like `Bash`.
- The call's arguments arrive on standard input as one line of JSON.
- What it prints on standard output is the result the model reads. Standard
  error follows it under `Stderr:`, and a nonzero exit code is reported.
  Each stream is cut to `max_output_length` characters (40,000 by default),
  keeping its beginning and end and the path of the complete output.
- `KOU_CONVEYOR_PLUGIN_NAME`, `KOU_CONVEYOR_PLUGIN_DIR`, `KOU_CONVEYOR_WORKSPACE`,
  `KOU_CONVEYOR_SESSION_ID`, `KOU_CONVEYOR_TOOL_NAME` and `KOU_CONVEYOR_TOOL_CALL_ID` are set,
  besides the runner's environment.

A call is a durable operation, like a `Bash` command: it runs in the
background while the agent goes on, is resumed if the runner restarts, and
is stopped with the run. A request's `disallowed_tools` leave plugin tools out
as well.

```sh
#!/bin/sh
arguments=$(cat)      # {"commits":3}
git status --short
```

## Commands

A command is typed in either cockpit's composer: `/review the error
handling`. `prompt` is what it asks the agent; `{{args}}` stands for the text
after the command, which is otherwise added at the end. `args` documents it,
as `[optional]` or `<required>`. Commands appear in the `/` suggestions, the
command palette and the keyboard sheet.

A web plugin can register commands with logic of their own (below).

## Changes while things run

Plugins are followed wherever they are used:

- **The browser cockpit.** The server looks at the plugins' files (a stat per
  file, a few times a second, only while pages are open) and streams the
  workspace's plugin listing to every page
  (`/api/w/<workspace>/plugins/events`). Each plugin's files are served under
  a fingerprint of them, so a changed plugin comes from new addresses, its
  modules' own `import`s included. The kernel compares the listing with what
  runs and loads what changed: the new version's module and style sheet are
  fetched first, then the old version is stopped — everything it added taken
  away — and the new one started, in one go. A plugin whose style sheets
  alone changed keeps running and gets its new style sheet. A plugin added
  starts; one removed, turned off or no longer trusted goes. A version that
  does not load (a syntax error) leaves the running one in place, and the
  Plugins panel says why. The page itself is loaded again only when the
  kernel's own files change.
- **Built-in plugins.** A web server built from a checkout — by `make
  build`, or by `install.sh` run from the checkout, wherever it installs —
  serves its page and its built-in plugins from that checkout, as long as it
  is there, rather than the copies compiled in, and follows them like any
  plugin:
  edit `cmd/kou-conveyor-web/plugins/composer/web/composer.js` and open pages
  have the new composer. `-assets embedded` serves the compiled-in ones,
  `-assets <dir>` those of a directory holding `static/` and `plugins/`
  (also `KOU_CONVEYOR_WEB_ASSETS`). The Plugins panel marks plugins read
  from disk *live*.
- **The server's own Go code.** A web server built from a checkout follows
  its Go code too: when a `.go` file of the checkout changes (or `go.mod`,
  `go.sum`), it builds itself anew, with the runner and the terminal cockpit
  installed beside it, and — once no agent runs and nothing waits in a queue
  — takes the new build up in place: the same process, the same port, whose
  socket the new build inherits. Open pages reconnect by themselves and say
  *The server runs the new build*; a build that fails leaves the server as
  it was, and the Plugins panel shows the compiler's errors. New runs use
  the new runner. `-rebuild=false` (or `KOU_CONVEYOR_WEB_REBUILD=0`) turns
  this off.
- **The runner.** Before every turn a run looks at where its plugins come
  from, and reads them again if anything changed: a tool added, changed or
  removed, other instructions, other skills, a plugin turned on or off reach
  the agent in its next request, and the run goes on. Calls already made
  keep running and bring their results back; a removed tool's new calls are
  turned away. The runner says so on stderr (`plugin> …`), which the browser
  cockpit shows in the runner log. `KOU_CONVEYOR_WATCH_PLUGINS=0` keeps a
  run's plugins as they were when it started. Turning the core plugin off
  takes its tools away at once; turning it on brings them from the next run.
- **The terminal cockpit** takes up plugin commands that came, changed or
  went within a couple of seconds, and says so.

Writing a web plugin that reloads well takes little: add things through the
API (they are taken away for you), keep state that should outlive a version
in `cockpit.hot.data`, and return a function (or export `deactivate`) for
anything else to undo.

In the browser's console, `cockpitKernel` shows what runs:
`cockpitKernel.instances()` the plugins loaded and their versions,
`services()`, `slots()`, `contributions('keys')`, `failures()`, and
`use('session')` any service.

## Web

The browser cockpit imports the `script` module of every active plugin of
the workspace in view, and calls its default export, or `activate`, with the
cockpit API. The plugins start in the order of the listing — built-in, then
the user's, then the workspace's — except that a plugin starts after those
its `web.after` names. Style sheets cascade in the same order, so a later
plugin's rules win over an earlier one's. Everything a plugin adds through
the API belongs to it: when it is loaded anew, turned off, or its workspace
is left, the kernel calls the function `activate` returned, or the module's
`deactivate`, and takes the rest away. Code that throws is reported in a
toast and in the runner log of the inspector, and the cockpit goes on.

Files are served from `/api/w/<workspace>/plugins/<name>/v/<version>/<path>`,
so a module can import its neighbours (`import './lib.js'`), and a plugin's
modules may import the kernel's helpers from `/kernel/dom.js` (`h`, `svg`,
`fmt`, `kv`, `uuid`, `typingIn`). Scripts, styles, JSON, images, fonts and
text are served; hidden files, other files and paths that leave the plugin
are not, nor is anything of a plugin that does not run.

```js
export default function activate(cockpit) {
  const { h } = cockpit;
  // A button in the bar over the transcript, left of the palette's.
  cockpit.ui.mount('bar.end', { id: 'standup', order: 45,
    node: h('button', { class: 'act', text: 'Standup', onclick: () => cockpit.prompt('Summarize what changed today.') }) });
  // How a tool's calls look in the timeline.
  cockpit.tools.register('GitGlance', { summary: (entry) => 'main · 2 changes' });
  // A section of the inspector, a command, a key, an item of the palette.
  cockpit.inspector.register({ id: 'git', title: 'Git', render: (view) => h('p', { text: view.title }) });
  cockpit.commands.register({ name: 'standup', args: '[days]', help: 'Summarize the recent work',
    run: (arg) => cockpit.prompt(`Summarize the commits of the last ${arg || 1} days.`) });
  cockpit.keys.register({ key: 'Mod+j', global: true, run: () => cockpit.prompt('Standup, please.') });
  cockpit.contribute('help.keys', { keys: ['⌘', 'J'], text: 'Standup', order: 300 }); // its line in the keyboard sheet
  cockpit.palette.register({ icon: '±', label: 'Review the changes', run: () => cockpit.prompt('Review the changes.') });
  // Every prompt that runs passes through a hook.
  cockpit.hooks.tap('run.request', (body) => ({ ...body, prompt: `${body.prompt}\n\nAnswer in English.` }));
  const off = cockpit.on('finish', ({ kind }) => cockpit.inspector.refresh());
  return () => off(); // runs when the plugin goes; what the API added goes by itself
}
```

### The API

| API | |
| --- | --- |
| `version` | the API's version: 2 |
| `plugin` | this plugin's listing: `name`, `version`, `source`, `script`, `code_version`… |
| `h(tag, attrs, ...children)`, `svg(markup)`, `fmt`, `uuid()` | DOM (`{ class, text, data: {...}, onclick, ... }`; `svg` for icons of your own), the cockpit's formats (`tokens`, `ago`, `duration`, `clock`, `bytes`…) |
| `api(path, options)` | the cockpit's HTTP API; a path without `/api/` is the workspace's, such as `/sessions` |
| `prefs.get(key, fallback)`, `prefs.set(key, value)` | small preferences in the browser |
| `hot.data`, `hot.reloaded` | an object kept across this plugin's versions; whether this is a version loaded anew |
| `onDispose(fn)`, `listen(target, type, fn, options)`, `timeout(fn, ms)`, `interval(fn, ms)` | cleanups, event listeners and timers that go with the plugin |
| `ui.slot(name, element)` | offers a slot: a place in the page other plugins mount into |
| `ui.mount(slot, { id, order, node })` | puts a node in a slot; the `id` of another plugin's item takes its place, and gives it back when this plugin goes |
| `ui.hide(slot, id)` | takes an item out of a slot, for as long as this plugin runs |
| `ui.slots()`, `ui.items(slot)` | the slots there are, and what is in one |
| `ui.kv(key, value)`, `ui.button(label, run, title)` | the inspector's rows and buttons |
| `contribute(point, item)`, `contributions(point, { unique })` | adds to a contribution point, and reads one (below) |
| `provide(name, service)`, `use(name)`, `has(name)` | offers a service; a stand-in that always reaches the latest provider of the name; whether one is there |
| `on(event, fn)`, `emit(event, ...args)` | listens to events (below), and sends them |
| `hooks.tap(name, fn, { order })`, `hooks.run(name, value, ...)`, `hooks.runAsync(...)`, `hooks.first(name, ...)` | takes part in what a plugin does (below) |
| `keys.register({ key, run, when, global, priority, views, repeat })`, `keys.guard(fn)` | keys: `"n"`, `"E"`, `"?"`, `"Escape"`, `"Mod+k"`, `"Alt+ArrowUp"`; a key not `global` does nothing while the user types or a dialog is open; `run` returning `false` passes the key on |
| `routes.register({ match(hash), enter(match), priority })`, `route()` | addresses the plugin answers |
| `styles.add(css)`, `styles.link(href)` | style sheets beside the manifest's |
| `store.get(key)`, `store.set(key, value)`, `store.watch(key, fn)` | small shared state, such as `view`, the view shown |
| `render()`, `renderNow()` | asks every plugin to redraw: `render` fires on the next frame |
| `safely(fn)` | runs code of the plugin's, reporting what it throws |
| `host.listing()`, `host.loaded()`, `host.reload({ force })`, `host.setWorkspace(id)`, `host.safe` | the plugins |
| `commands.register({ name, aliases, args, help, order, shown(view), complete(view), run(arg) })` | a slash command; `complete` returns `{ value, label, detail, current }` choices for the argument, `shown` hides it where it does not apply. A command replaces another plugin's of the same name |
| `palette.register({ group, icon, label, hint, detail, order, shown(view), run() })` | a command palette item |
| `inspector.register({ id, title, order, shown(view), render(view) })`, `inspector.refresh()` | an inspector section; `render` returns a node or text and runs whenever the cockpit redraws; the `id` of another takes its place |
| `tools.register(name, { summary(entry), render(entry, ui) })` | how a tool's calls look; `ui` has `h`, `fmt`, `stream(label, text)` and `copy` |
| `view()` | the session in view: `id`, `ws`, `fresh`, `title`, `running`, `interrupted`, `external`, `activity`, `usage`, `prompts`, `entries`, `queued`, `paused`, `pinned` |
| `entries()` | its entries: `{ id, kind, text, at, tool: { call_id, name, input, state, output, stderr, error, exit_code, image } }`; `image` describes the picture a `ViewImage` call read (`{ label, media_type, width, height, size }`) |
| `sessions()`, `workspaces()`, `effort()`, `models()`, `plugins()` | the session list, the workspaces, the effort and its levels, the models (`{ current, default, list }`), the plugin listing |
| `prompt(text)`, `toast(text, kind, key)`, `copy(text, label)` | runs a prompt in the session in view; a message; the clipboard |
| `actions` | what the cockpit's own commands do: `compact(focus)`, `continueRun()`, `stop()`, `editPrompt(n)`, `newSession()`, `openSession(id)`, `resume(query)`, `rename(title)`, `pin()`, `fork(n)`, `exportSession()`, `deleteSession()`, `effort(level)`, `model(id)`, `openSettings()`, `openAccounts()`, `addAccount(provider)`, `addEndpoint(kind)`, `workspace(query)`, `copyAnswer()`, `expandAll(open)`, `toggleChanges()`, `toggleInspector()`, `toggleTheme()`, `openHelp()`, `showPlugins()`, `trustPlugins(trusted)`, `enablePlugin(name, enabled)`, `reloadPlugins()`, `queue(action)` — each the service of a built-in plugin, which says so if that plugin is off |

### Slots

The built-in plugins offer these slots. Every part they put in them has an
`id` (the element's own), so a plugin can take any part's place, hide it, or
put its own before or after it by `order`.

| Slot | Offered by | Holds |
| --- | --- | --- |
| `rail.head` | layout | the mark (`mark`) |
| `rail.foot`, `rail.actions` | layout | the connection's summary (`connection`); the buttons for settings (`settings-open`, 10), the theme (`theme-toggle`, 20), help (`help-open`, 30) |
| `bar.crumbs` | layout | the workspace and title (`crumbs`) |
| `bar.end` | layout | `link-state` 10, `stream-state` 20, `run-state` 30, `session-actions` 40, `palette-open` 50, `changes-toggle` 60, `inspector-toggle` 70 |
| `stage.main` | layout | the transcript (`timeline`) |
| `dock`, `dock.float` | layout | `resume` 10, `activity` 20, `queue` 30, `composer` 40; the jump to the latest (`jump`) |
| `stage.overlay`, `overlays` | layout | the image shown large; dialogs, menus, toasts |
| `composer.above`, `composer.row` | composer | the command suggestions (`commands` 10) and the images (`attachments` 20); the model (`model` 10) and the effort (`effort` 20) |
| `sessions.tools` | session-list | `ws-switch` 10, `new-session` 20, `session-filter` 30 |

### Contribution points

A contribution point is a list any plugin adds to (`contribute(point,
item)`) and any plugin reads (`contributions(point)`, and the event
`point:<name>` when it changes). Points need no declaring; the built-in
plugins read these:

| Point | Item | Read by |
| --- | --- | --- |
| `layout.view` | `{ id, title, order, badge, rail, page, select(), shown(), hidden() }`: a view with a tab in the rail; `rail` shows in the rail while it is the view, `page`, if any, in the stage in place of the session | layout |
| `layout.panel` | `{ id, order, width, minWidth, node, opened(), closed() }`: a side panel; one is open at a time. `width` (a CSS length) is what it starts as; the user drags its edge to make it wider or narrower, down to `minWidth` pixels (260 by default), and the width is kept | layout |
| `timeline.renderer` | `{ kind, order, render(entry, ctx) }`: draws a kind of entry (`user`, `assistant`, `reasoning`, `tool`, `notice`, `error`); the latest that returns a node wins | timeline |
| `timeline.action` | `{ id, kinds, label, title, order, shown(entry, ctx), run(entry, ctx) }`: a button in an entry's header | timeline |
| `timeline.decoration` | `{ kinds, order, render(entry, ctx) }`: drawn under an entry | timeline |
| `timeline.editor` | `{ editor(entry, ctx) }`: a node in a prompt's place while it is edited | timeline |
| `tool.view` | what `tools.register` adds | timeline |
| `inspector.section` | what `inspector.register` adds | inspector |
| `commands`, `palette` | what `commands.register` and `palette.register` add | commands, palette |
| `palette.provider` | `{ id, order, items(view) }`: palette items that change | palette |
| `session.menu` | `{ items(ws, id, view) }`: more items for a session's menu | session |
| `overlay` | `{ id, order, modal, isOpen(), close() }`: Esc closes the first open one; while a modal one is open, keys that are not global do nothing | ui |
| `help.keys` | `{ keys: ['⌘', 'K'], text, order }`: a line of the keyboard sheet | help |
| `keys`, `routes` | what `keys.register` and `routes.register` add | the kernel |

The renderers' `ctx` has `h`, `fmt`, `view`, `summary`, `live`,
`index(id)`, `expanded(entry)`, `toggle(id, open)`, `copy`, `markdown(text)`,
`button(label, run, title)`, `actions(entry)` and `stream(label, text)`.

### Services

`use(name)` returns a stand-in for a service that reaches whichever plugin
provides it now: a plugin that holds it keeps working when the provider is
loaded anew, and a plugin that provides a service of a built-in's name
replaces it (for as long as it runs). The built-in plugins provide:

| Service | Plugin | Some of what it does |
| --- | --- | --- |
| `session` | session | `view()`, `summary()`, `state`, `submit(text, options)`, `prompt(text)`, `compact(focus)`, `stop()`, `newSession()`, `openSession(id)`, `branch(ws, id, message)`, `rename(ws, id, title)`, `menuItems(ws, id)`, `runBlocked(v)`, `switchWorkspace(id)`, `refreshSessions()` |
| `layout` | layout | `view()`, `show(id)`, `openPanel(id)`, `closePanel(id)`, `togglePanel(id)`, `panelOpen(id)`, `rail(open)`, `scroller()`, `width(id)` and `setWidth(id, px)` for the rail (`'rail'`) and the panels (`null` gives the default back) |
| `toast`, `menu`, `clipboard`, `overlays` | ui | `show(text, kind, key)`; `open(anchor, items)`, `toggle(anchor, items)`, `close()`; `copy(text, label)`; `closeTop()` |
| `timeline` | timeline | `schedule(id)`, `flushNow()`, `reveal(id)`, `scrollToBottom()`, `promptInView(v)`, `setAllTools(open)` |
| `composer` | composer | `value()`, `set(text, { focus, end })`, `focus()`, `clear()`, `submit({ force })`, `input`, `form` |
| `commands` | commands | `list()`, `named(name)`, `parse(text)`, `run(parsed)` |
| `models` | models | `next(v)`, `default()`, `catalog()`, `load()`, `set(v, id)`, `openPicker(options)`, `title(id)` |
| `effort` | effort | `current()`, `levels()`, `set(level)`, `cycle(step)` |
| `images` | images | `take(text)`, `encode(list)`, `attach({ input })`, `imageURL(v, entry, n)`, `toolImageURL(v, entry)`, `openToolImage(v, entry)` |
| `queue`, `edit` | queue, edit | `enqueue(text, { force })`, `command(arg)`; `begin(id)`, `editLast()` |
| `markdown` | markdown | `render(text, { onCopy })`, `inline(text)`, `codeBlock(text, language)` |
| `inspector`, `changes`, `palette`, `help`, `theme` | the plugins of those names | `toggle()`, `open()`, … |
| `accounts`, `connection`, `workspaces`, `session-list`, `plugins` | the plugins of those names | `show()`, `showConnection()`, `open()`, `openMenu(anchor)`, `rename(ws, id)`, `reload()`, … |

### Events

| Event | Arguments |
| --- | --- |
| `render` | none: redraw what you show |
| `entry`, `session`, `finish`, `plugins` | an entry and the view; the view; `{ kind, compact, view }`; the plugin listing |
| `session:view`, `session:leave`, `session:entry`, `session:dirty`, `session:loaded`, `session:finish`, `session:changes`, `session:queue`, `session:gone`, `session:before-run`, `session:sessions`, `session:workspaces`, `session:workspace`, `session:config` | the session plugin's model, as it changes (see its source) |
| `composer:input`, `composer:focus`, `composer:blur`, `composer:ready` | the composer |
| `timeline:flush`, `timeline:scroll`, `layout:view`, `layout:panel`, `layout:resize`, `models`, `theme` | the timeline; the layout (`layout:resize` is the stage's new width, as the window, the rail or a panel changes it); the models, the theme |
| `service`, `point:<name>`, `store:<key>`, `slots` | a service came or went; a contribution point changed; a stored key; a slot |
| `plugin-reloaded`, `plugin-restyled`, `plugin-error`, `online`, `ready` | the kernel |

### Hooks

| Hook | |
| --- | --- |
| `composer.submit` | `({ raw, text, force, view })`, first to return true takes what was written: the commands plugin takes `/commands`, the queue a message while the agent works |
| `composer.key` | `(event)`, first to return true takes a key pressed in the composer |
| `run.request` | `(body, view)`, async: every prompt's request passes through, and may be changed on its way |

### Changing the interface

Every part of the page is a plugin's, so any can be changed:

- **Restyle** with a plugin that has only a style sheet: the cockpit's
  colours are tokens (`--bg`, `--bg-2`…`--bg-4`, `--fg`, `--fg-2`…`--fg-4`,
  `--line`, `--line-2`, `--accent`, `--ok`, `--warn`, `--err`, `--mono`,
  `--sans`), and its classes work in plugins too: `act` for buttons, `label`
  for small capitals, `none` for empty states, `ins-actions` for a row of
  buttons.
- **Take a part's place** by mounting a node with its id in its slot:
  `cockpit.ui.mount('bar.end', { id: 'palette-open', order: 50, node })`;
  `cockpit.ui.hide('bar.end', 'run-state')` takes one away.
- **Draw entries your way** with `timeline.renderer`, or a service your way
  by providing its name (`cockpit.provide('markdown', …)`).
- **Add a view** (a tab of the rail with a page) with `layout.view`, a side
  panel with `layout.panel`, keys, routes and commands of its own.
- **Replace a whole built-in plugin**: copy its directory from
  `cmd/kou-conveyor-web/plugins` into your plugins directory and change it;
  yours runs in its place, and is followed as you edit it.

## The runner and the terminal cockpit

The runner and the terminal cockpit read the plugins at every start, and
follow them as they change (see [above](#changes-while-things-run)). The
terminal cockpit's `/plugins` lists them and trusts the workspace's;
`/plugins reload` reads them again at once.
