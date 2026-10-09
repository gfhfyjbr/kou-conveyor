// The agents on the canvas in view, in a small window over the board: kou
// agents, the agents its terminals launch (Claude Code, Codex…) and those
// the engine found running in its shells (detect.go), each with what it
// does now and for how long. A click shows one on the board, a
// double-click enters it. The window is dragged by its header, folded to
// it and closed; the bar's Agents shows and hides it. The page keeps where
// it was and whether it was folded or closed, and shows it by itself once
// the canvas has agents, unless it was closed.
import { glyphOf, agentGlyphOf, isHarness } from './geometry.js';
import { shownState, stateWord, atWork } from './status.js';

const PREF = 'canvas.agents';
// How close to the board's edges the window may go.
const MARGIN = 8;

// agentsOf lists the agents on a canvas, in reading order: each node that
// is one, or runs one, with what it is and how it came to run.
export function agentsOf(model, kinds, modelTitle = (id) => id) {
  if (!model) return [];
  const list = [];
  for (const n of model.doc.nodes) {
    const st = model.status.get(n.id);
    let agent = null;
    if (n.kind === 'agent') {
      const name = n.preset === 'foreman' ? 'foreman' : 'kou agent';
      agent = { title: n.config?.model ? `${name} · ${modelTitle(n.config.model)}` : name, glyph: glyphOf(n, kinds), how: 'kou' };
    } else if (n.kind === 'terminal' && st?.agent) {
      agent = {
        title: st.agent.title || st.agent.id, glyph: agentGlyphOf(st.agent, kinds),
        how: st.agent.detected ? 'shell' : 'preset', pid: st.agent.pid || 0,
      };
    } else if (isHarness(n)) {
      // A harness whose agent does not run now: it exited, or has not
      // started yet.
      agent = { title: (kinds.harness(n)?.title || n.preset).replace(/…$/, ''), glyph: glyphOf(n, kinds), how: 'preset' };
    }
    if (agent) list.push({ node: n, status: st, agent });
  }
  return list.sort((a, b) => (a.node.y - b.node.y) || (a.node.x - b.node.x));
}

