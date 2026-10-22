// The canvas's part of the rail (layout.view "canvas"): a new canvas, the
// way back to the sessions, the workspace's canvases, and for the canvas in
// view its nodes — Outline: what each does now and what waits for it; a
// click shows one, a double-click enters it — and what happened on it:
// Activity, the latest first, each line taking the board to its node or
// edge.
import { glyphOf } from './geometry.js';
import { shownState, stateWord } from './status.js';

const SHOWN_ACTIVITY = 80;

// outlineRows are the rows of a canvas's nodes, in reading order: the
// rail's Outline, and the list a phone shows in place of the board.
export function outlineRows(env, model, { selected = [], onPick, onEnter } = {}) {
  const { h } = env;
  if (!model) return [];
  return outlineFacts(env, model, selected).map((f) => h('button', {
    class: 'cv-outline-row', type: 'button', data: { id: f.id, state: f.state || null, kind: f.kind },
    'aria-current': f.current ? 'true' : null,
    title: f.tip,
    onclick: () => onPick?.(f.id),
    ondblclick: () => onEnter?.(f.id),
  },
  h('span', { class: 'cv-outline-glyph', 'aria-hidden': 'true', text: f.glyph }),
  h('span', { class: 'cv-outline-title', text: f.title }),
  h('span', { class: 'cv-outline-state', data: { waiting: f.waiting ? 'true' : null }, text: f.word })));
}

// outlineKey is all the rows of outlineRows show, as one string: while it
// is the same, the rows drawn are kept — a click, the focus, a tooltip on
// one is not lost to a status that changes nothing they show.
export function outlineKey(env, model, selected = []) {
  if (!model) return '';
  return JSON.stringify([model.loading, ...outlineFacts(env, model, selected).map((f) => Object.values(f))]);
}

function outlineFacts(env, model, selected) {
  const chosen = new Set(selected);
  const nodes = [...model.doc.nodes].sort((a, b) => (a.y - b.y) || (a.x - b.x));
  return nodes.map((n) => {
    const st = model.status.get(n.id);
    const state = n.kind === 'note' ? '' : n.proposed ? 'paused' : shownState(st, n.kind === 'agent' ? 'idle' : 'stopped');
    const waiting = model.pendingFor(n.id);
    return {
      id: n.id,
      kind: n.kind,
      state,
      waiting,
      word: [state ? stateWord(state) : '', waiting ? `${waiting} pending` : ''].filter(Boolean).join(' · '),
      current: chosen.has(n.id),
      title: n.title || n.id,
      tip: [n.title || n.id, st?.detail || '', 'Click to show it, double-click to enter it'].filter(Boolean).join('\n'),
      glyph: glyphOf(n, env.kinds),
    };
  });
}

// keepRows gives the rows drawn anew the elements a list has for the same
// IDs: such a row stays the element it is and takes what changed in place
// — a click on it, the focus or its tooltip is not lost to a status that
// came meanwhile.
export function keepRows(list, rows) {
  const have = new Map();
  for (const el of list.children) if (el.dataset.id) have.set(el.dataset.id, el);
  return rows.map((row) => {
    const old = have.get(row.dataset.id);
    if (!old || old.nodeName !== row.nodeName || old.children.length !== row.children.length) return row;
    sameAttributes(old, row);
    [...row.children].forEach((child, i) => {
      const was = old.children[i];
      sameAttributes(was, child);
      if (was.textContent !== child.textContent) was.textContent = child.textContent;
    });
    return old;
  });
}

function sameAttributes(old, fresh) {
  for (const { name } of [...old.attributes]) if (!fresh.hasAttribute(name)) old.removeAttribute(name);
  for (const { name, value } of [...fresh.attributes]) if (old.getAttribute(name) !== value) old.setAttribute(name, value);
}

// place makes a list's children the ones given, in their order, moving
// only those out of place; the focus stays where it was.
export function place(list, children) {
  const active = document.activeElement;
  const keep = new Set(children);
  for (const el of [...list.children]) if (!keep.has(el)) el.remove();
  let at = list.firstElementChild;
  for (const el of children) {
    if (el === at) at = at.nextElementSibling;
    else list.insertBefore(el, at);
  }
  if (active && active !== document.activeElement && active.isConnected) active.focus({ preventScroll: true });
}

