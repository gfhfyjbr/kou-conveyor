// surface: draws a surface — its components, from the one called root —
// with elements of the page's own, in the cockpit's theme. What it draws
// comes from the agent and from files, so it reaches the DOM only as text
// nodes and attributes: no HTML, and no style attributes, which the page's
// CSP refuses; sizes and positions are custom properties set through the
// CSSOM.
//
// renderSurface(env) draws one. env:
//   h                 the cockpit's element builder
//   doc, comps        the data model, and the components by id
//   live              whether inputs and buttons work (a snapshot's do not)
//   ui                what the user did to the drawing: { tabs, open } maps
//   redraw()          draws the surface again
//   edit(ptr, value)  an input changed the data model
//   act(component)    a button was pressed
//   used(id)          whether a once button was
//   openFile(path, line), rawUrl(path), markdown(text) → node
//   code(component)   → Promise of { text, runs, classes } | { error }
//   diffs             the diffs the call took, by component id
//   frame(component)  → the node of an Html: its page, framed (genui.js);
//                       without it, an Html shows its alt

import { MISSING, resolve, isBinding, asNumber, num, shown, metric, textOf, fillItem } from './data.js';

const SVG = 'http://www.w3.org/2000/svg';
const DEPTH = 32;
const SERIES = 6;
const POINTS = 2000;
const LINES = 400;

export function renderSurface(env) {
  if (!env.comps.has('root')) return env.h('div', { class: 'gu-none', text: 'This surface has no root component.' });
  return draw(env, 'root', 0) || env.h('div', { class: 'gu-none', text: 'Nothing to show.' });
}

function draw(env, id, depth) {
  const c = env.comps.get(id);
  if (!c || depth > DEPTH) return null;
  let node;
  try {
    node = (KINDS[c.component] || unknown)(env, c, depth);
  } catch (error) {
    node = env.h('div', { class: 'gu-unknown' }, env.h('span', { class: 'gu-badge', data: { tone: 'err' }, text: c.component }), ` ${c.id}: ${error.message}`);
  }
  if (node instanceof Element) node.dataset.guId = c.id;
  return node;
}

const children = (env, c, depth) => (Array.isArray(c.children) ? c.children : []).map((id) => draw(env, id, depth + 1)).filter(Boolean);
const text = (env, value) => textOf(env.doc, env.comps, value);
const label = (env, c) => (c.label != null ? text(env, c.label) : '');
const pointer = (value) => (isBinding(value) ? value.path : null);
const truthy = (value) => value !== MISSING && value != null && value !== false && value !== 0 && value !== '';

function setVar(el, name, value) {
  el.style.setProperty(name, String(value));
  return el;
}

// ---------------------------------------------------------------- layout

