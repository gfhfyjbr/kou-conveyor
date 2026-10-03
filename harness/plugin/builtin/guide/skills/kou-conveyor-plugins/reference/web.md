# The browser cockpit's plugin API

The reference for a plugin's `web.script`: what the cockpit API it is
started with holds, and what the built-in plugins offer through it — slots,
contribution points, services, events and hooks. The page (the *kernel*)
builds nothing itself: the layout, the sessions, the transcript, the
composer, the inspector, even the colours are built-in plugins on this same
API, so a plugin can add to any part, take any part's place, or replace a
whole built-in plugin.

## Lifecycle

```js
export default function activate(cockpit) {
  // add through cockpit.* — it all goes when the plugin does
  return () => { /* undo anything else */ };  // or export function deactivate(cockpit) {}
}
```

- Plugins start in the order of the listing — built in, then the user's,
  then the workspace's — except that a plugin starts after those its
  `web.after` names. `activate` may return a promise.
- When the plugin's files change, the kernel imports the new version,
  stops the old one (calling its cleanup and taking away everything it
  added) and starts the new one, in one go. A version that does not load —
  a syntax error — leaves the running one in place, and the Plugins section
  of the inspector says why. A change to the style sheet alone swaps it.
- `cockpit.hot.data` is an object kept across the plugin's versions;
  `cockpit.hot.reloaded` says whether this is a version loaded anew.
- Code of the plugin's that throws is reported in a toast and in the
  inspector's Runner log, and the page goes on. Callbacks the API is given
  run through the same guard; run other code with `cockpit.safely(fn)`.

## The API

