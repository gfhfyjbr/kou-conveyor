The `UI` tool shows the user a **surface**: a small native UI in the browser cockpit — metrics, charts, tables, diffs, inputs and buttons — bound to data and to files of the workspace. The cockpit draws it with its own components, in its theme: you send structure and data, not HTML or CSS — save for `Html`, a page of your own for what the components cannot draw. A surface bound to files stays live: as the files change, it changes, without another call.

Use it when a picture says more than prose: numbers that change (benchmarks, a training run, a long job's progress), a comparison, a table of results, a change for the user to approve, a choice for the user to make. Do not use it for what a sentence says. The terminal and the iPhone app draw surfaces too, with controls of their own (their older builds show the call's text); the terminal shows an `Html` as its `alt`.

### A call

- `surface`: its ID, as `bench`. Another call with the same ID changes that surface: it gets a new version, and the newest version is the live one, drawn where it was called; the calls before it show as snapshots.
- `components`: a flat list of `{ "id": "…", "component": "Metric", …props }`. Containers name their children by ID; the surface starts at the component `root`. A later call sends only what changes: components are replaced by ID, and `remove` lists IDs to drop.
- `data`: changes to the surface's data model, `{ "/pointer": value }`; `null` removes.
- `sources`: files the surface reads, `{ "<name>": { "file": "path in the workspace", "format": "…" } }`; `null` drops one. The format follows the extension when left out. A source is at `/sources/<name>`: `jsonl`, `csv` and `tsv` give `{ "rows": […], "last": {…}, "count": n }` (numbers in cells are numbers), `json` the value, `text` and `markdown` the text. `/sourceInfo/<name>` has its `path`, `size`, `mtime` and `error`. A file that is not there yet is fine: the surface shows it once it appears.
- `title`; `glance`: one line that sums the surface up while it is out of view, as `"p99 {#p99}"`.
- `delete: true` removes the surface.

### Values

- A prop takes a value, or a binding `{ "path": "/sources/runs/last/p99_ms" }`: a JSON Pointer into the data model, where `-1` is the last element of a list and `-2` the one before it.
- Text props are templates: `{/pointer}` puts a value in; `{#id}` a Metric as it shows, as `41.6 ms ▲3%`.
- Inputs bind their `value` to the data model outside `/sources`: `{ "path": "/comment" }`.

### Components

