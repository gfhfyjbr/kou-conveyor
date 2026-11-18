// genui in the desktop app: draws a surface (what an agent shows the user
// with kou's `UI` tool, see examples/plugins/genui) with the app's own
// elements, in a chat, where the call that made it stands.
//
// The app gives a drawing `ctx.data`:
//   key       which surface this is, of all the ones drawn
//   state     the version's file, as `ui.py` wrote it
//   live      whether this is the surface's newest version: its inputs and
//             buttons work, and its sources are the files as they are now
//   sources   for a live one, each source's file: { text | mark, size,
//             mtime } or { error }; `mark` alone when the text was given
//             before and has not changed
//   code      the text of the files its Code components show, by component
//
// and takes back, through `api.act`, what a button asks of the app:
// `surface.send` (the agent is told), `file.open`, `clipboard.copy`.
//
// What the user did to a drawing is its state: { edits, used, tabs, open }.
// The data model, templates, numbers and sources' formats are shared/data.js,
// the same file the browser cockpit draws from.

const DEPTH = 32;
const SERIES = 6;
const POINTS = 2000;
const LINES = 400;

const GAPS = { none: 0, small: 6, large: 20 };

// The series' colours, in this order always: checked to stay apart for
// every kind of colour vision, on the app's dark ground and its light one.
const SERIES_DARK = ["#3987e5", "#d95926", "#199e70", "#c98500", "#d55181", "#008300"];
const SERIES_LIGHT = ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300"];

// A colour a surface names: the theme's, by the names the catalog gives
// them, or `#rrggbb`. Nothing for anything else.
//
// A surface stands in a card of the chat, on the cards' own ground, and its
// grounds are counted from there: `bg` is that ground, and each one after
// it a step lighter. Nothing in a card is the black of the pane behind it.
const GROUND = "raised";
const STEP = "hover";
const TOKENS = {
  bg: GROUND, "bg-2": STEP, "bg-3": "selected", "bg-4": "lineStrong",
  fg: "fg", "fg-2": "fgDim", "fg-3": "fgMuted", "fg-4": "fgFaint",
  line: "line", "line-2": "lineStrong",
  accent: "accent", "accent-ink": "accentInk",
  ok: "ok", warn: "blocked", err: "error", info: "done",
};

function colorOf(value) {
  if (typeof value !== "string") return undefined;
  if (/^#([0-9a-f]{3}|[0-9a-f]{6}|[0-9a-f]{8})$/i.test(value)) {
    return value.length === 4 ? "#" + value.slice(1).replace(/./g, "$&$&") : value;
  }
  if (value === "accent-soft") return ui.alpha("accent", 0.14);
  const series = /^s([0-5])$/.exec(value);
  if (series) return seriesColors()[Number(series[1])];
  return TOKENS[value];
}

const seriesColors = () => (kou.theme.dark ? SERIES_DARK : SERIES_LIGHT);

const TONES = { ok: "ok", warn: "blocked", err: "error", info: "done", neutral: "fgMuted" };

// ---------------------------------------------------------------- the data

// The sources parsed, by surface and name: a file is parsed when its text
// comes, not at every drawing.
const parsed = new Map();
let again = false;

function sourceOf(key, name, format, read) {
  const slot = key + "\n" + name;
  if (typeof read.text === "string") {
    let got;
    try {
      got = { mark: read.mark, value: parseSource(format, read.text) };
    } catch (error) {
      got = { mark: read.mark, error: "not " + format + ": " + error.message };
    }
    parsed.delete(slot);
    parsed.set(slot, got);
    if (parsed.size > 256) parsed.delete(parsed.keys().next().value);
    return got;
  }
  const kept = parsed.get(slot);
  if (kept && kept.mark === read.mark) return kept;
  // Not kept any more: the app is asked for the text again.
  again = true;
  return { error: "reading" };
}

// The data model a surface is drawn from: its own data with what the user
// typed and chose, and its sources: the files as they are for a live one,
// as the call read them for an earlier one.
function docOf(data, state) {
  const version = data.state || {};
  const doc = JSON.parse(JSON.stringify(version.data || {}));
  for (const [pointer, value] of Object.entries((state && state.edits) || {})) set(doc, pointer, value);
  const sources = {};
  const sourceInfo = {};
  const snapshot = version.snapshot || {};
  for (const [name, decl] of Object.entries(version.sources || {})) {
    const read = data.live && data.sources ? data.sources[name] : null;
    if (read) {
      const format = decl.format || inferFormat(decl.file);
      const info = { path: decl.file, format };
      if (read.size != null) info.size = read.size;
      if (read.mtime) info.mtime = read.mtime;
      if (read.error) {
        info.error = read.error;
      } else {
        const got = sourceOf(data.key, name, format, read);
        if (got.error) info.error = got.error;
        else sources[name] = got.value;
      }
      sourceInfo[name] = info;
    } else {
      if (snapshot.sources && snapshot.sources[name] != null) sources[name] = snapshot.sources[name];
      if (snapshot.sourceInfo && snapshot.sourceInfo[name]) sourceInfo[name] = snapshot.sourceInfo[name];
    }
  }
  return { ...doc, sources, sourceInfo };
}

// ---------------------------------------------------------------- drawing

function draw(env, id, depth) {
  const c = env.comps.get(id);
  if (!c || depth > DEPTH) return null;
  let node;
  try {
    node = (Object.hasOwn(KINDS, c.component) ? KINDS[c.component] : unknown)(env, c, depth);
  } catch (error) {
    node = ui.row({ gap: 8, items: "center" }, [badge(String(c.component), "err"), small(c.id + ": " + error.message)]);
  }
  // (Tests find a component by its id; the ones that answer to a press
  // name the part of them that does.)
  if (node && !node.id && !NAMED.has(c.component)) node.id = "gu-" + c.id;
  return node;
}

