# genui

An example [plugin](../../../docs/plugins.md) that lets the agent build an
interface instead of only writing text. The agent describes a **surface** as
JSON: metrics, tables, charts, code, diffs, buttons and fields. The browser
cockpit draws it with its own elements in its own theme, live on the
workspace's files. The terminal and the iPhone app draw the same surfaces
with controls of their own.

- **Tool** `UI` ([ui.py](ui.py), Python 3 standard library only). It checks
  the surface and writes each version as
  `.harness/ui/<session>/<surface>/v<N>.json`, with `latest.json` beside it.
  The result's first line names that file; the rest is a text rendering, for
  the model and for clients that do not draw surfaces.
  - **Sources** are the files a surface reads (json, jsonl, csv, text).
    Components bind to them with JSON Pointers such as
    `{"path": "/sources/runs/last/p99_ms"}`.
  - The version keeps a snapshot of what the call read: the files, and the
    diffs a `Diff` shows.
  - A failed check writes nothing. It lists what to fix, with the path of
    each problem.
- **Instructions** ([prompt.md](prompt.md)) for the system prompt: when to
  make a surface, the component catalogue and how events come back.
- **Web** ([web/genui.js](web/genui.js), [web/surface.js](web/surface.js),
  [web/data.js](web/data.js), [web/genui.css](web/genui.css)):
  - **The card.** The newest call of each surface in a session is a card in
    the transcript. It reads its files again every 1.5 s while the page shows
    it, and redraws them in place.
  - **Older calls** stay rows. Opened, a row shows its version's snapshot.
  - **Open large** (⤢) shows the card over the transcript. **Pin** (⇥) puts it
    in a sidebar tab, which follows the surface's newest version.
  - **The chip.** While the newest card is scrolled away above, a chip at the
    top of the transcript stands for it, its glance live, as long as the
    surface is the current exchange's: called after the user's last message
    (a button's event does not count), or a file of it changed since — then
    the chip rings. A click brings the card back. The bottom, over the
    composer, is the jump button's.
  - **Events.** Fields and toggles keep their values in the page. A button
    with an `event` sends the agent a message,
    `[UI <surface> v<N>] <name> {data}`, through the session's queue.
  - **Html.** An `Html` is a page of the agent's (`html`, or the workspace
    file `file` names) in a frame. The cockpit serves it
    ([surfaceframe.go](../../../cmd/kou-conveyor-web/surfaceframe.go))
    sandboxed in an origin of its own, with no network, cookies or storage.
    Through `window.kou` the page takes the theme and the values the Html
    names in `inputs`, sets them back as a field would, and sends the events
    it names in `actions`, but only after a click or a key in the page. The
    frame outlives the card's redraws (`moveBefore`); a page from a file
    loads again when the file changes, keeping its values.
- **Terminal** ([surface.go](../../../cmd/kou-conveyor-tui/surface.go), on
  the Go port of `data.js` in [cmd/internal/surface](../../../cmd/internal/surface)):
  a UI call's block draws its surface in the transcript, its charts in block
  characters. The newest call of a surface is live, its files read again
  every 1.5 s. `ctrl+]` (or a click) focuses its controls: tab moves, enter
  acts, space toggles, esc leaves. A button's event goes to the session as
  the user's message. An `Html` shows its `alt`; a `FileLink` copies
  `file:line`, as the terminal opens no files.
- **iPhone app** ([SurfaceData.swift](../../../ios/KouConveyor/Core/SurfaceData.swift),
  [SurfaceModel.swift](../../../ios/KouConveyor/Models/SurfaceModel.swift),
  [SurfaceViews.swift](../../../ios/KouConveyor/Views/SurfaceViews.swift),
  [SurfaceWebView.swift](../../../ios/KouConveyor/Views/SurfaceWebView.swift)):
  SwiftUI controls and Swift Charts, live the same way through the relay,
  with events sent to the session as the user's message.
  - An `Html` is the page the cockpit makes, in a `WKWebView` of its own: a
    content security policy in the page, a rule list that blocks every load
    off the device, a data store that is not kept, and no navigation or
    popups.
  - The app hears the page from a script world the page cannot reach. It
    counts as the user's only a tap or a key in the page in the last 5 s:
    WebKit takes the app's own scripts for a gesture of the user's, so what
    the app sends reaches the page through a `MessageChannel`, without one.

Try it in a workspace:

```sh
mkdir -p .harness/plugins && cp -R path/to/examples/plugins/genui .harness/plugins/
```

Then trust the workspace (the Plugins panel of the browser cockpit, or
`/plugins trust`). Or copy it into your user plugins directory,
`~/Library/Application Support/kou-conveyor/plugins/` on macOS: from there it
runs in every workspace. Ask the agent for something to watch, for example
"show the benchmark results as a dashboard and keep it live". In this
repository, [examples/deps.json](examples/deps.json) holds the arguments of a
call with an Html: ask the agent to call `UI` with them, and the card frames
[examples/deps.html](examples/deps.html), a map of the modules go.mod
requires, whose filter and pick follow the surface's own field.

This is a prototype, and it has limits:

- **The terminal frames no `Html`:** it shows the `alt`. In its compact
  mode the transcript is printed once, so a surface there does not change.
- **The agent does not wait.** There is no `UIWait`: an event reaches the
  agent as an ordinary message, after the run it is in.
- **Files are polled by the client** that shows the surface (the page, the
  terminal, the app), while it shows it. Nothing watches them on the server.
- **A `Diff` is the one its call took.** It does not follow the file.
- **Html needs a cockpit with its route.** One built before it shows the
  `alt` text and says so. The snapshot of an older version frames its file as
  it is now, not as the call read it. The iPhone app needs the bridge of
  this version (its `ready` says `bridge: 2`): with an older one the page
  shows without its values, and the app says so.
- **The page's URL is not signed:** the cockpit guards it as it does the rest
  of its API. CSP does not cover WebRTC or DNS prefetching, so a page could
  still send out what it is given that way.