export function createAgents(env) {
  const { h, fmt, prefs } = env;
  const pref = read();
  let items = [];
  let placed = false; // the window was placed since it last showed

  // ---------------------------------------------------------------- elements

  const count = h('span', { class: 'cv-ag-count' });
  const sum = h('span', { class: 'cv-ag-sum' });
  const fold = h('button', { class: 'icon small', type: 'button', onclick: () => setFolded(!pref.folded) });
  const close = h('button', {
    class: 'icon small', type: 'button', title: 'Close it: the bar\'s Agents shows it again', 'aria-label': 'Close the agents\' window',
    onclick: () => setClosed(true),
  }, '×');
  const head = h('header', { class: 'cv-ag-head', title: 'Drag to move it' }, h('span', { class: 'label', text: 'Agents' }), count, sum, fold, close);
  const list = h('div', { class: 'cv-ag-list' });
  const element = h('section', { class: 'cv-agents', hidden: true, 'aria-label': 'The agents on the canvas' }, head, list);

  const buttonText = h('span', { text: 'Agents' });
  const button = h('button', {
    class: 'cv-agents-toggle', type: 'button', hidden: true, 'aria-pressed': 'false',
    onclick: () => toggle(),
  }, h('i', { 'aria-hidden': 'true' }), buttonText);

  // ---------------------------------------------------------------- what is kept

  function read() {
    const p = prefs.get(PREF, null);
    const at = (v) => (Number.isFinite(v) ? v : null);
    return p && typeof p === 'object'
      ? { closed: !!p.closed, folded: !!p.folded, x: at(p.x), y: at(p.y) }
      : { closed: false, folded: false, x: null, y: null };
  }
  const save = () => prefs.set(PREF, { ...pref });

  function setClosed(closed) {
    pref.closed = closed;
    save();
    draw();
  }

  function setFolded(folded) {
    pref.folded = folded;
    save();
    draw();
    place(pref.x ?? MARGIN + 4, pref.y ?? MARGIN + 4);
  }

  // toggle hides the window shown, or shows it.
  function toggle() {
    setClosed(!element.hidden);
  }

  // ---------------------------------------------------------------- the place

  // place puts the window at a point of the board, kept inside it. The
  // page's policy refuses style attributes: the place is set as a
  // property.
  function place(x, y) {
    const parent = element.offsetParent;
    if (!parent || element.hidden) return;
    const maxX = Math.max(MARGIN, parent.clientWidth - element.offsetWidth - MARGIN);
    const maxY = Math.max(MARGIN, parent.clientHeight - element.offsetHeight - MARGIN);
    element.style.left = `${Math.round(Math.min(maxX, Math.max(MARGIN, x)))}px`;
    element.style.top = `${Math.round(Math.min(maxY, Math.max(MARGIN, y)))}px`;
  }

  head.addEventListener('pointerdown', (event) => {
    if (event.button !== 0 || event.target.closest('button')) return;
    event.preventDefault();
    const from = { x: event.clientX, y: event.clientY, left: element.offsetLeft, top: element.offsetTop };
    try {
      head.setPointerCapture(event.pointerId);
    } catch { /* gone already */ }
    element.dataset.dragging = 'true';
    const stop = new AbortController();
    const move = (e) => {
      if (e.pointerId === event.pointerId) place(from.left + e.clientX - from.x, from.top + e.clientY - from.y);
    };
    const end = (e) => {
      if (e.pointerId !== event.pointerId) return;
      stop.abort();
      delete element.dataset.dragging;
      pref.x = element.offsetLeft;
      pref.y = element.offsetTop;
      save();
    };
    head.addEventListener('pointermove', move, { signal: stop.signal });
    head.addEventListener('pointerup', end, { signal: stop.signal });
    head.addEventListener('pointercancel', end, { signal: stop.signal });
  });
  // A double-click on the header folds the window, or unfolds it.
  head.addEventListener('dblclick', (event) => {
    if (!event.target.closest('button')) setFolded(!pref.folded);
  });

  // The board grew smaller: the window stays on it.
  const resizeObserver = new ResizeObserver(() => {
    if (!element.hidden) place(element.offsetLeft, element.offsetTop);
  });

  // ---------------------------------------------------------------- the rows

  const rows = new Map(); // node → its row

  function rowFor(id) {
    let row = rows.get(id);
    if (row) return row;
    const glyph = h('span', { class: 'cv-ag-glyph', 'aria-hidden': 'true' });
    const title = h('span', { class: 'cv-ag-title' });
    const word = h('b');
    const time = h('time');
    const meta = h('span', { class: 'cv-ag-meta' });
    const el = h('button', {
      class: 'cv-ag-row', type: 'button', data: { id },
      onclick: () => env.reveal(id),
      ondblclick: () => env.enter(id),
    }, glyph, title, h('span', { class: 'cv-ag-state' }, h('i', { 'aria-hidden': 'true' }), word, time), meta);
    row = { el, glyph, title, word, time, meta, state: '', since: 0 };
    rows.set(id, row);
    return row;
  }

  const set = (el, text) => {
    if (el.textContent !== text) el.textContent = text;
  };

  function fill(row, { node: n, status: st, agent }, m, chosen) {
    const state = n.proposed ? 'paused' : shownState(st, n.kind === 'agent' ? 'idle' : 'stopped');
    row.state = state;
    row.since = st?.since ? Date.parse(st.since) : 0;
    row.el.dataset.state = state;
    row.el.dataset.how = agent.how;
    set(row.glyph, agent.glyph);
    set(row.title, n.title || n.id);
    set(row.word, stateWord(state));
    const pending = m.pendingFor(n.id);
    const doing = (state === 'busy' || state === 'starting') && st?.activity ? st.activity : st?.detail || '';
    set(row.meta, [agent.title, agent.how === 'shell' ? 'in its shell' : '', doing, pending ? `${pending} pending` : ''].filter(Boolean).join(' · '));
    row.el.title = [
      `${n.title || n.id} — ${agent.title}${agent.how === 'shell' ? `, run in its shell${agent.pid ? ` (pid ${agent.pid})` : ''}` : ''}`,
      [stateWord(state), st?.detail || ''].filter(Boolean).join(': '),
      'Click to show it on the board, double-click to enter it',
    ].join('\n');
    if (chosen) row.el.setAttribute('aria-current', 'true');
    else row.el.removeAttribute('aria-current');
    clock(row);
  }

  // clock has a row at work say for how long.
  function clock(row) {
    set(row.time, row.since && atWork(row.state) ? fmt.duration(Date.now() - row.since) : '');
  }

  // ---------------------------------------------------------------- drawing

  // draw lists the agents of the canvas in view, and shows the window if
  // there are and it was not closed.
  function draw() {
    const m = env.model();
    items = m && !m.loading ? agentsOf(m, env.kinds, env.modelTitle) : [];
    const chosen = new Set(env.selected?.() || []);
    const ids = items.map((x) => x.node.id);
    for (const id of [...rows.keys()]) {
      if (ids.includes(id)) continue;
      rows.get(id).el.remove();
      rows.delete(id);
    }
    for (const item of items) fill(rowFor(item.node.id), item, m, chosen.has(item.node.id));
    if ([...list.children].map((el) => el.dataset.id).join() !== ids.join()) list.replaceChildren(...ids.map((id) => rows.get(id).el));

    // What they do, all told: at work (not running on quietly), waiting.
    let working = 0;
    let waiting = 0;
    let idle = 0;
    for (const row of rows.values()) {
      if (row.state === 'busy' || row.state === 'starting') working++;
      else if (row.state === 'waiting') waiting++;
      else if (row.state === 'idle' || row.state === 'running') idle++;
    }
    const overall = waiting ? 'waiting' : working ? 'busy' : idle ? 'idle' : '';
    set(count, String(items.length));
    set(sum, [working ? `${working} at work` : '', waiting ? `${waiting} waiting` : ''].filter(Boolean).join(' · '));
    sum.dataset.state = overall;

    const shown = items.length > 0 && !pref.closed;
    button.hidden = !items.length;
    button.dataset.state = overall;
    button.setAttribute('aria-pressed', String(shown));
    set(buttonText, `Agents ${items.length}`);
    button.title = `${items.length} agent${items.length === 1 ? '' : 's'} on the canvas${working ? ` · ${working} at work` : ''}${waiting ? ` · ${waiting} waiting` : ''}: ${shown ? 'hide' : 'show'} their window`;
    element.dataset.folded = pref.folded ? 'true' : '';
    fold.textContent = pref.folded ? '▸' : '▾';
    fold.title = pref.folded ? 'Unfold it' : 'Fold it to its header';
    fold.setAttribute('aria-label', fold.title);
    fold.setAttribute('aria-expanded', String(!pref.folded));

    const was = !element.hidden;
    element.hidden = !shown;
    if (!shown) {
      placed = false;
      return;
    }
    if (!was || !placed) {
      if (element.offsetParent) {
        resizeObserver.disconnect();
        resizeObserver.observe(element.offsetParent);
        place(pref.x ?? MARGIN + 4, pref.y ?? MARGIN + 4);
        placed = true;
      }
    }
  }

  // Those at work count their seconds.
  const ticker = setInterval(() => {
    if (element.hidden || pref.folded) return;
    for (const row of rows.values()) if (atWork(row.state)) clock(row);
  }, 1000);

  return {
    element,
    button,
    draw,
    toggle,
    count: () => items.length,
    shown: () => !element.hidden,
    dispose() {
      clearInterval(ticker);
      resizeObserver.disconnect();
    },
  };
}