const NAMED = new Set(["Button", "TextField", "Select", "Slider", "FileLink"]);

const children = (env, c, depth) =>
  (Array.isArray(c.children) ? c.children : []).map((id) => draw(env, id, depth + 1)).filter(Boolean);
const text = (env, value) => textOf(env.doc, env.comps, value);
const label = (env, c) => (c.label != null ? text(env, c.label) : "");
const pointer = (value) => (isBinding(value) ? value.path : null);
const truthy = (value) => value !== MISSING && value != null && value !== false && value !== 0 && value !== "";
const gapOf = (c, normal) => (typeof c.gap === "number" ? Math.max(0, Math.min(c.gap, 64)) : Object.hasOwn(GAPS, c.gap) ? GAPS[c.gap] : normal);

const small = (value, props) => ui.text(value, { size: 12, color: "fgMuted", ...(props || {}) });
const none = (message) => ui.text(message, { size: 12, color: "fgFaint", italic: true });

function badge(value, tone) {
  const tint = TONES[tone] || TONES.neutral;
  return ui.box(
    {
      px: 6,
      py: 1,
      radius: 4,
      border: 1,
      borderColor: ui.alpha(tint, 0.45),
      bg: ui.alpha(tint, 0.1),
      shrink: 0,
    },
    [ui.text(value, { size: 11, weight: "medium", color: tint, nowrap: true })],
  );
}

// Something that answers to a press, while the surface is live.
function pressed(env, props, run) {
  if (!env.live) return props;
  return { ...props, cursor: "pointer", onClick: run };
}

// What the user typed or chose, kept as the surface's own.
function edit(api, pointer, value) {
  if (!pointer) return;
  api.setState({ edits: { ...((api.state && api.state.edits) || {}), [pointer]: value } });
}

// A button's action, done: in the page, or by telling the agent.
function act(env, c) {
  return (api) => {
    const doc = docOf(env.data, api.state);
    const action = c.action && typeof c.action === "object" ? c.action : {};
    if (action.set && typeof action.set === "object") {
      const edits = { ...((api.state && api.state.edits) || {}) };
      for (const [path, value] of Object.entries(action.set)) {
        const found = resolve(doc, value);
        edits[path] = found === MISSING ? null : found;
      }
      api.setState({ edits });
    } else if (typeof action.open === "string") {
      api.act("file.open", { path: action.open });
    } else if (action.copy != null) {
      const found = typeof action.copy === "string" ? fill(doc, env.comps, action.copy) : resolve(doc, action.copy);
      api.act("clipboard.copy", { text: typeof found === "string" ? found : shown(found) });
    } else if (action.event && typeof action.event.name === "string") {
      const data = {};
      if (action.event.data && typeof action.event.data === "object") {
        for (const [key, value] of Object.entries(action.event.data)) {
          const found = resolve(doc, value);
          data[key] = found === MISSING ? null : found;
        }
      }
      const version = env.data.state || {};
      const told = "[UI " + version.surface + " v" + version.version + "] " + action.event.name +
        (Object.keys(data).length ? " " + JSON.stringify(data) : "");
      api.act("surface.send", { text: told, name: action.event.name });
      if (c.once) api.setState({ used: { ...((api.state && api.state.used) || {}), [c.id]: true } });
    }
  };
}

// ---------------------------------------------------------------- layout