const KINDS = {
  Column(env, c, depth) {
    return env.h('div', { class: 'gu-col', data: { gap: c.gap } }, children(env, c, depth));
  },

  Row(env, c, depth) {
    const kids = (Array.isArray(c.children) ? c.children : []).map((id) => env.comps.get(id)).filter(Boolean);
    // A row of Metrics reads as one strip.
    const strip = kids.length > 1 && kids.every((k) => k.component === 'Metric');
    return env.h('div', { class: strip ? 'gu-row gu-strip' : 'gu-row', data: { gap: c.gap, align: c.align } }, children(env, c, depth));
  },

  Grid(env, c, depth) {
    const node = env.h('div', { class: 'gu-grid', data: { gap: c.gap, fixed: Number.isInteger(c.columns) ? 'true' : null } }, children(env, c, depth));
    if (Number.isInteger(c.columns) && c.columns > 0) setVar(node, '--gu-cols', Math.min(c.columns, 12));
    return node;
  },

  Section(env, c, depth) {
    return env.h('section', { class: 'gu-section' },
      env.h('h3', { class: 'gu-section-title', text: text(env, c.title ?? '') }),
      env.h('div', { class: 'gu-col', data: { gap: c.gap } }, children(env, c, depth)));
  },

  Tabs(env, c, depth) {
    const tabs = Array.isArray(c.tabs) ? c.tabs.filter((t) => t && typeof t === 'object') : [];
    if (!tabs.length) return null;
    const at = Math.min(env.ui.tabs.get(c.id) || 0, tabs.length - 1);
    return env.h('div', { class: 'gu-tabs' },
      env.h('div', { class: 'gu-tablist', role: 'tablist' }, tabs.map((t, i) => env.h('button', {
        class: 'gu-tab', type: 'button', role: 'tab', 'aria-selected': String(i === at),
        onclick: () => { env.ui.tabs.set(c.id, i); env.redraw(); },
      }, text(env, t.label ?? `Tab ${i + 1}`)))),
      env.h('div', { class: 'gu-tabpanel', role: 'tabpanel' }, draw(env, tabs[at].child, depth + 1)));
  },

  Disclosure(env, c, depth) {
    const open = env.ui.open.has(c.id) ? env.ui.open.get(c.id) : !!c.open;
    const node = env.h('details', { class: 'gu-disclosure', open, ontoggle: () => env.ui.open.set(c.id, node.open) },
      env.h('summary', { class: 'gu-summary' }, env.h('span', { class: 'chev', 'aria-hidden': 'true' }), label(env, c)),
      env.h('div', { class: 'gu-col gu-disclosed', data: { gap: c.gap } }, children(env, c, depth)));
    return node;
  },

  // -------------------------------------------------------------- text

  Text(env, c) {
    const style = ['heading', 'caption', 'mono'].includes(c.style) ? c.style : 'body';
    return env.h(style === 'heading' ? 'h4' : 'p', { class: 'gu-text', data: { style }, text: text(env, c.text ?? '') });
  },

  Markdown(env, c) {
    return env.h('div', { class: 'gu-md prose' }, env.markdown(text(env, c.text ?? '')));
  },

  // -------------------------------------------------------------- values

  Metric(env, c) {
    const m = metric(env.doc, c);
    return env.h('div', { class: 'gu-metric', data: { missing: m.missing ? 'true' : null } },
      env.h('div', { class: 'gu-metric-label', text: label(env, c) }),
      env.h('div', { class: 'gu-metric-value' },
        env.h('span', { class: 'gu-num', text: m.text }),
        m.unit ? env.h('span', { class: 'gu-unit', text: m.unit }) : null),
      env.h('div', { class: 'gu-delta', data: { tone: m.tone }, text: m.change ? m.change.text : '\u00a0' }));
  },

  Badge(env, c) {
    const tone = ['ok', 'warn', 'err', 'info', 'neutral'].includes(c.tone) ? c.tone : 'neutral';
    return env.h('span', { class: 'gu-badge', data: { tone }, text: text(env, c.text ?? '') });
  },

  Progress(env, c) {
    const value = asNumber(resolve(env.doc, c.value));
    const top = asNumber(resolve(env.doc, c.max ?? 1)) || 1;
    const share = value == null ? 0 : Math.max(0, Math.min(1, value / top));
    const bar = setVar(env.h('i'), '--gu-share', share);
    return env.h('div', { class: 'gu-progress', data: { missing: value == null ? 'true' : null } },
      env.h('div', { class: 'gu-progress-head' },
        env.h('span', { class: 'gu-progress-label', text: label(env, c) }),
        env.h('span', { class: 'gu-progress-value', text: value == null ? '—' : `${Math.round(share * 100)}%` })),
      env.h('div', { class: 'gu-bar' }, bar));
  },

  Table(env, c) {
    const rows = resolve(env.doc, c.rows);
    if (!Array.isArray(rows)) return none(env, rows === MISSING ? 'No rows yet' : `Not a list: ${shown(rows)}`);
    const ordered = c.reverse ? [...rows].reverse() : rows;
    const limit = Number.isInteger(c.limit) && c.limit > 0 ? c.limit : 10;
    const shownRows = ordered.slice(0, limit);
    let columns = Array.isArray(c.columns) && c.columns.length ? c.columns : null;
    if (!columns) {
      const first = rows.find((r) => r && typeof r === 'object' && !Array.isArray(r));
      columns = first ? Object.keys(first).slice(0, 8) : ['value'];
    }
    const cols = columns.map((col) => (typeof col === 'string'
      ? { key: col, label: col }
      : { key: String(col?.key ?? ''), label: String(col?.label || col?.key || ''), align: col?.align }));
    const cellOf = (row, col, i) => {
      if (row && typeof row === 'object' && !Array.isArray(row)) return Object.hasOwn(row, col.key) ? row[col.key] : MISSING;
      return i === 0 ? row : MISSING;
    };
    for (const col of cols) {
      if (!['left', 'right', 'center'].includes(col.align)) {
        const values = shownRows.map((r) => cellOf(r, col, cols.indexOf(col))).filter((v) => v !== MISSING && v != null);
        col.align = values.length && values.every((v) => typeof v === 'number') ? 'right' : 'left';
      }
    }
    const table = env.h('table', { class: 'gu-table' },
      env.h('thead', null, env.h('tr', null, cols.map((col) => env.h('th', { data: { align: col.align }, text: col.label })))),
      env.h('tbody', null, shownRows.map((row) => env.h('tr', null, cols.map((col, i) => {
        const v = cellOf(row, col, i);
        return env.h('td', { data: { align: col.align, missing: v === MISSING || v == null ? 'true' : null }, text: typeof v === 'number' ? num(v) : shown(v) });
      })))));
    const more = rows.length - shownRows.length;
    return env.h('div', { class: 'gu-table-box' },
      env.h('div', { class: 'gu-table-wrap' }, table),
      more > 0 ? env.h('div', { class: 'gu-more', text: `${num(more)} more ${more === 1 ? 'row' : 'rows'}${c.reverse ? ' before these' : ' after these'}` }) : null);
  },

  List(env, c) {
    const items = resolve(env.doc, c.items);
    if (!Array.isArray(items)) return none(env, items === MISSING ? 'Nothing yet' : `Not a list: ${shown(items)}`);
    const ordered = c.reverse ? [...items].reverse() : items;
    const limit = Number.isInteger(c.limit) && c.limit > 0 ? c.limit : 10;
    const list = env.h('ul', { class: 'gu-list' }, ordered.slice(0, limit).map((item) => env.h('li', {
      text: typeof c.text === 'string' && item && typeof item === 'object' ? fillItem(item, c.text) : shown(item),
    })));
    const more = items.length - Math.min(items.length, limit);
    return more > 0 ? env.h('div', null, list, env.h('div', { class: 'gu-more', text: `${num(more)} more` })) : list;
  },

  Chart: chart,

  // -------------------------------------------------------------- files

  Image(env, c) {
    const file = typeof c.file === 'string' ? c.file : '';
    const alt = String(c.alt ?? '');
    const img = env.h('img', { src: env.rawUrl(file), alt, loading: 'lazy', decoding: 'async' });
    img.addEventListener('error', () => img.replaceWith(none(env, `No picture at ${file}`)), { once: true });
    return env.h('figure', { class: 'gu-image' }, img, alt ? env.h('figcaption', { text: alt }) : null);
  },

  Code(env, c) {
    const file = typeof c.file === 'string' ? c.file : '';
    const body = env.h('div', { class: 'gu-code-body' });
    const head = env.h('div', { class: 'gu-file-head' },
      env.h('span', { class: 'gu-file-name', text: file ? `${file}${c.lines ? `:${c.lines}` : ''}` : (c.language || 'code') }),
      file ? openButton(env, file, firstLine(c.lines)) : null);
    if (c.text != null || !file) {
      body.append(...codeLines(env.h, text(env, c.text ?? ''), [], [], 1, 0));
    } else {
      body.append(env.h('div', { class: 'gu-none', text: 'Reading…' }));
      env.code(c).then((got) => {
        if (got.error) return body.replaceChildren(none(env, got.error));
        const [from, to] = range(c.lines);
        body.replaceChildren(...codeLines(env.h, got.text, got.runs || [], got.classes || [], from, to));
      }, (error) => body.replaceChildren(none(env, error.message)));
    }
    return env.h('div', { class: 'gu-file' }, head, body);
  },

  Diff(env, c) {
    const kept = c.patch != null ? { patch: String(c.patch) } : (env.diffs?.[c.id] || { error: 'the call kept no diff' });
    const lines = kept.patch ? kept.patch.split('\n') : [];
    let added = 0;
    let removed = 0;
    for (const line of lines) {
      if (line.startsWith('+') && !line.startsWith('+++')) added++;
      else if (line.startsWith('-') && !line.startsWith('---')) removed++;
    }
    const file = typeof c.file === 'string' ? c.file : '';
    const head = env.h('div', { class: 'gu-file-head' },
      env.h('span', { class: 'gu-file-name', text: file || 'patch' }),
      file ? env.h('span', { class: 'gu-dim', text: `against ${c.against || 'HEAD'}` }) : null,
      env.h('span', { class: 'gu-stat' }, env.h('span', { class: 'gu-add', text: `+${added}` }), env.h('span', { class: 'gu-del', text: `−${removed}` })),
      file ? openButton(env, file) : null);
    const body = env.h('div', { class: 'gu-diff-body' });
    if (kept.error) body.append(none(env, kept.error));
    else if (!added && !removed) body.append(none(env, 'No changes'));
    else body.append(...diffLines(env.h, lines));
    if (kept.cut) body.append(env.h('div', { class: 'gu-more', text: 'The diff was cut at 64 KB.' }));
    return env.h('div', { class: 'gu-file gu-diff' }, head, body);
  },

  FileLink(env, c) {
    const file = typeof c.file === 'string' ? c.file : '';
    const line = Number.isInteger(c.line) ? c.line : null;
    // A label is what the agent called the place; the line is in the tip then.
    const name = c.label ? text(env, c.label) : `${file}${line ? `:${line}` : ''}`;
    return env.h('button', { class: 'gu-link', type: 'button', title: `Open ${file}${line ? `:${line}` : ''}`, onclick: () => env.openFile(file, line) },
      env.h('span', { class: 'gu-link-mark', text: '↗' }), name);
  },

  // -------------------------------------------------------------- inputs

  Button(env, c) {
    const used = !!c.once && env.used(c.id);
    const style = ['primary', 'danger'].includes(c.style) ? c.style : 'default';
    return env.h('button', {
      class: 'gu-btn', type: 'button', data: { style, used: used ? 'true' : null },
      disabled: !env.live || used || truthy(resolve(env.doc, c.disabled)),
      title: used ? 'Done: this button works once' : (c.action?.event ? `Tells the agent: ${c.action.event.name}` : null),
      onclick: () => env.act(c),
    }, label(env, c), used ? ' ✓' : null);
  },

  TextField(env, c) {
    const ptr = pointer(c.value);
    const value = resolve(env.doc, c.value);
    const attrs = { class: 'gu-input', placeholder: c.placeholder != null ? text(env, c.placeholder) : null, disabled: !env.live || !ptr, data: { guKey: c.id } };
    const input = c.multiline ? env.h('textarea', { ...attrs, rows: 3 }) : env.h('input', { ...attrs, type: 'text' });
    input.value = value === MISSING || value == null ? '' : String(value);
    // Typing changes the data; what shows it is drawn again once the
    // field is left, which keeps the field as it is while it is typed in.
    input.addEventListener('input', () => env.edit(ptr, input.value, false));
    input.addEventListener('change', () => env.edit(ptr, input.value, true));
    return field(env, c, input);
  },

  Toggle(env, c) {
    const ptr = pointer(c.value);
    const box = env.h('input', { class: 'gu-switch', type: 'checkbox', disabled: !env.live || !ptr, data: { guKey: c.id } });
    box.checked = resolve(env.doc, c.value) === true;
    box.addEventListener('change', () => env.edit(ptr, box.checked, true));
    return env.h('label', { class: 'gu-toggle' }, box, env.h('span', { text: label(env, c) }));
  },

  Select(env, c) {
    const ptr = pointer(c.value);
    const options = (Array.isArray(c.options) ? c.options : []).map((o) => (o && typeof o === 'object'
      ? { value: o.value, label: o.label != null ? String(o.label) : shown(o.value) }
      : { value: o, label: shown(o) }));
    const current = resolve(env.doc, c.value);
    const at = options.findIndex((o) => o.value === current || (current !== MISSING && current != null && String(o.value) === String(current)));
    // Options go by their place, which keeps their values' types.
    const select = env.h('select', { class: 'gu-select', disabled: !env.live || !ptr, data: { guKey: c.id } },
      at < 0 ? env.h('option', { value: '', text: '—' }) : null,
      options.map((o, i) => env.h('option', { value: String(i), text: o.label })));
    select.value = at < 0 ? '' : String(at);
    select.addEventListener('change', () => {
      const o = options[Number(select.value)];
      if (o) env.edit(ptr, o.value, true);
    });
    return field(env, c, select);
  },

  Slider(env, c) {
    const ptr = pointer(c.value);
    const min = asNumber(c.min) ?? 0;
    const max = asNumber(c.max) ?? 100;
    const step = asNumber(c.step) ?? 1;
    const value = asNumber(resolve(env.doc, c.value));
    const range = env.h('input', { class: 'gu-range', type: 'range', min, max, step, disabled: !env.live || !ptr, data: { guKey: c.id } });
    range.value = String(value ?? min);
    const out = env.h('output', { class: 'gu-range-value', text: value == null ? '—' : num(value) });
    range.addEventListener('input', () => {
      out.textContent = num(Number(range.value));
      env.edit(ptr, Number(range.value), false);
    });
    range.addEventListener('change', () => env.edit(ptr, Number(range.value), true));
    return env.h('label', { class: 'gu-field' },
      env.h('span', { class: 'gu-field-label' }, env.h('span', { text: label(env, c) }), out),
      range);
  },

  // An Html is a page of its own: what draws the surface frames it.
  Html(env, c) {
    if (env.frame) return env.frame(c);
    return env.h('div', { class: 'gu-unknown' },
      env.h('span', { class: 'gu-badge', data: { tone: 'neutral' }, text: 'HTML' }),
      env.h('span', { class: 'gu-unknown-alt', text: String(c.alt ?? '') }),
      env.h('span', { class: 'gu-dim', text: 'Live HTML is not drawn here: this is what it shows.' }));
  },
};