export function createRail(env) {
  const { h, fmt } = env;
  const newButton = h('button', { class: 'new', type: 'button', title: 'A new, empty canvas', onclick: () => env.create() }, h('span', { text: 'New canvas' }));
  const back = h('button', { class: 'icon cv-back', type: 'button', title: 'Back to the sessions', 'aria-label': 'Back to the sessions', onclick: () => env.back() }, '←');
  const listCount = h('span', { text: '0' });
  const list = h('nav', { class: 'cv-canvases', 'aria-label': 'Canvases' });
  const outlineCount = h('span');
  const outline = h('div', { class: 'cv-outline', role: 'list', 'aria-label': 'The nodes of the canvas' });
  const activity = h('ol', { class: 'cv-activity', 'aria-label': 'What happened on the canvas' });
  const outlineLabel = h('div', { class: 'rail-label' }, h('span', { text: 'Outline' }), outlineCount);
  const activityLabel = h('div', { class: 'rail-label' }, h('span', { text: 'Activity' }));
  const scroll = h('div', { class: 'cv-rail-scroll' }, h('div', { class: 'rail-label' }, h('span', { text: 'Canvases' }), listCount), list,
    outlineLabel, outline, activityLabel, activity);
  const element = h('div', { class: 'rail-view cv-rail', data: { view: 'canvas' } },
    h('div', { class: 'rail-tools' }, newButton, back), scroll);

  // ---------------------------------------------------------------- canvases

  function renderList() {
    const canvases = env.canvases() || [];
    const current = env.current();
    const rows = [];
    const known = canvases.some((c) => c.id === current?.id);
    // A canvas made here and not saved yet heads the list.
    if (current && !known && current.fresh) {
      rows.push(row({ id: current.id, title: current.title || 'New canvas', fresh: true }, true));
    }
    for (const c of canvases) rows.push(row(c, current?.id === c.id));
    if (!rows.length) rows.push(h('p', { class: 'sessions-empty', text: 'No canvases in this workspace yet.' }));
    const focused = document.activeElement?.closest?.('.cv-canvases a.session')?.dataset.id;
    list.replaceChildren(...rows);
    if (focused) list.querySelector(`a.session[data-id="${CSS.escape(focused)}"]`)?.focus();
    listCount.textContent = String(canvases.length);
  }

  function row(c, current) {
    const title = c.title || 'Canvas';
    const meta = c.fresh ? 'unsaved' : env.metaOf(c);
    const items = c.fresh ? null : env.menuOf?.(c);
    return h('div', { class: 'session-row', data: { id: c.id } },
      h('a', {
        class: 'session', href: env.hash(c.id), title, 'aria-current': current ? 'true' : null,
        data: { id: c.id, running: c.busy ? 'true' : null },
        onclick: (event) => {
          if (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
          event.preventDefault();
          env.open(c.id);
        },
      },
      h('span', { class: 'n', text: '◧' }),
      h('span', { class: 't', text: title }),
      h('span', { class: 'm', text: meta })),
      items?.length ? h('button', {
        class: 'more', type: 'button', title: 'Canvas actions', 'aria-label': `Actions for ${title}`, 'aria-haspopup': 'menu', 'aria-expanded': 'false',
        onclick: (event) => {
          event.preventDefault();
          event.stopPropagation();
          env.cockpit.use('menu')?.toggle?.(event.currentTarget, () => env.menuOf(c), { compact: true });
        },
      }, h('span', { 'aria-hidden': 'true', text: '⋯' })) : null);
  }

  // ---------------------------------------------------------------- outline

  let outlineDrawn = null;
  function renderOutline() {
    const m = env.model();
    outlineCount.textContent = m && m.doc.nodes.length ? String(m.doc.nodes.length) : '';
    outlineLabel.hidden = !m;
    outline.hidden = !m;
    const selected = env.selected();
    const key = outlineKey(env, m, selected);
    if (key === outlineDrawn) return;
    outlineDrawn = key;
    const rows = outlineRows(env, m, {
      selected,
      onPick: (id) => env.reveal(id),
      onEnter: (id) => env.enter(id),
    });
    place(outline, rows.length ? keepRows(outline, rows) : [h('p', { class: 'sessions-empty', text: m?.loading ? 'Loading…' : 'No nodes yet: + Add, or ask the foreman.' })]);
  }

  // ---------------------------------------------------------------- activity

  // The rows of Activity, by the item each shows: an item that came adds
  // its row, and the others stay as they are (text selected in one, the
  // focus on one).
  let activityRows = new WeakMap();
  let activityOf = null;
  function renderActivity() {
    const m = env.model();
    const items = (m?.activity || []).slice(-SHOWN_ACTIVITY).reverse();
    activityLabel.hidden = !m;
    activity.hidden = !m;
    if (!m) return;
    if (activityOf !== m) {
      activityOf = m;
      activityRows = new WeakMap();
    }
    if (!items.length) {
      activity.replaceChildren(h('li', { class: 'cv-activity-none', text: 'Outputs, messages and what agents do show here.' }));
      return;
    }
    place(activity, items.map((item) => {
      const target = item.node && m.node(item.node) ? { node: item.node } : item.edge && m.edge(item.edge) ? { edge: item.edge } : null;
      const key = target ? `${target.node ? 'n' : 'e'}:${target.node || target.edge}` : '';
      const kept = activityRows.get(item);
      if (kept?.key === key) return kept.row;
      const text = h(target ? 'button' : 'span', {
        class: 'cv-activity-text', type: target ? 'button' : null, text: item.text || '',
        title: target ? `${item.text}\nShow it on the board` : item.text,
        onclick: target ? () => (target.node ? env.reveal(target.node) : env.revealEdge(target.edge)) : null,
      });
      const row = h('li', { data: { kind: item.kind || null, level: item.level || null } },
        h('time', { text: fmt.clock(item.at), title: fmt.stamp(item.at) }), text);
      activityRows.set(item, { key, row });
      return row;
    }));
  }

  // render draws what changed by the next frame, while the rail shows.
  let frame = 0;
  const parts = new Set();
  function render(part = 'all') {
    if (part === 'all') ['list', 'outline', 'activity'].forEach((p) => parts.add(p));
    else parts.add(part);
    if (frame) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      if (element.hidden || !element.isConnected) return;
      const now = [...parts];
      parts.clear();
      if (now.includes('list')) renderList();
      if (now.includes('outline')) renderOutline();
      if (now.includes('activity')) renderActivity();
    });
  }

  return {
    element,
    render,
    dispose: () => cancelAnimationFrame(frame),
  };
}