const KINDS = {
  Column(env, c, depth) {
    return ui.col({ gap: gapOf(c, 12), minW: 0 }, children(env, c, depth));
  },

  Row(env, c, depth) {
    const kids = (Array.isArray(c.children) ? c.children : []).map((id) => env.comps.get(id)).filter(Boolean);
    // A row of Metrics reads as one strip.
    if (kids.length > 1 && kids.every((kid) => kid.component === "Metric")) {
      return ui.row(
        { border: 1, borderColor: "line", radius: 6, overflow: "hidden", minW: 0 },
        children(env, c, depth).map((kid, i) =>
          ui.box({ grow: 1, minW: 0, px: 12, py: 10, borderL: i ? 1 : 0, borderColor: "line" }, [kid])),
      );
    }
    const items = { start: "start", center: "center", end: "end" }[c.align];
    const row = { gap: gapOf(c, 12), wrap: true, minW: 0, items: c.align === "stretch" ? undefined : items || "start" };
    // What fills a line shares the row; what is as small as it says does not.
    return ui.row(row, children(env, c, depth).map((kid, i) =>
      (INLINE.has((kids[i] || {}).component) ? kid : ui.box({ grow: 1, minW: 160 }, [kid]))));
  },

  Grid(env, c, depth) {
    const kids = children(env, c, depth);
    if (Number.isInteger(c.columns) && c.columns > 0) {
      return ui.el("box", { dir: "grid", cols: Math.min(c.columns, 12), gap: gapOf(c, 12), minW: 0 },
        kids.map((kid) => ui.box({ minW: 0 }, [kid])));
    }
    return ui.row({ gap: gapOf(c, 12), wrap: true, minW: 0 }, kids.map((kid) => ui.box({ grow: 1, minW: 180 }, [kid])));
  },

  Section(env, c, depth) {
    const title = text(env, c.title ?? "");
    return ui.col({ gap: 10, minW: 0 }, [
      title ? ui.row({ gap: 10, items: "center" }, [
        ui.text(title.toUpperCase(), { size: 11, weight: "semibold", color: "fgMuted", nowrap: true }),
        ui.box({ grow: 1, h: 1, bg: "line" }),
      ]) : null,
      ui.col({ gap: gapOf(c, 12), minW: 0 }, children(env, c, depth)),
    ]);
  },

  Tabs(env, c, depth) {
    const tabs = Array.isArray(c.tabs) ? c.tabs.filter((tab) => tab && typeof tab === "object") : [];
    if (!tabs.length) return null;
    const at = Math.min((env.state.tabs || {})[c.id] || 0, tabs.length - 1);
    return ui.col({ gap: 12, minW: 0 }, [
      ui.row({ gap: 2, borderB: 1, borderColor: "line" }, tabs.map((tab, i) => ui.box(
        {
          id: "gu-" + c.id + "-tab-" + i,
          px: 10,
          py: 6,
          mb: -1,
          borderB: 2,
          borderColor: i === at ? "accent" : "transparent",
          cursor: "pointer",
          hover: { bg: "hover" },
          onClick: (api) => api.setState({ tabs: { ...((api.state && api.state.tabs) || {}), [c.id]: i } }),
        },
        [ui.text(text(env, tab.label ?? "Tab " + (i + 1)), { size: 12, weight: i === at ? "semibold" : "normal", color: i === at ? "fg" : "fgMuted" })],
      ))),
      draw(env, tabs[at].child, depth + 1),
    ]);
  },

  Disclosure(env, c, depth) {
    const opened = env.state.open || {};
    const open = Object.hasOwn(opened, c.id) ? !!opened[c.id] : !!c.open;
    return ui.col({ gap: 8, minW: 0 }, [
      ui.row(
        {
          id: "gu-" + c.id + "-head",
          gap: 6,
          items: "center",
          cursor: "pointer",
          onClick: (api) => api.setState({ open: { ...((api.state && api.state.open) || {}), [c.id]: !open } }),
        },
        [ui.icon(open ? "chevron-down" : "chevron-right", { size: 12 }), ui.text(label(env, c), { size: 13, color: "fgDim" })],
      ),
      open ? ui.col({ gap: gapOf(c, 12), pl: 18, minW: 0 }, children(env, c, depth)) : null,
    ]);
  },

  // -------------------------------------------------------------- free layout

  // A box of the agent's own arranging.
  Div(env, c, depth) {
    const row = c.direction === "row";
    const props = {
      dir: row ? "row" : "col",
      gap: gapOf(c, 0),
      wrap: c.wrap === true,
      items: { start: "start", center: "center", end: "end" }[c.align],
      justify: { start: "start", center: "center", end: "end", between: "between", around: "around" }[c.justify],
      p: lengthOf(c.padding),
      px: lengthOf(c.paddingX),
      py: lengthOf(c.paddingY),
      w: sizeOf(c.width),
      h: sizeOf(c.height),
      minW: sizeOf(c.minWidth) ?? 0,
      maxW: sizeOf(c.maxWidth),
      minH: sizeOf(c.minHeight),
      maxH: sizeOf(c.maxHeight),
      grow: c.grow === true ? 1 : typeof c.grow === "number" ? c.grow : undefined,
      bg: colorOf(c.background),
      border: c.border === true ? 1 : lengthOf(c.border),
      borderColor: colorOf(c.borderColor) || (c.border ? "line" : undefined),
      radius: c.radius === "full" ? "full" : lengthOf(c.radius),
      overflow: c.radius != null ? "hidden" : undefined,
      opacity: typeof c.opacity === "number" ? c.opacity : undefined,
      ...inkOf(c),
    };
    // In a row, what fills a line shares the row's width, as in a Row; what
    // the agent arranges itself (a Div, a P, a Container) is as it says.
    let kids = children(env, c, depth);
    if (row) {
      const made = (Array.isArray(c.children) ? c.children : []).map((id) => env.comps.get(id)).filter(Boolean);
      kids = kids.map((kid, i) => {
        const kind = (made[i] || {}).component;
        return INLINE.has(kind) || OWN.has(kind) ? kid : ui.box({ grow: 1, minW: 140 }, [kid]);
      });
    }
    const box = ui.el("box", props, kids);
    if (c.action && env.live) {
      box.cursor = "pointer";
      box.onClick = act(env, c);
      box.hover = { bg: ui.alpha("fg", 0.06) };
    }
    return box;
  },

  // A paragraph: text as the agent wants it set.
  P(env, c) {
    return ui.text(text(env, c.text ?? ""), {
      size: 13,
      lineHeight: lineOf(c.size),
      color: "fg",
      ...inkOf(c),
      align: { center: "center", right: "right" }[c.align],
      italic: c.italic === true,
      truncate: c.truncate === true,
    });
  },

  // A card: a ground of its own, a rim, room inside.
  Container(env, c, depth) {
    const tint = TONES[c.tone];
    const title = c.title != null ? text(env, c.title) : "";
    return ui.col(
      {
        gap: gapOf(c, 10),
        p: lengthOf(c.padding) ?? 12,
        radius: 8,
        border: 1,
        borderColor: tint ? ui.alpha(tint, 0.5) : "line",
        bg: tint ? ui.mix(GROUND, tint, 0.08) : STEP,
        minW: 0,
      },
      [title ? ui.text(title, { size: 13, weight: "semibold", color: tint || "fg" }) : null, children(env, c, depth)],
    );
  },

  Divider(env, c) {
    // (The stronger line: it has to show on a toned Container's ground too.)
    const line = () => ui.box({ grow: 1, h: 1, bg: "lineStrong" });
    if (c.label == null) return ui.row({ items: "center", py: 2 }, [line()]);
    return ui.row({ gap: 10, items: "center", py: 2 }, [line(), small(text(env, c.label), { size: 11, nowrap: true }), line()]);
  },

  // Room between two things: as much as there is, or as much as it says.
  Spacer(env, c) {
    const size = lengthOf(c.size);
    return size == null ? ui.box({ grow: 1 }) : ui.box({ w: size, h: size, shrink: 0 });
  },

  // -------------------------------------------------------------- text

  Text(env, c) {
    const value = text(env, c.text ?? "");
    if (c.style === "heading") return ui.text(value, { size: 14, weight: "semibold", color: "fg", lineHeight: 20 });
    if (c.style === "caption") return ui.text(value, { size: 12, color: "fgMuted", lineHeight: 17 });
    if (c.style === "mono") return ui.text(value, { size: 12, font: "mono", color: "fgDim", lineHeight: 18 });
    return ui.text(value, { size: 13, color: "fg", lineHeight: 19 });
  },

  Markdown(env, c) {
    return ui.el("markdown", { value: text(env, c.text ?? ""), minW: 0 });
  },

  // -------------------------------------------------------------- values

  Metric(env, c) {
    const m = metric(env.doc, c);
    const tint = { good: "ok", bad: "error" }[m.tone] || "fgMuted";
    return ui.col({ gap: 3, minW: 0 }, [
      ui.text(label(env, c).toUpperCase(), { size: 10, weight: "semibold", color: "fgMuted", truncate: true }),
      ui.row({ gap: 4, items: "baseline" }, [
        ui.text(m.text, { size: 22, weight: "semibold", color: m.missing ? "fgFaint" : "fg", lineHeight: 26, nowrap: true }),
        m.unit ? ui.text(m.unit, { size: 12, color: "fgMuted" }) : null,
      ]),
      ui.text(m.change ? m.change.text : " ", { size: 11, weight: "medium", color: tint }),
    ]);
  },

  Badge(env, c) {
    return ui.row({}, [badge(text(env, c.text ?? ""), c.tone)]);
  },

  Progress(env, c) {
    const value = asNumber(resolve(env.doc, c.value));
    const top = asNumber(resolve(env.doc, c.max ?? 1)) || 1;
    const share = value == null ? 0 : Math.max(0, Math.min(1, value / top));
    return ui.col({ gap: 6, minW: 0 }, [
      ui.row({ gap: 8, justify: "between", items: "baseline" }, [
        ui.text(label(env, c), { size: 12, color: "fgDim", truncate: true }),
        ui.text(value == null ? "—" : Math.round(share * 100) + "%", { size: 12, font: "mono", color: "fgMuted" }),
      ]),
      ui.box({ h: 6, radius: "full", bg: "selected", overflow: "hidden" }, [
        ui.box({ h: 6, w: (share * 100).toFixed(2) + "%", radius: "full", bg: "accent" }),
      ]),
    ]);
  },

  Table(env, c) {
    const rows = resolve(env.doc, c.rows);
    if (!Array.isArray(rows)) return none(rows === MISSING ? "No rows yet" : "Not a list: " + shown(rows));
    const ordered = c.reverse ? [...rows].reverse() : rows;
    const limit = Number.isInteger(c.limit) && c.limit > 0 ? c.limit : 10;
    const some = ordered.slice(0, limit);
    let columns = Array.isArray(c.columns) && c.columns.length ? c.columns : null;
    if (!columns) {
      const first = rows.find((row) => row && typeof row === "object" && !Array.isArray(row));
      columns = first ? Object.keys(first).slice(0, 8) : ["value"];
    }
    const cols = columns.map((col) => (typeof col === "string"
      ? { key: col, label: col }
      : { key: String((col && col.key) ?? ""), label: String((col && (col.label || col.key)) || ""), align: col && col.align }));
    const cellOf = (row, col, i) => {
      if (row && typeof row === "object" && !Array.isArray(row)) return Object.hasOwn(row, col.key) ? row[col.key] : MISSING;
      return i === 0 ? row : MISSING;
    };
    const said = (value) => (typeof value === "number" ? num(value) : shown(value));
    cols.forEach((col, i) => {
      const values = some.map((row) => cellOf(row, col, i));
      if (!["left", "right", "center"].includes(col.align)) {
        const real = values.filter((value) => value !== MISSING && value != null);
        col.align = real.length && real.every((value) => typeof value === "number") ? "right" : "left";
      }
      // A column is as wide as what it holds, within reason.
      col.share = Math.max(4, Math.min(28, col.label.length, ...values.map((value) => said(value).length)));
    });
    const cell = (col, value, props) => ui.box({ grow: col.share, minW: 0, px: 8, py: 5 }, [
      ui.text(value, { truncate: true, align: col.align === "left" ? undefined : col.align, ...props }),
    ]);
    const more = rows.length - some.length;
    return ui.col({ minW: 0, border: 1, borderColor: "line", radius: 6, overflow: "hidden" }, [
      ui.row({ bg: STEP, borderB: 1, borderColor: "line" }, cols.map((col) =>
        cell(col, col.label.toUpperCase(), { size: 10, weight: "semibold", color: "fgMuted" }))),
      some.map((row, at) => ui.row({ borderT: at ? 1 : 0, borderColor: "line" }, cols.map((col, i) => {
        const value = cellOf(row, col, i);
        const missing = value === MISSING || value == null;
        return cell(col, said(value), {
          size: 12,
          font: typeof value === "number" ? "mono" : undefined,
          color: missing ? "fgFaint" : "fgDim",
        });
      }))),
      more > 0 ? ui.box({ px: 8, py: 5, borderT: 1, borderColor: "line" }, [
        small(num(more) + " more " + (more === 1 ? "row" : "rows") + (c.reverse ? " before these" : " after these"), { size: 11 }),
      ]) : null,
    ]);
  },

  List(env, c) {
    const items = resolve(env.doc, c.items);
    if (!Array.isArray(items)) return none(items === MISSING ? "Nothing yet" : "Not a list: " + shown(items));
    const ordered = c.reverse ? [...items].reverse() : items;
    const limit = Number.isInteger(c.limit) && c.limit > 0 ? c.limit : 10;
    const more = items.length - Math.min(items.length, limit);
    return ui.col({ gap: 4, minW: 0 }, [
      ordered.slice(0, limit).map((item) => ui.row({ gap: 8, items: "start" }, [
        ui.text("•", { size: 13, color: "fgFaint" }),
        ui.box({ grow: 1, minW: 0 }, [ui.text(
          typeof c.text === "string" && item && typeof item === "object" ? fillItem(item, c.text) : shown(item),
          { size: 13, color: "fgDim", lineHeight: 19 },
        )]),
      ])),
      more > 0 ? small(num(more) + " more", { size: 11 }) : null,
    ]);
  },

  Chart: chart,

  // -------------------------------------------------------------- files

  Image(env, c) {
    const file = typeof c.file === "string" ? c.file : "";
    const alt = String(c.alt ?? "");
    return ui.col({ gap: 6, minW: 0 }, [
      ui.el("image", { file, radius: 6, overflow: "hidden" }),
      alt ? small(alt) : null,
    ]);
  },

  Code(env, c) {
    const file = typeof c.file === "string" ? c.file : "";
    let value;
    let failed = "";
    if (c.text != null || !file) {
      value = text(env, c.text ?? "");
    } else {
      const read = (env.data.code || {})[c.id];
      if (read && typeof read.text === "string") value = read.text;
      else failed = (read && read.error) || "There is no " + file + ".";
    }
    const language = typeof c.language === "string" ? c.language : (/\.([A-Za-z0-9]+)$/.exec(file) || [])[1];
    return fileBox(env, file ? file + (c.lines ? ":" + c.lines : "") : c.language || "code", file, firstLine(c.lines), [], [
      failed ? ui.box({ p: 10 }, [none(failed)]) : ui.el("code", { value, lang: language, minW: 0 }),
    ]);
  },

  Diff(env, c) {
    const kept = c.patch != null ? { patch: String(c.patch) }
      : ((env.data.state.snapshot || {}).diffs || {})[c.id] || { error: "the call kept no diff" };
    const lines = kept.patch ? kept.patch.split("\n") : [];
    let added = 0;
    let removed = 0;
    for (const line of lines) {
      if (line.startsWith("+") && !line.startsWith("+++")) added++;
      else if (line.startsWith("-") && !line.startsWith("---")) removed++;
    }
    const file = typeof c.file === "string" ? c.file : "";
    const stat = [
      file ? small("against " + (c.against || "HEAD"), { size: 11 }) : null,
      ui.text("+" + added, { size: 11, font: "mono", color: "ok" }),
      ui.text("−" + removed, { size: 11, font: "mono", color: "error" }),
    ];
    let body;
    if (kept.error) body = [ui.box({ p: 10 }, [none(kept.error)])];
    else if (!added && !removed) body = [ui.box({ p: 10 }, [none("No changes")])];
    else body = diffLines(lines);
    if (kept.cut) body.push(ui.box({ px: 10, py: 5 }, [small("The diff was cut at 64 KB.", { size: 11 })]));
    return fileBox(env, file || "patch", file, null, stat, body);
  },

  FileLink(env, c) {
    const file = typeof c.file === "string" ? c.file : "";
    const line = Number.isInteger(c.line) ? c.line : null;
    const name = c.label ? text(env, c.label) : file + (line ? ":" + line : "");
    return ui.row({}, [ui.row(
      {
        id: "gu-" + c.id,
        gap: 5,
        items: "center",
        cursor: "pointer",
        tooltip: "Open " + file + (line ? ":" + line : ""),
        onClick: (api) => api.act("file.open", { path: file, line }),
      },
      [ui.text("↗", { size: 12, color: "accent" }), ui.text(name, { size: 13, color: "fg" })],
    )]);
  },

  // -------------------------------------------------------------- inputs

  Button(env, c) {
    const used = !!c.once && !!(env.state.used || {})[c.id];
    const off = !env.live || used || truthy(resolve(env.doc, c.disabled));
    // (In a row of its own, so that it is as wide as what it says.)
    return ui.row({}, [ui.button(label(env, c) + (used ? " ✓" : ""), {
      id: "gu-" + c.id,
      primary: c.style === "primary",
      danger: c.style === "danger",
      disabled: off,
      tooltip: used ? "Done: this button works once"
        : !env.live ? "A snapshot: buttons work in the newest version"
        : c.action && c.action.event ? "Tells the agent: " + c.action.event.name : undefined,
      onClick: off ? undefined : act(env, c),
    })]);
  },

  TextField(env, c) {
    const path = pointer(c.value);
    const value = resolve(env.doc, c.value);
    return field(env, c, ui.el("input", {
      id: "gu-" + c.id,
      key: c.id,
      value: value === MISSING || value == null ? "" : String(value),
      placeholder: c.placeholder != null ? text(env, c.placeholder) : "",
      multiline: c.multiline === true,
      disabled: !env.live || !path,
      onChange: env.live && path ? (api, event) => edit(api, path, String(event.value ?? "")) : undefined,
    }));
  },

  Toggle(env, c) {
    const path = pointer(c.value);
    const on = resolve(env.doc, c.value) === true;
    return ui.row(pressed(env, { id: "gu-" + c.id, gap: 8, items: "center" }, (api) => edit(api, path, !on)), [
      ui.box({ position: "relative", w: 30, h: 18, radius: "full", bg: on ? "accent" : "lineStrong", shrink: 0 }, [
        ui.box({ position: "absolute", top: 2, left: on ? 14 : 2, w: 14, h: 14, radius: "full", bg: on ? "accentInk" : "fgDim" }),
      ]),
      ui.text(label(env, c), { size: 13, color: "fgDim" }),
    ]);
  },

  Select(env, c) {
    const path = pointer(c.value);
    const options = (Array.isArray(c.options) ? c.options : []).map((option) => (option && typeof option === "object"
      ? { value: option.value, label: option.label != null ? String(option.label) : shown(option.value) }
      : { value: option, label: shown(option) }));
    const current = resolve(env.doc, c.value);
    const at = options.findIndex((option) => option.value === current ||
      (current !== MISSING && current != null && String(option.value) === String(current)));
    const pick = (i) => (api) => {
      api.setState({
        edits: { ...((api.state && api.state.edits) || {}), [path]: options[i].value },
        open: { ...((api.state && api.state.open) || {}), ["select:" + c.id]: false },
      });
    };
    const can = env.live && !!path;
    // A few short choices stand side by side; more open as a list.
    const letters = options.reduce((sum, option) => sum + option.label.length, 0);
    if (options.length <= 5 && letters <= 44) {
      return field(env, c, ui.row({ border: 1, borderColor: "lineStrong", radius: 6, overflow: "hidden" },
        options.map((option, i) => ui.box(
          {
            id: "gu-" + c.id + "-" + i,
            px: 10,
            py: 5,
            borderL: i ? 1 : 0,
            borderColor: "lineStrong",
            bg: i === at ? "selected" : undefined,
            ...(can ? { cursor: "pointer", hover: { bg: "hover" }, onClick: pick(i) } : {}),
          },
          [ui.text(option.label, { size: 12, weight: i === at ? "semibold" : "normal", color: i === at ? "fg" : "fgMuted", nowrap: true })],
        ))), true);
    }
    const slot = "select:" + c.id;
    const open = can && !!(env.state.open || {})[slot];
    return field(env, c, ui.col({ gap: 4, minW: 0 }, [
      ui.row(
        {
          id: "gu-" + c.id,
          gap: 8,
          px: 8,
          py: 5,
          items: "center",
          justify: "between",
          border: 1,
          borderColor: open ? "accent" : "lineStrong",
          radius: 6,
          bg: "code",
          ...(can ? {
            cursor: "pointer",
            onClick: (api) => api.setState({ open: { ...((api.state && api.state.open) || {}), [slot]: !open } }),
          } : {}),
        },
        [
          ui.text(at < 0 ? "—" : options[at].label, { size: 13, color: at < 0 ? "fgFaint" : "fg", truncate: true }),
          ui.icon(open ? "chevron-down" : "chevron-right", { size: 12 }),
        ],
      ),
      open ? ui.col({ border: 1, borderColor: "line", radius: 6, overflow: "hidden", bg: STEP },
        options.map((option, i) => ui.box(
          { id: "gu-" + c.id + "-" + i, px: 8, py: 5, bg: i === at ? "selected" : undefined, cursor: "pointer", hover: { bg: "hover" }, onClick: pick(i) },
          [ui.text(option.label, { size: 13, color: i === at ? "fg" : "fgDim", truncate: true })],
        ))) : null,
    ]));
  },

  Slider(env, c) {
    const path = pointer(c.value);
    const min = asNumber(c.min) ?? 0;
    const max = asNumber(c.max) ?? 100;
    const step = asNumber(c.step) || 1;
    const value = asNumber(resolve(env.doc, c.value));
    const span = max - min || 1;
    const share = Math.max(0, Math.min(1, ((value ?? min) - min) / span));
    const can = env.live && !!path;
    const at = (share * 100).toFixed(2) + "%";
    return ui.col({ gap: 4, minW: 0 }, [
      ui.row({ gap: 8, justify: "between", items: "baseline" }, [
        ui.text(label(env, c), { size: 12, color: "fgMuted", truncate: true }),
        ui.text(value == null ? "—" : num(value), { size: 12, font: "mono", color: "fgDim" }),
      ]),
      ui.box(
        {
          id: "gu-" + c.id,
          position: "relative",
          h: 18,
          mx: 7,
          ...(can ? {
            cursor: "pointer",
            onPoint: (api, event) => {
              const raw = min + event.x * span;
              const next = Math.max(min, Math.min(max, Number((min + Math.round((raw - min) / step) * step).toPrecision(12))));
              const now = asNumber(resolve(docOf(env.data, api.state), c.value));
              if (next !== now) edit(api, path, next);
            },
          } : {}),
        },
        [
          ui.box({ position: "absolute", top: 7, left: 0, right: 0, h: 4, radius: "full", bg: "selected" }),
          ui.box({ position: "absolute", top: 7, left: 0, w: at, h: 4, radius: "full", bg: "accent" }),
          ui.box({ position: "absolute", top: 2, left: at, ml: -7, w: 14, h: 14, radius: "full", bg: "fg", border: 2, borderColor: GROUND }),
        ],
      ),
    ]);
  },

  // A page of the agent's own: the app has nothing to draw one with.
  Html(env, c) {
    return ui.col({ gap: 6, p: 10, border: 1, borderColor: "line", radius: 6, dashed: true }, [
      ui.row({ gap: 8, items: "center" }, [badge("HTML", "neutral"), ui.text(String(c.alt ?? ""), { size: 13, color: "fg" })]),
      small("A page of the agent's own is not drawn in the desktop app: this is what it shows.", { size: 11 }),
    ]);
  },
};