function unknown(env, c) {
  return env.h('div', { class: 'gu-unknown' },
    env.h('span', { class: 'gu-badge', data: { tone: 'neutral' }, text: String(c.component ?? '?') }),
    env.h('span', { class: 'gu-unknown-alt', text: String(c.alt ?? 'not drawn here') }));
}

function none(env, message) {
  return env.h('div', { class: 'gu-none', text: message });
}

function field(env, c, control) {
  const name = label(env, c);
  return env.h('label', { class: 'gu-field' }, name ? env.h('span', { class: 'gu-field-label', text: name }) : null, control);
}

function openButton(env, file, line = null) {
  return env.h('button', { class: 'gu-open', type: 'button', title: `Open ${file}`, onclick: () => env.openFile(file, line) }, 'Open ↗');
}

function range(lines) {
  const m = /^(\d+)(?:-(\d+))?$/.exec(typeof lines === 'string' ? lines.trim() : '');
  if (!m) return [1, 0];
  const from = Number(m[1]);
  return [from, Number(m[2] || from)];
}

const firstLine = (lines) => range(lines)[0] || null;

// ---------------------------------------------------------------- code and diffs

// codeLines draws lines from to to (0: the last) of text, highlighted as
// the cockpit's file view does: runs are lengths in UTF-16 units and
// indexes into classes, pair by pair.
function codeLines(h, text, runs, classes, from, to) {
  const starts = [0];
  for (let at = text.indexOf('\n'); at >= 0; at = text.indexOf('\n', at + 1)) starts.push(at + 1);
  if (starts.length > 1 && starts[starts.length - 1] === text.length) starts.pop();
  const count = starts.length;
  const first = Math.max(1, Math.min(from || 1, count));
  const last = Math.min(count, to > 0 ? to : count, first + LINES - 1);
  const bounds = [];
  for (let i = 0, at = 0; i + 1 < runs.length; i += 2) {
    bounds.push([at, at + runs[i], classes[runs[i + 1]]]);
    at += runs[i];
  }
  const out = [];
  let r = 0;
  for (let n = first; n <= last; n++) {
    const start = starts[n - 1];
    const end = n < count ? starts[n] - 1 : text.endsWith('\n') ? text.length - 1 : text.length;
    while (r < bounds.length && bounds[r][1] <= start) r++;
    const content = h('span', { class: 'gu-lt' });
    let pos = start;
    let k = r;
    while (pos < end) {
      if (k >= bounds.length) {
        content.append(text.slice(pos, end));
        break;
      }
      const [s, e, cls] = bounds[k];
      if (s > pos) {
        const stop = Math.min(end, s);
        content.append(text.slice(pos, stop));
        pos = stop;
        continue;
      }
      const stop = Math.min(end, e);
      if (stop > pos) content.append(cls ? h('span', { class: `tk-${cls}`, text: text.slice(pos, stop) }) : text.slice(pos, stop));
      pos = stop;
      if (pos >= e) k++;
    }
    if (start >= end) content.append('\u200b');
    out.push(h('div', { class: 'gu-line' }, h('span', { class: 'gu-ln', text: String(n) }), content));
  }
  if (to <= 0 && count > last) out.push(h('div', { class: 'gu-more', text: `${num(count - last)} more lines` }));
  return out;
}

