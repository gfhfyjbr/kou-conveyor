// data: a surface's data model as ui.py makes it — JSON Pointers, the
// sources' formats, templates, numbers and a Metric's change — so a surface
// reads the same in the page as in the text the agent got back. What is
// here mirrors ui.py; a change to one is a change to the other.

export const MISSING = Symbol('missing');

// Limits, as ui.py's.
export const MAX_ROWS = 10000;
export const MAX_TEXT = 64 << 10;

export const MARKER = /^UI ([a-z0-9][a-z0-9_-]*) v(\d+)( deleted)? \((\.harness\/ui\/[A-Za-z0-9_./-]+?\.json)\)/;
export const TEMPLATE = /\{(\/[^{}]*|#[A-Za-z0-9][A-Za-z0-9_.-]*)\}/g;
const NUMBER = /^-?(\d+\.?\d*|\.\d+)([eE][-+]?\d+)?$/;

// ---------------------------------------------------------------- JSON Pointer

export function tokens(pointer) {
  if (pointer === '') return [];
  if (typeof pointer !== 'string' || !pointer.startsWith('/')) return null;
  return pointer.slice(1).split('/').map((t) => t.replaceAll('~1', '/').replaceAll('~0', '~'));
}

// get finds what pointer names in doc, or MISSING. A negative index counts
// from the end of a list: -1 is its last element.
export function get(doc, pointer) {
  const list = tokens(pointer);
  if (!list) return MISSING;
  let current = doc;
  for (const token of list) {
    if (Array.isArray(current)) {
      if (!/^-?\d+$/.test(token)) return MISSING;
      let i = Number(token);
      if (i < 0) i += current.length;
      if (i < 0 || i >= current.length) return MISSING;
      current = current[i];
    } else if (current && typeof current === 'object') {
      if (!Object.hasOwn(current, token)) return MISSING;
      current = current[token];
    } else {
      return MISSING;
    }
  }
  return current;
}

// set sets (or with null removes) what pointer names in doc, an object,
// making the objects on the way.
export function set(doc, pointer, value) {
  const list = tokens(pointer);
  if (!list?.length) return doc;
  let current = doc;
  for (const token of list.slice(0, -1)) {
    if (Array.isArray(current)) {
      const i = /^\d+$/.test(token) ? Number(token) : -1;
      if (i < 0 || i >= current.length) return doc;
      current = current[i];
      continue;
    }
    let next = current[token];
    if (!next || typeof next !== 'object') {
      next = {};
      current[token] = next;
    }
    current = next;
  }
  const last = list[list.length - 1];
  if (Array.isArray(current)) {
    if (last === '-' && value != null) current.push(value);
    const i = /^\d+$/.test(last) ? Number(last) : -1;
    if (i >= 0 && i < current.length) {
      if (value == null) current.splice(i, 1);
      else current[i] = value;
    }
    return doc;
  }
  if (value == null) delete current[last];
  else current[last] = value;
  return doc;
}

export const isBinding = (v) => !!v && typeof v === 'object' && !Array.isArray(v) && Object.keys(v).length === 1 && typeof v.path === 'string';

export const resolve = (doc, v) => (isBinding(v) ? get(doc, v.path) : v);

// ---------------------------------------------------------------- sources

export function inferFormat(path) {
  const ext = (/\.[^./]*$/.exec(path)?.[0] || '').toLowerCase();
  return { '.json': 'json', '.jsonl': 'jsonl', '.ndjson': 'jsonl', '.csv': 'csv', '.tsv': 'tsv', '.md': 'markdown', '.markdown': 'markdown' }[ext] || 'text';
}

function cell(text) {
  const t = text.trim();
  if (NUMBER.test(t)) return /^-?\d+$/.test(t) ? parseInt(t, 10) : parseFloat(t);
  return text;
}

// csvRows reads CSV as Python's csv.reader does by default: quotes start a
// field, "" in one is a quote, and a blank line is an empty row.
function csvRows(text, delimiter) {
  const rows = [];
  let row = [];
  let field = '';
  let quoted = false;
  let wasQuoted = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      if (c === '"') {
        if (text[i + 1] === '"') { field += '"'; i++; } else quoted = false;
      } else field += c;
      continue;
    }
    if (c === '"' && field === '' && !wasQuoted) { quoted = true; wasQuoted = true; continue; }
    if (c === delimiter) { row.push(field); field = ''; wasQuoted = false; continue; }
    if (c === '\n' || c === '\r') {
      if (c === '\r' && text[i + 1] === '\n') i++;
      if (row.length || field !== '' || wasQuoted) row.push(field);
      rows.push(row);
      row = [];
      field = '';
      wasQuoted = false;
      continue;
    }
    field += c;
  }
  if (row.length || field !== '' || wasQuoted) {
    row.push(field);
    rows.push(row);
  }
  return rows;
}