// What stands in a line of a Row as it is, rather than sharing its width;
// and what the agent sizes itself.
const INLINE = new Set(["Badge", "Button", "Toggle", "FileLink", "Spacer", "P", "Text"]);
const OWN = new Set(["Div", "Container", "Divider"]);

function unknown(env, c) {
  return ui.row({ gap: 8, items: "center" }, [
    badge(String(c.component ?? "?"), "neutral"),
    ui.text(String(c.alt ?? "not drawn here"), { size: 13, color: "fgDim" }),
  ]);
}

// An input under its name.
function field(env, c, control, tight) {
  const name = label(env, c);
  return ui.col({ gap: 4, minW: 0, items: tight ? "start" : undefined }, [
    name ? ui.text(name, { size: 12, color: "fgMuted" }) : null,
    control,
  ]);
}

// A file's frame: its name, what is said of it, a way to open it; and what
// it holds under that.
function fileBox(env, name, file, line, said, body) {
  return ui.col({ minW: 0, border: 1, borderColor: "line", radius: 6, overflow: "hidden" }, [
    ui.row({ gap: 10, px: 10, py: 6, items: "center", bg: STEP, borderB: 1, borderColor: "line" }, [
      ui.box({ grow: 1, minW: 0 }, [ui.text(name, { size: 12, font: "mono", color: "fgDim", truncate: true })]),
      said,
      file ? ui.box(
        { cursor: "pointer", tooltip: "Open " + file, onClick: (api) => api.act("file.open", { path: file, line }) },
        [ui.text("Open ↗", { size: 11, color: "fgMuted", nowrap: true })],
      ) : null,
    ]),
    body,
  ]);
}