| API | |
| --- | --- |
| `version` | the API's version: 2 |
| `plugin` | this plugin's listing: `name`, `version`, `source`, `script`, `code_version`… |
| `h(tag, attrs, ...children)` | an element: `attrs` takes `class`, `text`, `data: {…}`, `on<event>` handlers and other attributes; children are nodes or text. Text is never parsed as HTML |
| `svg(markup)`, `fmt`, `uuid()` | an icon from trusted SVG markup of your own; formats (`fmt.tokens`, `ago`, `duration`, `timer`, `clock`, `short`, `stamp`, `bytes`, `lines`); a UUID |
| `api(path, options)` | the cockpit's HTTP API, as JSON; a path without `/api/` is the workspace's, such as `/sessions`. `options`: `method`, `body`, `signal`, `headers` |
| `prefs.get(key, fallback)`, `prefs.set(key, value)` | small preferences in the browser's storage |
| `hot.data`, `hot.reloaded` | see Lifecycle |
| `onDispose(fn)`, `listen(target, type, fn, options)`, `timeout(fn, ms)`, `interval(fn, ms)` | a cleanup, an event listener, timers: all go with the plugin |
| `ui.slot(name, element)` | offers a slot, a place other plugins mount into |
| `ui.mount(slot, { id, order, node })` | puts a node in a slot; the `id` of another plugin's item takes its place, and gives it back when this plugin goes |
| `ui.hide(slot, id)` | takes an item out of a slot, for as long as this plugin runs |
| `ui.slots()`, `ui.items(slot)` | the slots there are, and what is in one |
| `ui.kv(key, value)`, `ui.button(label, run, title)` | the inspector's key · · · value rows, and its buttons |
| `contribute(point, item)`, `contributions(point, { unique })` | adds to a contribution point, and reads one |
| `provide(name, service)`, `use(name)`, `has(name)` | offers a service; a stand-in that always reaches the latest provider of the name; whether one is there |
| `on(event, fn)`, `emit(event, ...args)`, `listening(event)` | listens to events and sends them |
| `hooks.tap(name, fn, { order })`, `hooks.run(name, value, ...)`, `hooks.runAsync(...)`, `hooks.first(name, ...)` | takes part in what another plugin does |
| `keys.register({ key, run, when, global, priority, views, repeat, own })`, `keys.guard(fn)` | keys: `"n"`, `"E"`, `"?"`, `"Escape"`, `"Mod+k"`, `"Alt+ArrowUp"`. A key not `global` does nothing while the user types or a modal dialog is open; `views` limits it to views; `run` returning `false` passes the key on; `own` keys reach a part of the page marked `data-keys="own"`, such as a terminal |
| `routes.register({ match(hash), enter(match, hash), priority })`, `route()` | addresses the plugin answers |
| `styles.add(css)`, `styles.link(href)` | style sheets beside the manifest's |
| `store.get(key)`, `store.set(key, value)`, `store.watch(key, fn)` | small shared state, such as `view`, the view shown |
| `render()`, `renderNow()` | asks every plugin to redraw: `render` fires on the next frame |
| `safely(fn, ...args)` | runs code of the plugin's, reporting what it throws |
| `host.listing()`, `host.loaded()`, `host.failures()`, `host.reload({ force })`, `host.workspace()`, `host.setWorkspace(id)`, `host.safe` | the plugins: the server's listing (which holds the `skills` too), those loaded, their failures |
| `commands.register({ name, aliases, args, help, order, shown(view), complete(view), run(arg) })` | a slash command; `complete` returns `{ value, label, detail, current }` choices for the argument, `shown` hides it where it does not apply. It replaces another plugin's command of the name |
| `palette.register({ group, icon, label, hint, detail, order, shown(view), run() })` | an item of the command palette |
| `inspector.register({ id, title, order, shown(view), render(view), fold, open, meta(view) })`, `inspector.refresh()` | a section of the inspector; `render` returns a node or text and runs whenever the cockpit redraws; the `id` of another takes its place. With `fold` the section is a bar — its title, and what `meta` says (text, or parts `{ text, tone }`, tone `warn`, `err`, `ok` or `accent`) — that opens onto its body; it starts closed (open with `open`), stays as the user leaves it, and renders only while open |
| `tools.register(name, { summary(entry), render(entry, ui) })` | how a tool's calls look in the transcript: `summary` is the call's line, `render` the body of the open call (`null` keeps the cockpit's own); `ui` has `h`, `fmt`, `stream(label, text)` and `copy` |
| `view()` | the session in view: `id`, `ws`, `fresh`, `title`, `running`, `interrupted`, `external`, `activity`, `usage`, `prompts`, `entries`, `queued`, `paused`, `pinned` |
| `entries()` | its entries: `{ id, kind, text, at, files, tool: { call_id, name, input, state, output, stderr, error, exit_code, image } }` |
| `sessions()`, `workspaces()`, `effort()`, `models()`, `plugins()` | the session list, the workspaces, the effort and its levels, the models (`{ current, default, list }`), the plugin listing |
| `prompt(text)`, `toast(text, kind, key)`, `copy(text, label)` | runs a prompt in the session in view; a short message (`kind` `info` or `error`; a later one with the same `key` replaces it); the clipboard |
| `actions` | what the cockpit's own commands do: `compact(focus)`, `continueRun()`, `stop()`, `editPrompt(n)`, `newSession()`, `openSession(id)`, `resume(query)`, `rename(title)`, `pin()`, `fork(n)`, `exportSession()`, `deleteSession()`, `effort(level)`, `model(id)`, `openSettings()`, `openAccounts()`, `openUsage(range)`, `addAccount(provider)`, `addEndpoint(kind)`, `workspace(query)`, `copyAnswer()`, `expandAll(open)`, `toggleChanges()`, `toggleInspector()`, `toggleTheme()`, `openHelp()`, `showPlugins()`, `trustPlugins(trusted)`, `enablePlugin(name, enabled)`, `reloadPlugins()`, `queue(action)` |

A module may also import the kernel's helpers itself:
`import { h, svg, fmt, kv, uuid, typingIn } from '/kernel/dom.js'`.

## Slots

Every part the built-in plugins put in a slot has an `id`, so a plugin can
take any part's place (`ui.mount(slot, { id, order, node })`), hide it
(`ui.hide(slot, id)`), or put its own before or after it by `order`.

| Slot | Offered by | Holds, by order |
| --- | --- | --- |
| `rail.head` | layout | the mark (`mark`) |
| `rail.foot`, `rail.actions` | layout | the connection's summary (`connection`, 10) and the canvases that run (`canvas-live`, 20); the buttons for settings (`settings-open`, 10), the theme (`theme-toggle`, 20) and help (`help-open`, 30) |
| `bar.crumbs` | layout | the workspace and the title (`crumbs`); a new session's Chat · Canvas switch (`session-mode`, 5) and, for the agent of a canvas node, the way back to its canvas (`canvas-crumb`, 6) |
| `bar.end` | layout | `link-state` 10, `stream-state` 20, `run-state` 30, `session-actions` 40, `palette-open` 50, `changes-toggle` 60, `inspector-toggle` 70 |
| `stage.main` | layout | the transcript (`timeline`) |
| `dock`, `dock.float` | layout | `resume` 10, `activity` 20, `queue` 30, `composer` 40; the jump to the latest (`jump`) |
| `stage.overlay`, `overlays` | layout | the image shown large; dialogs, menus, toasts |
| `composer.above`, `composer.row` | composer | the command suggestions (`commands` 10), the file suggestions (`file-suggestions` 12), the images (`attachments` 20), the linked files (`links` 22); the model (`model` 10) and the effort (`effort` 20) |
| `sessions.tools` | session-list | `ws-switch` 10, `new-session` 20, `session-filter` 30 |

## Contribution points

A contribution point is a list any plugin adds to (`contribute(point,
item)`) and any plugin reads (`contributions(point)`, and the event
`point:<name>` when it changes). Points need no declaring. The built-in
plugins read these:

| Point | Item | Read by |
| --- | --- | --- |
| `layout.view` | `{ id, title, order, badge, rail, page, select(), shown(), hidden() }`: a view with a tab in the rail; `rail` shows in the rail while it is the view, `page`, if any, in the stage in place of the session | layout |
| `layout.panel` | `{ id, order, width, minWidth, node, opened(), closed() }`: a side panel, one open at a time; the user drags its edge, and the width is kept | layout |
| `timeline.renderer` | `{ kind, order, render(entry, ctx) }`: draws a kind of entry (`user`, `assistant`, `reasoning`, `tool`, `notice`, `error`); the latest that returns a node wins | timeline |
| `timeline.action` | `{ id, kinds, label, title, order, shown(entry, ctx), run(entry, ctx) }`: a button in an entry's header | timeline |
| `timeline.decoration` | `{ kinds, order, render(entry, ctx) }`: drawn under an entry | timeline |
| `timeline.editor` | `{ editor(entry, ctx) }`: a node in a prompt's place while it is edited | timeline |
| `tool.view` | what `tools.register` adds | timeline |
| `inspector.section` | what `inspector.register` adds | inspector |
| `sidebar.tab` | `{ id, title, icon, description, order, multiple, hello, key, create(tab) }`: a kind of tab of the sidebar (below) | sidebar |
| `sidebar.hello` | `{ id, order, render(tab) }`: a section of Hello, what the sidebar shows with no tab open | sidebar |
| `settings.section` | `{ id, title, order, render() }`: a section of the sidebar's Settings tab | sidebar |
| `commands`, `palette` | what `commands.register` and `palette.register` add | commands, palette |
| `palette.provider` | `{ id, order, items(view) }`: palette items that change | palette |
| `session.menu` | `{ items(ws, id, view) }`: more items of a session's menu | session |
| `overlay` | `{ id, order, modal, isOpen(), close() }`: Esc closes the first open one; while a modal one is open, keys that are not global do nothing | ui |
| `help.keys` | `{ keys: ['⌘', 'K'], text, order }`: a line of the keyboard sheet | help |
| `session-list.rows` | `{ id, order, rows(ws) }`: rows of the session list, among the sessions not pinned by when they changed; `rows` returns `{ id, glyph, title, meta, at, href, current, running, open(), menu() }` — `menu` returns menu items (the canvases are such rows) | session-list |
| `canvas.node` | `{ kind, preset, order, create(node, ctx) }`: what a canvas node shows (below); a `preset` (`"id"` or `"plugin/id"`) is matched before a `kind` | canvas |
| `canvas.add` | `{ id, group, title, icon, order, shown(canvas), create(at) }`: an item of a canvas's + Add menu; `at` is the point of the board it was asked at | canvas |
| `canvas.template` | `{ id, title, description, build() }`: a canvas an empty one can start from; `build` returns the operations that make it (`{ op: 'node.add', node }`, `{ op: 'edge.add', edge }`…) | canvas |
| `canvas.inspector` | `{ kinds, order, title, render(node, ctx) }`: a section of a canvas node's inspector, for nodes of the `kinds` (kinds, presets or `plugin/preset`; all when empty) | canvas |
| `keys`, `routes` | what `keys.register` and `routes.register` add | the kernel |

The renderers' `ctx` has `h`, `fmt`, `view`, `summary`, `live`,
`index(id)`, `expanded(entry)`, `toggle(id, open)`, `copy`,
`markdown(text)`, `button(label, run, title)`, `actions(entry)` and
`stream(label, text)`.

### Canvas nodes

A node's body is what `create(node, ctx)` returns: `{ node, update(node),
status(), lod(level, live), shown(), hidden(), focus(), blur(), resized(),
zoomed(zoom), contains(element), menu(), dispose() }` — `node` the
element, the rest optional: `lod` says how much the zoom shows (`low`,
`mid`, `full`) and whether a terminal may draw live, `focus` gives the
node the keys (true if it took them), `menu` returns items of the node's
⋯. `ctx` has `cockpit`, `h`, `fmt`, `model` (the canvas: `doc`,
`status`, `send(node, { text, submit, keys, deliver })`, `read(node,
what)`, `apply(ops)`, `on(event, fn)`…), `kinds`, `zoom()`, `touch()`,
`toast(text, kind)`, `update(set, label)` (changes the node, undoably),
`apply(ops, options)`, `markdown(text)`, `inspect()`, `select()`,
`focus()` and `center()`.

### Sidebar tabs

```js
cockpit.contribute('sidebar.tab', {
  id: 'notes', title: 'Notes', icon: '<svg …>', key: 'N', order: 50,
  description: 'Notes of the workspace',
  multiple: false,            // one tab of the kind at most
  create(tab) {               // when a tab of the kind first shows
    const area = cockpit.h('textarea');
    area.value = tab.state?.text || '';
    area.addEventListener('input', () => tab.save({ text: area.value }));
    return {
      node: area,             // what the tab shows
      shown() {}, hidden() {}, resized() {}, focus: () => area.focus(),
      close: () => true,      // false keeps the tab open
      dispose() {},           // the view goes: tab closed, plugin reloaded, workspace left
    };
  },
});
```

`tab` is `{ id, kind, state, save(state), setTitle(text), setBadge(text),
close(), activate(), visible() }`; tabs are kept per workspace with what
each saves.

## Services

`use(name)` returns a stand-in that reaches whichever plugin provides the
service now, so it keeps working when the provider is loaded anew; a plugin
that provides a service under a built-in's name replaces it for as long as
it runs. Check `has(name)` before using a service of a plugin that may be
off.

| Service | Plugin | Some of what it does |
| --- | --- | --- |
| `session` | session | `view()`, `summary()`, `state`, `submit(text, options)`, `prompt(text)`, `compact(focus)`, `stop()`, `newSession()`, `openSession(id)`, `branch(ws, id, message)`, `rename(ws, id, title)`, `menuItems(ws, id)`, `runBlocked(v)`, `switchWorkspace(id)`, `refreshSessions()` |
| `layout` | layout | `view()`, `show(id)`, `openPanel(id)`, `closePanel(id)`, `togglePanel(id)`, `panelOpen(id)`, `rail(open)`, `scroller()`, `width(id)`, `setWidth(id, px)` |
| `sidebar` | sidebar | `open(kind, { state, reuse, focus })`, `close(id)`, `activate(id)`, `toggle(kind)`, `show()`, `hide()`, `isOpen()`, `tabs()`, `active()`, `kinds()`, `view(id)` |
| `toast`, `menu`, `clipboard`, `overlays` | ui | `show(text, kind, key)`, `dismiss(key)`; `open(anchor, items)`, `toggle(anchor, items)`, `close()`; `copy(text, label)`; `closeTop()`, `closeAll()` |
| `timeline` | timeline | `schedule(id)`, `flushNow()`, `reveal(id)`, `scrollToBottom()`, `promptInView(v)`, `setAllTools(open)`, `toolState(entry, running)` |
| `composer` | composer | `value()`, `set(text, { focus, end })`, `focus()`, `clear()`, `submit({ force })`, `input`, `form` |
| `commands` | commands | `list()`, `named(name)`, `parse(text)`, `run(parsed)` |
| `models` | models | `next(v)`, `default()`, `catalog()`, `load()`, `set(v, id)`, `openPicker(options)`, `title(id)` |
| `effort` | effort | `current()`, `levels()`, `set(level)`, `cycle(step)` |
| `images` | images | `take(text)`, `encode(list)`, `attach({ input })`, `imageURL(v, entry, n)`, `toolImageURL(v, entry)`, `openToolImage(v, entry)` |
| `files` | files | `attach({ input, container })`, `complete(query)`, `links(text)`, `query(before)`, `label(path)` |
| `explorer` | explorer | `open(path)`: the Files tab, at a file of the workspace |
| `terminal` | terminal | `mount(container, { id, fontSize, readOnly, scale, onMeta, onExit, onFocus, onReady })` draws a shell of the server's that runs (by its `id`) in an element, and returns `{ focus(), blur(), resize(), dispose(), connected() }`; `open(fresh)`, a terminal tab; `settings()` |
| `canvas` | canvas | `open(id, ws)`, `create({ template, title })`, `current()` — `{ ws, id, title, exists, live, nodes, selected }` —, `addNode(spec, at)`, `select(ids)`, `focusNode(id)`, `fit()` |
| `queue`, `edit` | queue, edit | `enqueue(text, { force })`, `command(arg)`; `begin(id)`, `editLast()` |
| `markdown` | markdown | `render(text, { onCopy })`, `inline(text)`, `codeBlock(text, language)` |
| `inspector` | inspector | `toggle()`, `open()`, `show()`, `expand(id, open)`, `fold({ id, title, hint, meta, level, beforeOpen })` — a bar for a section to hold: `{ node, body, isOpen(), set(open), meta(parts) }` |
| `plugins`, `skills` | plugins, skills | `show()`, `trust(trusted)`, `enable(name, enabled)`, `reload()`; `show()` |
| `changes`, `palette`, `help`, `theme` | the plugins of those names | `toggle()`, `open()`; `open()`, `close()`; `open()`, `close()`; `current()`, `set(theme)`, `toggle()` |
| `accounts`, `connection`, `workspaces`, `session-list` | the plugins of those names | `show()`, `showUsage(range)`, …; `open()`, `viaGateway(url)`, …; `openMenu(anchor)`, `command(query)`, …; `rename(ws, id)`, … |

## Events

| Event | Arguments |
| --- | --- |
| `render` | none: redraw what you show |
| `entry`, `session`, `finish`, `plugins` | an entry and the view; the view; `{ kind, compact, view }`; the plugin listing |
| `session:view`, `session:leave`, `session:entry`, `session:event`, `session:dirty`, `session:loaded`, `session:finish`, `session:changes`, `session:queue`, `session:gone`, `session:before-run`, `session:sessions`, `session:workspaces`, `session:workspace`, `session:config` | the session plugin's model as it changes |
| `composer:input`, `composer:focus`, `composer:blur`, `composer:ready` | the composer |
| `timeline:flush`, `timeline:scroll` | the transcript |
| `layout:view`, `layout:panel`, `layout:resize`, `sidebar:tab` | the layout: the view, the open panel, the stage's new width, the sidebar's tab |
| `models`, `theme` | the models; the theme |
| `service`, `point:<name>`, `store:<key>`, `slots` | a service came or went; a contribution point changed; a stored key; a slot |
| `plugin-reloaded`, `plugin-restyled`, `plugin-error`, `online`, `ready` | the kernel |

## Hooks

| Hook | |
| --- | --- |
| `composer.submit` | `({ raw, text, force, view })`: the first hook to return true takes what was written — the commands plugin takes `/commands`, the queue a message while the agent works |
| `composer.key` | `(event)`: the first to return true takes a key pressed in the composer |
| `run.request` | `(body, view)`, async: every prompt's request passes through, and may be changed on its way: `cockpit.hooks.tap('run.request', (body) => ({ ...body, prompt: body.prompt + '\n\nAnswer briefly.' }))` |

## Changing the interface

- **Restyle** with a plugin that has only a style sheet: the colours are the
  tokens `--bg`, `--bg-2`…`--bg-4`, `--fg`, `--fg-2`…`--fg-4`, `--line`,
  `--line-2`, `--accent`, `--ok`, `--warn`, `--err`, with the fonts
  `--mono` and `--sans`, redefined by the light and dark themes.
- **Take a part's place** by mounting a node with its id in its slot:
  `cockpit.ui.mount('bar.end', { id: 'palette-open', order: 50, node })`;
  `cockpit.ui.hide('bar.end', 'run-state')` takes one away.
- **Draw entries your way** with `timeline.renderer`, or a service your way
  by providing its name (`cockpit.provide('markdown', …)`).
- **Add a view** (a tab of the rail with a page) with `layout.view`, a side
  panel with `layout.panel`, a kind of sidebar tab with `sidebar.tab`, and
  keys, routes and commands of its own.
- **Replace a whole built-in plugin**: name yours like it. In the
  kou-conveyor source its directory is
  `cmd/kou-conveyor-web/plugins/<name>` — copy it into the plugins
  directory of the scope you want and change it; yours runs in its place.

## Debugging

In the browser console, `cockpitKernel` shows what runs:
`instances()` the plugins loaded and their versions, `failures()`,
`services()`, `slots()`, `points()`, `contributions('keys')`, and
`use('session')` any service. `/?safe` opens the page with the built-in
plugins only.