// diffLines draws a unified diff's hunks, with the lines' numbers old and new.
function diffLines(h, lines) {
  const out = [];
  let old = 0;
  let now = 0;
  let inHunk = false;
  let drawn = 0;
  let left = 0;
  for (const line of lines) {
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/.exec(line);
    if (hunk) {
      old = Number(hunk[1]);
      now = Number(hunk[2]);
      inHunk = true;
      if (drawn < LINES) out.push(h('div', { class: 'gu-dl', data: { t: 'hunk' } }, h('span', { class: 'gu-ln' }), h('span', { class: 'gu-ln' }), h('span', { class: 'gu-lt', text: line })));
      continue;
    }
    if (!inHunk || line.startsWith('\\')) continue;
    const t = line[0] === '+' ? 'add' : line[0] === '-' ? 'del' : 'ctx';
    if (t !== 'ctx' || line.startsWith(' ')) {
      if (drawn >= LINES) {
        left++;
      } else {
        out.push(h('div', { class: 'gu-dl', data: { t } },
          h('span', { class: 'gu-ln', text: t === 'add' ? '' : String(old) }),
          h('span', { class: 'gu-ln', text: t === 'del' ? '' : String(now) }),
          h('span', { class: 'gu-lt', text: line.slice(1) || '\u200b' })));
        drawn++;
      }
    }
    if (t !== 'add') old++;
    if (t !== 'del') now++;
  }
  if (left) out.push(h('div', { class: 'gu-more', text: `${num(left)} more lines` }));
  return out;
}