// ---------------------------------------------------------------- what a Div and a P take

function lengthOf(value) {
  return typeof value === "number" && value >= 0 ? Math.min(value, 2000) : undefined;
}

function sizeOf(value) {
  if (typeof value === "number") return lengthOf(value);
  if (value === "full" || (typeof value === "string" && /^\d+(\.\d+)?%$/.test(value))) return value;
  return undefined;
}

const SIZES = { small: 12, normal: 13, large: 16, title: 20 };

function fontSize(value) {
  if (typeof value === "number") return Math.max(8, Math.min(value, 72));
  return SIZES[value];
}

function lineOf(value) {
  const size = fontSize(value) || 13;
  return Math.round(size * 1.45);
}

// How text is set in a Div and everything in it, or in a P.
function inkOf(c) {
  const out = {};
  const color = colorOf(c.color);
  if (color) out.color = color;
  const size = fontSize(c.size);
  if (size) out.size = size;
  if (["normal", "medium", "semibold", "bold"].includes(c.weight)) out.weight = c.weight;
  if (c.font === "mono" || c.font === "ui") out.font = c.font;
  return out;
}

// ---------------------------------------------------------------- diffs

function range(lines) {
  const m = /^(\d+)(?:-(\d+))?$/.exec(typeof lines === "string" ? lines.trim() : "");
  if (!m) return [1, 0];
  const from = Number(m[1]);
  return [from, Number(m[2] || from)];
}