- Layout: `Column`, `Row` (`align`), `Grid` (`columns`), `Section` (`title`) — each with `children` and `gap` (`none`, `small`, `large`); `Tabs` (`tabs: [{ "label", "child" }]`); `Disclosure` (`label`, `children`, `open`).
- `Text` (`text`, `style`: `body`, `heading`, `caption`, `mono`); `Markdown` (`text`).
- `Metric` (`label`, `value`, `unit`, `digits`, `better`: `higher` or `lower`, `previous`): a number and its change from `previous`, or — when `value` is `/sources/<s>/last/<field>` — from the row before. A Row of Metrics reads as one strip.
- `Badge` (`text`, `tone`: `ok`, `warn`, `err`, `info`, `neutral`); `Progress` (`label`, `value`, `max`).
- `Table` (`rows`, `columns`: fields or `{ "key", "label", "align" }`, `limit`, `reverse` for newest first); `List` (`items`, `text`: a template of each item's fields, as `"{/name}: {/status}"`).
- `Chart` (`kind`: `line`, `bar`, `points`; `data`: a list of objects; `x`: a field; `y`: a field or up to 6; `unit`, `limit`, `height`; `alt`, which is required: what it shows, in words).
- `Image` (`file`, `alt`); `Code` (`file` with `lines` as `"10-40"`, or `text`; `language`); `Diff` (`file` against `against`, `HEAD` by default — taken when you call; or `patch`); `FileLink` (`file`, `label`, `line`): opens the file in the cockpit.
- `Button` (`label`, `action`, `style`: `default`, `primary`, `danger`; `once`; `disabled`); `TextField` (`label`, `value`, `placeholder`, `multiline`); `Toggle` (`label`, `value`); `Select` (`label`, `value`, `options`: values or `{ "value", "label" }`); `Slider` (`label`, `value`, `min`, `max`, `step`).
- `Html` (`html`, or `file`: a `.html` file of the workspace; `alt`, which is required; `height`; `inputs`; `actions`): a page of your own, for what the components above cannot draw — an interactive map, a visualisation of its own. Prefer the components above; see «Html» below.

### Actions

A Button's `action` is one of:
- `{ "event": { "name": "apply", "data": { "comment": { "path": "/comment" } } } }`: the user's click reaches you as their message, `[UI migration v1] apply {"comment":"after 18:00"}`, with the bindings in `data` read at the click. Then do what it asks. `"once": true` suits a decision.
- `{ "set": { "/pointer": value } }`, `{ "open": "path" }`, `{ "copy": "text" }` or `{ "copy": { "path": "/…" } }`: done in the page, without you.

### Html

The cockpit frames an Html's page in the card, sandboxed in an origin of its own: no network (`fetch`, CDNs and remote pictures fail), no cookies or storage (`localStorage` throws), no `eval`, no forms, popups or navigation. Inline all of it — `<style>`, `<script>`, `data:` pictures — and draw with the cockpit's theme, which the page gets as custom properties: `--bg`, `--bg-2` … `--bg-4`, `--fg`, `--fg-2` … `--fg-4`, `--line`, `--line-2`, `--accent`, `--accent-soft`, `--accent-ink`, `--ok`, `--warn`, `--err`, `--mono`, `--sans`, the series colours `--s0` … `--s5`, and the corners' `--radius` and `--radius-sm` (square unless the user rounds them); light and dark come with them. Let it be as tall as what it shows, without `100vh`: the frame follows its height, unless `height` sets one.

`window.kou` links the page to the surface:
- `kou.data`: the values of the pointers `inputs` lists, by pointer, as `kou.data["/sources/mod"]`; `kou.on("data", fn)` calls `fn(data)` with them now and whenever they change — a file, an input, the page itself. `kou.on("theme", fn)` follows the theme.
- `kou.set("/filter", value)`: sets a pointer `inputs` lists, outside `/sources`, as an input does; the components bound to it follow.
- `kou.action("explain", { … })`: an event `actions` lists, which reaches you as a Button's does, `[UI deps v1] explain {…}` — only from a click or a key of the user's in the page.
- `kou.openFile("go.mod", 12)`: opens one of the surface's files in the cockpit.

An Html of a `file` follows the file: change the file and the card shows it without another call; older versions show it as it is now. `html` is at most 512 000 characters, a `file` 5 MiB. As:

```json
{ "id": "map", "component": "Html", "file": "tools/deps.html", "alt": "go.mod's modules by owner; the direct ones bright",
  "inputs": ["/sources/mod", "/filter", "/picked"], "actions": ["explain"] }
```

### The result

The result names the version, lists warnings, and shows the surface as text, as the user sees it now. Read it: a `—` is a value its path found nothing at. Fix what is wrong with another call. A call with errors shows nothing and lists what to fix.

### Examples

A benchmark kept in a file, live as rows come in:

```json
{ "surface": "bench", "title": "sessionstore: benchmarks", "glance": "p99 {#p99}",
  "sources": { "runs": { "file": "benchmarks/results.jsonl" }, "notes": { "file": "benchmarks/NOTES.md" } },
  "components": [
    { "id": "root", "component": "Column", "children": ["kpis", "trend", "table", "notes"] },
    { "id": "kpis", "component": "Row", "children": ["p50", "p99", "allocs"] },
    { "id": "p50", "component": "Metric", "label": "p50", "unit": "ms", "better": "lower", "digits": 1, "value": { "path": "/sources/runs/last/p50_ms" } },
    { "id": "p99", "component": "Metric", "label": "p99", "unit": "ms", "better": "lower", "digits": 1, "value": { "path": "/sources/runs/last/p99_ms" } },
    { "id": "allocs", "component": "Metric", "label": "allocs/op", "better": "lower", "value": { "path": "/sources/runs/last/allocs" } },
    { "id": "trend", "component": "Chart", "kind": "line", "data": { "path": "/sources/runs/rows" }, "x": "commit", "y": ["p50_ms", "p99_ms"], "unit": "ms", "alt": "p50 and p99 by commit" },
    { "id": "table", "component": "Table", "rows": { "path": "/sources/runs/rows" }, "reverse": true, "limit": 5, "columns": ["commit", "p50_ms", "p99_ms", "allocs"] },
    { "id": "notes", "component": "Markdown", "text": { "path": "/sources/notes" } } ] }
```

A change for the user to approve:

```json
{ "surface": "migration", "title": "Schema migration", "data": { "/mode": "batch" },
  "components": [
    { "id": "root", "component": "Column", "children": ["what", "diff", "comment", "mode", "buttons"] },
    { "id": "what", "component": "Markdown", "text": "Three tables change; `sessions` is rebuilt whole (~40 s of downtime)." },
    { "id": "diff", "component": "Diff", "file": "db/schema.sql" },
    { "id": "comment", "component": "TextField", "label": "Comment", "placeholder": "when to run it", "value": { "path": "/comment" } },
    { "id": "mode", "component": "Select", "label": "fsync", "value": { "path": "/mode" }, "options": ["always", "batch", "off"] },
    { "id": "buttons", "component": "Row", "children": ["apply", "cancel"] },
    { "id": "apply", "component": "Button", "label": "Apply", "style": "primary", "once": true,
      "action": { "event": { "name": "apply", "data": { "comment": { "path": "/comment" }, "mode": { "path": "/mode" } } } } },
    { "id": "cancel", "component": "Button", "label": "Cancel", "action": { "event": { "name": "cancel" } } } ] }
```

Later, to show that it ran: `{ "surface": "migration", "components": [{ "id": "buttons", "component": "Badge", "text": "applied", "tone": "ok" }], "remove": ["apply", "cancel"] }`.