// ---------------------------------------------------------------- charts

// ticks are round values from lo to hi, about count of them, and the step.
function ticks(lo, hi, count) {
  const raw = (hi - lo) / count;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw * 0.999) || 10 * mag;
  return step;
}

function svgEl(tag, attrs = {}) {
  const el = document.createElementNS(SVG, tag);
  for (const [key, value] of Object.entries(attrs)) if (value != null) el.setAttribute(key, String(value));
  return el;
}

function chart(env, c) {
  const { h } = env;
  const fields = (typeof c.y === 'string' ? [c.y] : Array.isArray(c.y) ? c.y : []).filter((f) => typeof f === 'string').slice(0, SERIES);
  const alt = String(c.alt ?? '');
  const unit = c.unit != null ? String(c.unit) : '';
  const box = h('figure', { class: 'gu-chart', role: 'img', 'aria-label': alt });
  let data = resolve(env.doc, c.data);
  if (!Array.isArray(data) || !data.length || !fields.length) {
    box.append(none(env, data === MISSING ? `No points yet · ${alt}` : `No points · ${alt}`));
    return box;
  }
  const limit = Number.isInteger(c.limit) && c.limit > 0 ? Math.min(c.limit, POINTS) : POINTS;
  data = data.slice(-limit);
  const n = data.length;
  const at = (p, f) => asNumber(p && typeof p === 'object' ? p[f] : p);
  const series = fields.map((f) => data.map((p) => at(p, f)));
  const xs = data.map((p, i) => (typeof c.x === 'string' && p && typeof p === 'object' && p[c.x] != null ? shown(p[c.x]) : String(i + 1)));
  let lo = Infinity;
  let hi = -Infinity;
  for (const s of series) for (const v of s) if (v != null) { lo = Math.min(lo, v); hi = Math.max(hi, v); }
  if (!Number.isFinite(lo)) {
    box.append(none(env, `No numbers in ${fields.join(', ')} · ${alt}`));
    return box;
  }
  const kind = ['line', 'bar', 'points'].includes(c.kind) ? c.kind : 'line';
  if (kind === 'bar') { lo = Math.min(lo, 0); hi = Math.max(hi, 0); }
  if (lo === hi) { lo -= Math.abs(lo) * 0.1 || 1; hi += Math.abs(hi) * 0.1 || 1; }
  const step = ticks(lo, hi, 3);
  lo = Math.floor(lo / step + 1e-9) * step;
  hi = Math.ceil(hi / step - 1e-9) * step;
  const W = 1000;
  const H = 1000;
  const X = (i) => (kind === 'bar' ? ((i + 0.5) / n) * W : n === 1 ? W / 2 : (i / (n - 1)) * W);
  const Y = (v) => (1 - (v - lo) / (hi - lo)) * H;
  const value = (v) => (v == null ? '—' : `${num(v)}${unit ? ` ${unit}` : ''}`);

  const plot = svgEl('svg', { viewBox: `0 0 ${W} ${H}`, preserveAspectRatio: 'none', 'aria-hidden': 'true', class: 'gu-plot' });
  const yaxis = h('div', { class: 'gu-yaxis' });
  let chars = 1; // the widest label, for the axis's width
  for (let t = lo; t <= hi + step * 1e-6; t += step) {
    const y = Y(t);
    const tick = num(Math.abs(t) < step * 1e-6 ? 0 : Number(t.toPrecision(12)));
    chars = Math.max(chars, tick.length);
    plot.append(svgEl('line', { x1: 0, x2: W, y1: y, y2: y, class: Math.abs(t) < step * 1e-6 && lo < 0 ? 'gu-grid-line gu-zero' : 'gu-grid-line', 'vector-effect': 'non-scaling-stroke' }));
    yaxis.append(setVar(h('span', { text: tick }), '--gu-at', y / H));
  }
  setVar(yaxis, '--gu-chars', chars);
  if (kind === 'bar') {
    const group = W / n;
    const width = (group * 0.72) / series.length;
    series.forEach((s, j) => s.forEach((v, i) => {
      if (v == null) return;
      const y0 = Y(Math.max(lo, Math.min(hi, 0)));
      const y1 = Y(v);
      plot.append(svgEl('rect', { x: i * group + group * 0.14 + j * width, y: Math.min(y0, y1), width: Math.max(width, 0.5), height: Math.max(Math.abs(y1 - y0), 0.5), class: `gu-bar-${j}`, 'shape-rendering': 'crispEdges' }));
    }));
  } else {
    series.forEach((s, j) => {
      let d = '';
      let pen = false;
      s.forEach((v, i) => {
        if (v == null) { pen = false; return; }
        if (kind === 'points') d += `M${X(i).toFixed(2)} ${Y(v).toFixed(2)}h0`;
        else { d += `${pen ? 'L' : 'M'}${X(i).toFixed(2)} ${Y(v).toFixed(2)}`; pen = true; }
      });
      if (d) plot.append(svgEl('path', { d, class: `gu-series gu-s-${j}${kind === 'points' ? ' gu-dots' : ''}`, 'vector-effect': 'non-scaling-stroke' }));
      // The newest point, marked.
      const lastAt = s.findLastIndex((v) => v != null);
      if (kind === 'line' && lastAt >= 0) plot.append(svgEl('path', { d: `M${X(lastAt).toFixed(2)} ${Y(s[lastAt]).toFixed(2)}h0`, class: `gu-series gu-s-${j} gu-dots gu-last`, 'vector-effect': 'non-scaling-stroke' }));
    });
  }

  // Pointing at the chart shows the values there.
  const guide = h('div', { class: 'gu-guide', hidden: true });
  const tip = h('div', { class: 'gu-tip', hidden: true });
  const area = h('div', { class: 'gu-area' }, plot, guide, tip);
  if (Number.isInteger(c.height) && c.height >= 60) setVar(area, '--gu-h', `${Math.min(c.height, 600)}px`);
  area.addEventListener('pointermove', (event) => {
    const rect = area.getBoundingClientRect();
    const share = Math.max(0, Math.min(1, (event.clientX - rect.left) / rect.width));
    const i = kind === 'bar' ? Math.min(n - 1, Math.floor(share * n)) : Math.round(share * (n - 1));
    setVar(guide, '--gu-x', X(i) / W);
    setVar(tip, '--gu-x', X(i) / W);
    tip.dataset.side = X(i) / W > 0.62 ? 'left' : 'right';
    tip.replaceChildren(h('b', { text: xs[i] }), ...fields.map((f, j) => h('span', { class: 'gu-tip-row' },
      h('i', { class: `gu-swatch gu-s-${j}` }), `${f} `, h('span', { class: 'gu-tip-value', text: value(series[j][i]) }))));
    guide.hidden = false;
    tip.hidden = false;
  });
  area.addEventListener('pointerleave', () => { guide.hidden = true; tip.hidden = true; });

  const legend = h('figcaption', { class: 'gu-legend' },
    h('span', { class: 'gu-legend-alt', text: alt }),
    fields.map((f, j) => {
      const s = series[j];
      const last = s.findLast((v) => v != null);
      return h('span', { class: 'gu-legend-item' }, h('i', { class: `gu-swatch gu-s-${j}` }), `${f} `, h('b', { text: value(last) }));
    }));
  const xaxis = h('div', { class: 'gu-xaxis' }, h('span', { text: xs[0] }), n > 2 ? h('span', { text: xs[Math.floor((n - 1) / 2)] }) : null, n > 1 ? h('span', { text: xs[n - 1] }) : null);
  box.dataset.kind = kind;
  box.append(legend, h('div', { class: 'gu-chart-grid' }, yaxis, area, h('span'), xaxis));
  return box;
}