const firstLine = (lines) => range(lines)[0] || null;

// A unified diff's hunks, with the lines' numbers old and new.
function diffLines(lines) {
  const out = [];
  let old = 0;
  let now = 0;
  let inHunk = false;
  let drawn = 0;
  let left = 0;
  const number = (value) => ui.box({ w: 34, shrink: 0 }, [ui.text(value, { size: 11, font: "mono", color: "fgFaint", align: "right" })]);
  const row = (kind, a, b, value) => ui.row(
    { gap: 8, px: 8, bg: kind === "add" ? ui.alpha("ok", 0.12) : kind === "del" ? ui.alpha("error", 0.12) : kind === "hunk" ? STEP : undefined },
    [number(a), number(b), ui.box({ grow: 1, minW: 0 }, [ui.text(value, {
      size: 12, font: "mono", lineHeight: 18, truncate: true, color: kind === "hunk" ? "fgMuted" : kind === "ctx" ? "fgDim" : "fg",
    })])],
  );
  for (const line of lines) {
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/.exec(line);
    if (hunk) {
      old = Number(hunk[1]);
      now = Number(hunk[2]);
      inHunk = true;
      if (drawn < LINES) out.push(row("hunk", "", "", line));
      continue;
    }
    if (!inHunk || line.startsWith("\\")) continue;
    const kind = line[0] === "+" ? "add" : line[0] === "-" ? "del" : "ctx";
    if (kind !== "ctx" || line.startsWith(" ")) {
      if (drawn >= LINES) {
        left++;
      } else {
        out.push(row(kind, kind === "add" ? "" : String(old), kind === "del" ? "" : String(now), line.slice(1) || " "));
        drawn++;
      }
    }
    if (kind !== "add") old++;
    if (kind !== "del") now++;
  }
  if (left) out.push(ui.box({ px: 10, py: 5 }, [small(num(left) + " more lines", { size: 11 })]));
  return out;
}