// parseSource is what a source's text is in the data model: throws when it
// is not of its format.
export function parseSource(format, text) {
  if (format === 'json') return JSON.parse(text);
  if (format === 'jsonl') {
    let rows = [];
    // Lines end where Python's splitlines ends them.
    for (const raw of text.split(/\r\n|[\n\r\v\f\x1c\x1d\x1e\x85\u2028\u2029]/)) {
      const line = raw.trim();
      if (!line) continue;
      try { rows.push(JSON.parse(line)); } catch { /* a line being written, or a stray one */ }
    }
    const count = rows.length;
    rows = rows.slice(-MAX_ROWS);
    return { rows, last: rows.length ? rows[rows.length - 1] : null, count };
  }
  if (format === 'csv' || format === 'tsv') {
    const table = csvRows(text, format === 'csv' ? ',' : '\t');
    if (!table.length) return { columns: [], rows: [], last: null, count: 0 };
    const columns = table[0].map((c) => c.trim());
    let rows = [];
    for (const r of table.slice(1)) {
      if (!r.some((c) => c.trim())) continue;
      const row = {};
      r.forEach((c, i) => { row[i < columns.length && columns[i] ? columns[i] : `col${i + 1}`] = cell(c); });
      rows.push(row);
    }
    const count = rows.length;
    rows = rows.slice(-MAX_ROWS);
    return { columns, rows, last: rows.length ? rows[rows.length - 1] : null, count };
  }
  return text.slice(0, MAX_TEXT);
}

// ---------------------------------------------------------------- numbers and text

export function asNumber(value) {
  if (typeof value === 'number') return value;
  if (typeof value === 'string' && NUMBER.test(value.trim())) return parseFloat(value);
  return null;
}

// num is a number as a surface shows it: digits fixed, or else as few as
// say it (8.5, 41.6, 0.12), and thousands apart (1 204).
export function num(value, digits) {
  if (typeof value !== 'number') return String(value);
  if (Number.isNaN(value)) return 'nan';
  if (!Number.isFinite(value)) return value > 0 ? 'inf' : '-inf';
  const fixed = Number.isInteger(digits) && digits >= 0;
  let d = digits;
  if (!fixed) {
    const a = Math.abs(value);
    d = Number.isInteger(value) || a >= 100 ? 0 : a >= 10 ? 1 : 2;
  }
  let [whole, part] = Math.abs(value).toFixed(Math.min(d, 20)).split('.');
  whole = whole.replace(/\B(?=(\d{3})+(?!\d))/g, ' ');
  let text = part !== undefined ? `${whole}.${part}` : whole;
  if (!fixed && text.includes('.')) text = text.replace(/0+$/, '').replace(/\.$/, '');
  return (value < 0 ? '-' : '') + text;
}

export function shown(value) {
  if (value === MISSING || value == null) return '—';
  if (typeof value === 'boolean') return value ? 'true' : 'false';
  if (typeof value === 'object') return JSON.stringify(value);
  if (typeof value === 'number') return num(value);
  return String(value);
}