// ---------------------------------------------------------------- charts

// The step between round values from lo to hi, about `count` of them.
function ticks(lo, hi, count) {
  const raw = (hi - lo) / count;
  const mag = 10 ** Math.floor(Math.log10(raw));
  return [1, 2, 2.5, 5, 10].map((m) => m * mag).find((step) => step >= raw * 0.999) || 10 * mag;
}

function chart(env, c) {
  const fields = (typeof c.y === "string" ? [c.y] : Array.isArray(c.y) ? c.y : []).filter((f) => typeof f === "string").slice(0, SERIES);
  const alt = String(c.alt ?? "");
  const unit = c.unit != null ? String(c.unit) : "";
  let data = resolve(env.doc, c.data);
  if (!Array.isArray(data) || !data.length || !fields.length) {
    return none((data === MISSING ? "No points yet" : "No points") + (alt ? " · " + alt : ""));
  }
  const limit = Number.isInteger(c.limit) && c.limit > 0 ? Math.min(c.limit, POINTS) : POINTS;
  data = data.slice(-limit);
  const n = data.length;
  const at = (point, field) => asNumber(point && typeof point === "object" ? point[field] : point);
  const series = fields.map((field) => data.map((point) => at(point, field)));
  const xs = data.map((point, i) => (typeof c.x === "string" && point && typeof point === "object" && point[c.x] != null ? shown(point[c.x]) : String(i + 1)));
  let lo = Infinity;
  let hi = -Infinity;
  for (const values of series) for (const value of values) if (value != null) { lo = Math.min(lo, value); hi = Math.max(hi, value); }
  if (!Number.isFinite(lo)) return none("No numbers in " + fields.join(", ") + (alt ? " · " + alt : ""));
  const kind = ["line", "bar", "points"].includes(c.kind) ? c.kind : "line";
  if (kind === "bar") { lo = Math.min(lo, 0); hi = Math.max(hi, 0); }
  if (lo === hi) { lo -= Math.abs(lo) * 0.1 || 1; hi += Math.abs(hi) * 0.1 || 1; }
  const step = ticks(lo, hi, 3);
  lo = Math.floor(lo / step + 1e-9) * step;
  hi = Math.ceil(hi / step - 1e-9) * step;
  const said = (value) => (value == null ? "—" : num(value) + (unit ? " " + unit : ""));
  const grid = [];
  for (let t = lo; t <= hi + step * 1e-6; t += step) grid.push(Math.abs(t) < step * 1e-6 ? 0 : Number(t.toPrecision(12)));
  const colors = seriesColors();
  const height = Number.isInteger(c.height) && c.height >= 60 ? Math.min(c.height, 600) : 160;
  const last = (values) => {
    for (let i = values.length - 1; i >= 0; i--) if (values[i] != null) return values[i];
    return null;
  };
  // Names and newest values in the text's own ink; the colour is the mark
  // beside them. One series needs no mark: the line above says what it is.
  const legend = ui.row({ gap: 14, wrap: true, items: "baseline" }, [
    alt ? ui.box({ grow: 1, minW: 120 }, [small(alt)]) : null,
    fields.map((field, j) => ui.row({ gap: 6, items: "center" }, [
      fields.length > 1 ? ui.box({ w: 8, h: 8, radius: 2, bg: colors[j], shrink: 0 }) : null,
      small(field, { nowrap: true }),
      ui.text(said(last(series[j])), { size: 12, weight: "medium", color: "fg", nowrap: true }),
    ])),
  ]);
  // The grid's values, each beside its line.
  const axis = ui.col({ h: height + 14, my: -7, justify: "between", items: "end", shrink: 0 },
    [...grid].reverse().map((value) => ui.text(num(value), { size: 10, font: "mono", color: "fgFaint", lineHeight: 14 })));
  const names = kind === "bar" && n <= 12
    ? ui.row({}, xs.map((x) => ui.box({ grow: 1, minW: 0, px: 2 }, [ui.text(x, { size: 10, color: "fgFaint", align: "center", truncate: true })])))
    : ui.row({ justify: "between", gap: 8 }, [
      ui.text(xs[0], { size: 10, color: "fgFaint", nowrap: true }),
      n > 2 ? ui.text(xs[Math.floor((n - 1) / 2)], { size: 10, color: "fgFaint", nowrap: true }) : null,
      n > 1 ? ui.text(xs[n - 1], { size: 10, color: "fgFaint", nowrap: true }) : null,
    ]);
  return ui.col({ gap: 10, minW: 0 }, [
    legend,
    ui.row({ gap: 8, items: "start", pt: 7 }, [
      axis,
      ui.col({ grow: 1, minW: 0, gap: 6 }, [
        ui.el("plot", {
          id: "gu-" + c.id + "-plot",
          kind,
          lo,
          hi,
          grid,
          h: height,
          labels: xs,
          series: fields.map((field, j) => ({ name: field, color: colors[j], values: series[j], shown: series[j].map(said) })),
        }),
        names,
      ]),
    ]),
  ]);
}

// ---------------------------------------------------------------- the view

kou.view({
  id: "genui",
  render(ctx) {
    const data = ctx.data || {};
    const version = data.state || {};
    again = false;
    const state = ctx.state || {};
    const comps = new Map((Array.isArray(version.components) ? version.components : []).map((c) => [c.id, c]));
    const env = { data, state, comps, live: data.live === true, doc: docOf(data, state) };
    let body;
    if (!comps.has("root")) body = none("This surface has no root component.");
    else body = draw(env, "root", 0) || none("Nothing to show.");
    // What is wrong with its files, said under it.
    const said = [];
    for (const [name, decl] of Object.entries(version.sources || {})) {
      const error = (env.doc.sourceInfo[name] || {}).error;
      if (!error || error === "reading") continue;
      said.push(error === "no such file" ? "Waiting for " + decl.file : decl.file + ": " + error);
    }
    return ui.col({ again, gap: 12, p: 14, minW: 0 }, [
      body,
      said.length ? ui.col({ gap: 2 }, said.map((line) => small(line, { size: 11, color: "blocked" }))) : null,
    ]);
  },
});