// delta is the change from previous to value: { text: "▲8%", dir: 1 }, or
// null when there is none to tell.
export function delta(value, previous) {
  const v = asNumber(value === MISSING ? null : value);
  const p = asNumber(previous === MISSING ? null : previous);
  if (v == null || p == null || p === 0) return null;
  const pct = ((v - p) / Math.abs(p)) * 100;
  if (Math.abs(pct) < 0.05) return { text: '=0%', dir: 0 };
  const size = Math.abs(pct) < 1 ? Math.abs(pct).toFixed(1) : Math.abs(pct).toFixed(0);
  return { text: `${pct > 0 ? '▲' : '▼'}${size}%`, dir: pct > 0 ? 1 : -1 };
}

// previousOf is a Metric's previous value: its own, or the row before the
// last of the source its value is the last row of.
export function previousOf(doc, c) {
  if (Object.hasOwn(c, 'previous')) return resolve(doc, c.previous);
  if (isBinding(c.value)) {
    const m = /^\/sources\/([^/]+)\/last\/(.+)$/.exec(c.value.path);
    if (m) return get(doc, `/sources/${m[1]}/rows/-2/${m[2]}`);
  }
  return MISSING;
}

// metric is a Metric's parts: its value as text, its unit, and its change,
// good or bad as the Metric's better says.
export function metric(doc, c) {
  const value = resolve(doc, c.value);
  const numeric = typeof value === 'number';
  const text = numeric ? num(value, c.digits) : shown(value);
  const unit = c.unit && value !== MISSING && value != null ? String(c.unit) : '';
  const change = delta(value, previousOf(doc, c));
  let tone = 'flat';
  if (change && change.dir && (c.better === 'lower' || c.better === 'higher')) {
    tone = (change.dir < 0) === (c.better === 'lower') ? 'good' : 'bad';
  }
  return { text, unit, change, tone, missing: value === MISSING || value == null };
}

export function metricText(doc, c) {
  const m = metric(doc, c);
  return `${m.text}${m.unit ? ` ${m.unit}` : ''}${m.change ? ` ${m.change.text}` : ''}`;
}

// fill puts values in a template: {/pointer} a value of doc, {#id} a Metric
// as it shows.
export function fill(doc, comps, template) {
  return String(template).replace(TEMPLATE, (_, ref) => {
    if (ref.startsWith('#')) {
      const c = comps.get(ref.slice(1));
      return c ? metricText(doc, c) : '?';
    }
    return shown(get(doc, ref));
  });
}

// fillItem fills a List's template with an item's fields.
export function fillItem(item, template) {
  return String(template).replace(TEMPLATE, (whole, ref) => (ref.startsWith('/') ? shown(get(item, ref)) : whole));
}

// textOf is a text prop as it shows: a template filled, or a value bound.
export function textOf(doc, comps, value) {
  const v = resolve(doc, value);
  if (v === MISSING) return '—';
  return typeof v === 'string' ? fill(doc, comps, v) : shown(v);
}

// ---------------------------------------------------------------- the call

// callOf reads a UI call's result: the version it made, where, and the
// title its text shows. null for a call that made none.
export function callOf(output) {
  if (typeof output !== 'string') return null;
  const m = MARKER.exec(output);
  if (!m) return null;
  const [, surface, version, deleted, path] = m;
  const call = { surface, version: Number(version), deleted: !!deleted, path, title: '', warnings: [], text: '' };
  const lines = output.split('\n');
  call.warnings = lines.filter((line) => line.startsWith('warning: ')).map((line) => line.slice(9));
  const head = lines.findIndex((line) => line.startsWith('◇ '));
  if (head >= 0) {
    const tag = ` ${surface} v${version}`;
    const line = lines[head];
    call.title = (line.endsWith(tag) ? line.slice(2, -tag.length) : line.slice(2)).trimEnd();
    call.text = lines.slice(head).join('\n');
  }
  // The folder of a version is its session's: .harness/ui/<session>/<surface>/v<N>.json.
  call.session = path.split('/')[2] || '';
  return call;
}
