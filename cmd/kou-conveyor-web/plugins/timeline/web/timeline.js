// timeline: the transcript of the session in view, in the layout's
// stage.main. It draws the session's entries as they come and change, and
// takes contributions that change how they look:
//
//   timeline.renderer    { kind, render(entry, ctx) → node | null }: draws a
//                        kind of entry (user, assistant, reasoning, tool,
//                        notice, error, or a kind of a plugin's); the latest
//                        that returns a node wins, null leaves it to the next
//   timeline.action      { id, kinds, label, title, order, shown(entry, ctx), run(entry, ctx) }:
//                        a button in an entry's header (Edit, Copy, Reuse…)
//   timeline.decoration  { kinds, order, render(entry, ctx) → node | null }: drawn
//                        under an entry (a prompt's images)
//   timeline.editor      { editor(entry, ctx) → node | null }: a prompt being
//                        edited shows this node in place of its text
//   tool.view            { tool, summary(entry), render(entry, ui) }: how a
//                        tool's calls read (cockpit.tools.register)
//
// A session can run to thousands of entries and megabytes of tool output,
// more than a page lays out at the pace of scrolling, so only the entries
// in view and a screen or so around it are drawn. Between them a gap
// (li.gap) stands in for the others, as tall as they were when they were
// drawn, or as they are guessed to be from what they say; they are drawn
// as they come into view, and what is in view stays where it is as their
// heights come true. node(id) is only there for an entry drawn; reveal(id)
// draws it. Below a prompt being edited, the entries drawn are marked
// data-after-edit.
//
// It provides the timeline service: schedule(id), flushNow(), reveal(id),
// scrollToBottom(), nearBottom(), promptInView(v), node(id), forget(ids),
// setAllTools(open), scroller().
import { fmt, h } from '/kernel/dom.js';

const TOOL_GLYPH = { queued: '□', running: '■', done: '✓', failed: '✕', canceled: '⊘', interrupted: '◌' };
const TOOL_WORD = { queued: 'Queued', running: 'Running', done: 'Done', failed: 'Failed', canceled: 'Stopped', interrupted: 'Interrupted' };

const STARTERS = [
  ['Survey', 'Map the architecture, entry points and how to build and test.', 'Map this repository: its architecture, entry points, and how to build and test it.'],
  ['Verify', 'Run the tests and fix the first real failure.', 'Run the test suite. If anything fails, find the root cause and fix it.'],
  ['Review', 'Audit uncommitted changes for bugs and risky edits.', 'Review the uncommitted changes for bugs, races and risky edits. Report findings by severity.'],
  ['Profile', 'Find the slowest step and propose a concrete fix.', 'Find the slowest part of the build or test run and propose a concrete fix.'],
];

// toolState maps a tool to what is shown: a tool that never finished in a
// session nobody is running was interrupted.
export function toolState(entry, live) {
  const state = entry.tool?.state || 'queued';
  if (!live && (state === 'queued' || state === 'running')) return 'interrupted';
  return state;
}

export default function activate(cockpit) {
  const session = cockpit.use('session');
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const view = () => session.view?.();

  // ---------------------------------------------------------------- the log

  const empty = h('section', { class: 'empty', id: 'empty', hidden: true },
    h('div', { class: 'empty-card ticks' },
      h('div', { class: 'empty-head' }, h('span', { class: 'label', text: 'Session · New' }), h('span', { class: 'label', id: 'empty-path' })),
      h('h1', null, 'Ready', h('span', { class: 'caret', 'aria-hidden': 'true' })),
      h('p', { text: 'The agent works in this workspace with a shell. Describe the outcome you want — it plans, runs commands and reports back as it goes.' }),
      h('ol', { class: 'starters' }, STARTERS.map(([title, text, prompt], n) => h('li', null, h('button', {
        type: 'button', data: { starter: prompt },
        onclick: () => service('composer')?.set?.(prompt, { focus: true, end: true }),
      }, h('span', { class: 'n', text: String(n + 1).padStart(2, '0') }), h('b', { text: title }), h('span', { text }))))),
      h('div', { class: 'empty-keys' },
        h('span', null, h('kbd', { text: '⌘K' }), ' commands'), h('span', null, h('kbd', { text: '/' }), ' sessions'), h('span', null, h('kbd', { text: '?' }), ' shortcuts'))));
  const emptyPath = empty.querySelector('#empty-path');
  const loading = h('div', { class: 'state-line', id: 'loading', hidden: true },
    h('span', { class: 'meter', 'aria-hidden': 'true' }, Array.from({ length: 8 }, () => h('i'))), 'Loading session');
  const errorText = h('span', { id: 'load-error-text' });
  const loadError = h('div', { class: 'load-error', id: 'load-error', hidden: true },
    h('b', { text: 'Could not load this session' }), errorText,
    h('button', { class: 'act', id: 'load-retry', type: 'button', onclick: () => session.openSession(view().id, { push: false }) }, 'Retry'));
  const list = h('ol', { class: 'entries', id: 'entries', 'aria-live': 'polite' });
  const log = h('div', { class: 'log' }, empty, loading, loadError, list);
  cockpit.ui.mount('stage.main', { id: 'timeline', order: 0, node: log });

  const jumpCount = h('span', { id: 'jump-count', text: '0' });
  const jump = h('button', { class: 'jump', id: 'jump', type: 'button', hidden: true, onclick: () => scrollToBottom() }, jumpCount, ' new ↓');
  cockpit.ui.mount('dock.float', { id: 'jump', order: 0, node: jump });

  const scroller = () => service('layout')?.scroller?.() || log.parentElement;

  // Drawn are the entries in view and those within REACH screens of it;
  // those drawn that end up further than KEEP screens away are let go. A
  // frame draws what is in view whatever that costs, and what is beyond it
  // within BUDGET milliseconds; the rest waits for the next.
  const REACH = 1;
  const KEEP = 3;
  const BUDGET = 8;
  const nodes = new Map(); // id → its item, for the entries drawn
  const owner = new WeakMap(); // item → the id of its entry
  const shape = new WeakMap(); // item → whether it was drawn open
  const sized = new WeakMap(); // item, gap or scroller → its height, as last seen
  const heights = new Map(); // id → { open, closed }: its heights, as measured
  const guesses = new WeakMap(); // entry → { columns, true, false }: its heights, as guessed
  const gaps = []; // the gaps, in their order in the list
  // The session's order, where each entry is in it, and where each starts
  // in the list: pos[i] is the top of the ith, pos[n] the list's height.
  // Those from the entry at from on are to be worked out again.
  const model = { order: null, length: 0, last: undefined, index: new Map(), pos: new Float64Array(1024), from: 0 };
  let heightsOf = ''; // the session the heights are of
  let columns = 80; // about how many characters a line of an entry holds
  let watched = null; // the scroller whose size is watched
  const pending = { ids: new Set(), all: false, fresh: false, frame: 0 };
  let unseen = 0;
  let held = null; // the entry that holds the view in place: { id, offset }
  let following = true; // whether the view keeps to the end
  let lastTop = 0;

  // An item drawn, or the scroller, that changes its height moves what is
  // in view: the view keeps to the end, or its entry where it was. Only the
  // scrolling happens here; drawing waits for the frame.
  const sizes = new ResizeObserver((records) => {
    let moved = false;
    for (const record of records) {
      const height = record.borderBoxSize?.[0]?.blockSize ?? record.target.getBoundingClientRect().height;
      if (Math.abs((sized.get(record.target) ?? -1) - height) < 0.5) continue;
      sized.set(record.target, height);
      const id = owner.get(record.target);
      if (id !== undefined) resized(id);
      moved = true;
    }
    const el = scroller();
    if (!moved || !usable(el)) return;
    if (following) el.scrollTop = el.scrollHeight;
    else restore(el, held);
    request();
  });
  cockpit.onDispose(() => {
    cancelAnimationFrame(pending.frame);
    sizes.disconnect();
  });

  function request() {
    if (!pending.frame) pending.frame = requestAnimationFrame(frame);
  }

  function schedule(id) {
    if (id === undefined) pending.all = true;
    else pending.ids.add(id);
    request();
  }

  // frame draws what changed, or else what came into view.
  function frame() {
    pending.frame = 0;
    const v = view();
    if (!v) return;
    const el = scroller();
    if (el && el !== watched) {
      if (watched) sizes.unobserve(watched);
      sizes.observe(el);
      watched = el;
    }
    if (pending.all || pending.ids.size) flush(v, el);
    else place(v, el);
  }

  // flushNow draws what is pending right away, for code that needs the DOM.
  function flushNow() {
    if (pending.frame) cancelAnimationFrame(pending.frame);
    frame();
  }

  function usable(el) {
    return !!el && el.clientHeight > 0 && el.contains(list);
  }

  function atEnd(el) {
    return el.scrollHeight - el.scrollTop - el.clientHeight < 2;
  }

  function nearBottom() {
    const el = scroller();
    return !el || el.scrollHeight - el.scrollTop - el.clientHeight < 96;
  }

  // scrollToBottom goes to the end, and draws what is there: as the entries
  // there come out taller or shorter than they were guessed, the end moves.
  function scrollToBottom() {
    unseen = 0;
    jump.hidden = true;
    following = true;
    const el = scroller();
    const v = view();
    if (!el) return;
    const deadline = performance.now() + BUDGET;
    for (let i = 0; i < 8; i++) {
      el.scrollTop = el.scrollHeight;
      if (!v || !usable(el) || !fill(el, v, deadline)) break;
    }
    held = anchorAt(el, v);
  }

  function flush(v, el) {
    const live = usable(el);
    // A session just opened shows its end.
    const stick = !live || pending.fresh || nearBottom();
    const at = stick ? null : anchorAt(el, v);
    const ctx = context(v);
    // Drawing an entry again moves what has the focus in it (an editor),
    // which loses the focus; it gets it back.
    const active = list.contains(document.activeElement) ? document.activeElement : null;
    const selection = active && 'selectionStart' in active ? [active.selectionStart, active.selectionEnd, active.selectionDirection] : null;
    if (heightsOf !== `${v.ws}/${v.id}`) {
      // Another session: nothing of the one before is of use.
      for (const [id, node] of nodes) letGo(id, node);
      heights.clear();
      heightsOf = `${v.ws}/${v.id}`;
      model.order = null;
    }
    if (live) {
      const fits = Math.max(20, Math.floor((list.clientWidth - 100) / 7.3));
      if (fits !== columns) {
        columns = fits;
        model.from = 0;
      }
    }
    let added = 0;
    for (const id of pending.ids) if (!model.index.has(id) && v.entries.has(id)) added++;
    // What changed is drawn again, if it is drawn; how all entries look
    // changed, every one drawn is.
    const keep = keptIds(v);
    const fresh = [];
    for (const id of pending.all ? new Set([...nodes.keys(), ...keep]) : pending.ids) {
      const entry = v.entries.get(id);
      if (!entry) continue;
      if (nodes.has(id) || keep.has(id)) fresh.push([id, redraw(id, entry, ctx)]);
      resized(id);
    }
    if (pending.all) model.from = 0;
    pending.all = false;
    pending.fresh = false;
    pending.ids.clear();
    commit(v);
    for (const [id, node] of fresh) remember(id, node);
    if (active && active.isConnected && document.activeElement !== active) {
      active.focus({ preventScroll: true });
      if (selection) active.setSelectionRange(...selection);
    }
    renderState(v);
    cockpit.render();
    if (live && stick) scrollToBottom();
    else if (live) {
      restore(el, at);
      settle(el, v, at, performance.now() + BUDGET);
      held = anchorAt(el, v);
      if (added) {
        unseen += added;
        jumpCount.textContent = String(unseen);
        jump.hidden = false;
      }
    }
    cockpit.emit('timeline:flush', v);
  }

  // place draws what came into view as the view moved, or as what is in it
  // changed its size. Keeping to the end, the view goes back to it when the
  // end moved away, not when it is scrolled a little up from it.
  function place(v, el) {
    if (!usable(el)) return;
    if (following && !atEnd(el)) return scrollToBottom();
    const at = anchorAt(el, v);
    settle(el, v, at, performance.now() + BUDGET);
    held = anchorAt(el, v);
    if (following && !atEnd(el)) scrollToBottom();
  }

  // settle fills the view until what is drawn in it keeps its height,
  // holding the entry at where it was in the view.
  function settle(el, v, at, deadline) {
    for (let i = 0; i < 8 && fill(el, v, deadline); i++) restore(el, at);
  }

  // fill draws the entries in view, and those within reach of it as the
  // budget allows, and lets go of those drawn far from it. It says whether
  // what it drew came out of another height than it stood in at, which
  // moves what is in view. around is somewhere else to fill than the view:
  // { top } in the list.
  function fill(el, v, deadline, around = null) {
    sync(v);
    const screen = el.clientHeight;
    const top = around ? around.top : viewTop(el);
    const bottom = top + screen;
    // Far from the view, the entries drawn are let go, but for those with
    // the focus, the selection or an editor in them.
    const keep = keptIds(v);
    const selection = document.getSelection?.();
    const range = selection?.rangeCount && !selection.isCollapsed ? selection.getRangeAt(0) : null;
    const nearFrom = indexAt(top - KEEP * screen);
    const nearTo = indexAt(bottom + KEEP * screen);
    let changed = false;
    for (const [id, node] of nodes) {
      const i = model.index.get(id);
      if (i !== undefined && i >= nearFrom && i <= nearTo) continue;
      if (keep.has(id) || (range && node instanceof Node && range.intersectsNode(node))) continue;
      remember(id, node);
      letGo(id, node);
      changed = true;
    }
    // In view, every entry is drawn; within reach of it, as many as the
    // budget allows, the nearest first.
    const want = [];
    let inView = 0;
    if (model.length) {
      const first = indexAt(top);
      const last = indexAt(bottom);
      const from = indexAt(top - REACH * screen);
      const to = indexAt(bottom + REACH * screen);
      for (let i = first; i <= last; i++) if (!nodes.has(model.order[i])) want.push(i);
      inView = want.length;
      for (let d = 1; last + d <= to || first - d >= from; d++) {
        if (last + d <= to && !nodes.has(model.order[last + d])) want.push(last + d);
        if (first - d >= from && !nodes.has(model.order[first - d])) want.push(first - d);
      }
    }
    const fresh = [];
    if (want.length) {
      const ctx = context(v);
      for (let k = 0; k < want.length; k++) {
        if (k >= inView && performance.now() > deadline) {
          request();
          break;
        }
        const id = model.order[want[k]];
        const entry = v.entries.get(id);
        if (!entry) continue;
        const before = heightAt(v, id);
        fresh.push([id, drawItem(entry, ctx), before]);
      }
    }
    if (!changed && !fresh.length) return false;
    commit(v);
    let moved = false;
    for (const [id, node, before] of fresh) if (Math.abs(remember(id, node) - before) >= 0.5) moved = true;
    return moved;
  }

  // commit lays the entries drawn out in the list in their order, with a
  // gap for each run of entries between them, as tall as those are.
  function commit(v) {
    sync(v);
    let gone = false;
    for (const [id, node] of nodes) {
      if (model.index.has(id) && v.entries.has(id)) continue;
      letGo(id, node);
      gone = true;
    }
    if (gone) sync(v);
    const { order, index, pos, length } = model;
    const at = [];
    for (const id of nodes.keys()) at.push(index.get(id));
    at.sort((a, b) => a - b);
    const edited = v.edit ? index.get(v.edit.id) ?? -1 : -1;
    const sequence = [];
    let used = 0;
    let next = 0;
    const gap = (from, to) => {
      if (to <= from) return;
      const node = gaps[used] || (gaps[used] = h('li', { class: 'gap', 'aria-hidden': 'true' }));
      used++;
      const height = pos[to] - pos[from];
      if (sized.get(node) !== height) {
        node.style.height = `${height}px`;
        sized.set(node, height);
      }
      sequence.push(node);
    };
    for (const i of at) {
      gap(next, i);
      const node = nodes.get(order[i]);
      const after = edited >= 0 && i > edited;
      if (node instanceof Element && node.hasAttribute('data-after-edit') !== after) node.toggleAttribute('data-after-edit', after);
      sequence.push(node);
      next = i + 1;
    }
    gap(next, length);
    // Each goes where it belongs, and what is not wanted any more goes.
    const wanted = new Set(sequence);
    let cursor = list.firstChild;
    for (const node of sequence) {
      while (cursor && cursor !== node && !wanted.has(cursor)) {
        const after = cursor.nextSibling;
        cursor.remove();
        cursor = after;
      }
      if (cursor === node) cursor = cursor.nextSibling;
      else list.insertBefore(node, cursor);
    }
    while (cursor) {
      const after = cursor.nextSibling;
      cursor.remove();
      cursor = after;
    }
  }

  // sync has the model follow the session's order, and work out where the
  // entries start from the first whose height changed on.
  function sync(v) {
    const order = v.order;
    const n = order.length;
    if (model.order !== order || n < model.length || (model.length && order[model.length - 1] !== model.last)) {
      model.order = order;
      model.index = new Map();
      for (let i = 0; i < n; i++) model.index.set(order[i], i);
      model.from = 0;
    } else if (n > model.length) {
      for (let i = model.length; i < n; i++) model.index.set(order[i], i);
      model.from = Math.min(model.from, model.length);
    }
    model.length = n;
    model.last = order[n - 1];
    if (model.pos.length < n + 1) {
      const pos = new Float64Array(Math.max(n + 1, model.pos.length * 2));
      pos.set(model.pos);
      model.pos = pos;
    }
    const pos = model.pos;
    for (let i = model.from; i < n; i++) pos[i + 1] = pos[i] + heightAt(v, order[i]);
    model.from = n;
  }

  // resized has the model work out again where the entries start, from
  // the one with id on.
  function resized(id) {
    const i = model.index.get(id);
    if (i !== undefined && i < model.from) model.from = i;
  }

  // indexAt is the entry at y in the list: the first that ends below it, or
  // the last.
  function indexAt(y) {
    const pos = model.pos;
    let lo = 0;
    let hi = Math.max(0, model.length - 1);
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (pos[mid + 1] > y) hi = mid;
      else lo = mid + 1;
    }
    return lo;
  }

  // viewTop is where the top of the view is in the list.
  function viewTop(el) {
    return el.getBoundingClientRect().top - list.getBoundingClientRect().top;
  }

  // drawItem draws an entry as an item of the list.
  function drawItem(entry, ctx) {
    const node = draw(entry, ctx);
    nodes.set(entry.id, node);
    owner.set(node, entry.id);
    shape.set(node, isOpen(ctx.view, entry));
    if (node instanceof Element) sizes.observe(node);
    return node;
  }

  // redraw draws an entry drawn again, in the place of its item.
  function redraw(id, entry, ctx) {
    const old = nodes.get(id);
    const node = drawItem(entry, ctx);
    if (old) {
      if (old instanceof Element) sizes.unobserve(old);
      if (old.parentNode === list) old.replaceWith(node);
    }
    return node;
  }

  // letGo has an entry drawn stand in a gap again, as tall as it was.
  function letGo(id, node) {
    if (node instanceof Element) sizes.unobserve(node);
    nodes.delete(id);
    resized(id);
  }

  // remember measures an item drawn, for the height its entry stands in a
  // gap at, open or closed.
  function remember(id, node) {
    if (!(node instanceof Element)) return 0;
    const height = node.getBoundingClientRect().height;
    // Out of the page, or in a view not shown, an item has no height.
    if (!height) return sized.get(node) ?? 0;
    if (sized.get(node) !== height) {
      sized.set(node, height);
      resized(id);
    }
    const known = heights.get(id) || {};
    known[shape.get(node) ? 'open' : 'closed'] = height;
    heights.set(id, known);
    return height;
  }

  // heightAt is how tall an entry is: as it is drawn, or else as it was,
  // or as it is guessed to be.
  function heightAt(v, id) {
    const node = nodes.get(id);
    if (node && sized.has(node)) return sized.get(node);
    const entry = v.entries.get(id);
    return entry ? heightOf(entry, v) : 0;
  }

  function heightOf(entry, v) {
    const open = isOpen(v, entry);
    const known = heights.get(entry.id)?.[open ? 'open' : 'closed'];
    if (known !== undefined) return known;
    let guess = guesses.get(entry);
    if (guess?.columns !== columns) guesses.set(entry, (guess = { columns }));
    return (guess[open] ??= estimate(entry, open));
  }

  // isOpen is whether an entry shows open, as ctx.expanded says.
  function isOpen(v, entry) {
    if (v.expanded.has(entry.id)) return !!v.expanded.get(entry.id);
    return entry.kind === 'tool' && entry.tool?.state === 'failed' && !!entry.tool?.error;
  }

  // estimate is about how tall an entry not drawn yet is, from what it says:
  // how many lines it takes, where it shows them.
  function estimate(entry, open) {
    switch (entry.kind) {
      case 'tool': {
        const tool = entry.tool || {};
        const calls = tool.calls?.length || 0;
        const peek = tool.state === 'failed' && tool.error ? 26 : 0;
        if (!open) return calls ? 61 + (calls > 12 ? 9 : calls) * 24 + peek : 54 + peek;
        // A stream shows 420 pixels of its text at most.
        const stream = (text) => 41 + Math.min(420, wrapped(text, 24) * 18.6);
        let height = 55;
        if (tool.input && (tool.input.includes('\n') || tool.input.length > 90)) height += stream(tool.input);
        if (calls) height += 38 + calls * 24;
        if (tool.error) height += 20 + wrapped(tool.error, 200) * 18.6;
        for (const text of [tool.output, tool.stderr, tool.logs, tool.value]) if (text) height += stream(text);
        if (tool.files?.length) height += 34;
        if (!tool.output && !tool.stderr && !tool.error && !calls && !tool.value && !tool.logs) height += 38;
        return height;
      }
      case 'user': return 71 + wrapped(entry.text) * 23.2;
      case 'assistant': return 49 + wrapped(entry.text) * 25;
      case 'reasoning': return 40 + (open ? 16 + wrapped(entry.text) * 22.4 : 0);
      case 'notice': return 40 + (open && entry.detail ? 16 + wrapped(entry.detail) * 22.4 : 0);
      case 'error': return 64 + wrapped(entry.text) * 19.4;
      default: return 48;
    }
  }

  // wrapped is how many lines a text takes in an entry, up to cap.
  function wrapped(text, cap = Infinity) {
    const s = typeof text === 'string' ? text : String(text ?? '');
    let lines = 0;
    for (let at = 0; at <= s.length && lines < cap;) {
      let end = s.indexOf('\n', at);
      if (end < 0) end = s.length;
      lines += Math.max(1, Math.ceil((end - at) / columns));
      at = end + 1;
    }
    return Math.min(lines, cap);
  }

  // keptIds are the entries drawn wherever they are: the prompt being
  // edited (v.edit, the edit plugin's), and those with the focus or an end
  // of the selection in them.
  function keptIds(v) {
    const keep = new Set();
    if (v.edit?.id) keep.add(v.edit.id);
    const add = (node) => {
      while (node && node.parentNode !== list) node = node.parentNode;
      const id = node ? owner.get(node) : undefined;
      if (id !== undefined) keep.add(id);
    };
    add(document.activeElement);
    const selection = document.getSelection?.();
    if (selection?.rangeCount && !selection.isCollapsed) {
      add(selection.anchorNode);
      add(selection.focusNode);
    }
    return keep;
  }

  // anchorAt is what holds the view in place: the first entry drawn in it
  // (one in a gap may come out taller or shorter than it stood in at), and
  // how far below the top of the view its top is.
  function anchorAt(el, v = view()) {
    if (!v || !usable(el)) return null;
    const box = el.getBoundingClientRect();
    for (const node of list.children) {
      const id = owner.get(node);
      if (id === undefined) continue;
      const rect = node.getBoundingClientRect();
      if (rect.top >= box.bottom) break;
      if (rect.bottom > box.top) return { id, offset: rect.top - box.top };
    }
    sync(v);
    if (!model.length) return null;
    const top = viewTop(el);
    const i = indexAt(top);
    return { id: model.order[i], offset: model.pos[i] - top };
  }

  // restore scrolls the entry at back to where it was in the view.
  function restore(el, at) {
    if (!at) return;
    const node = nodes.get(at.id);
    let now;
    if (node instanceof Element && node.isConnected) now = node.getBoundingClientRect().top - el.getBoundingClientRect().top;
    else {
      const v = view();
      if (!v) return;
      sync(v);
      const i = model.index.get(at.id);
      if (i === undefined) return;
      now = model.pos[i] - viewTop(el);
    }
    const delta = now - at.offset;
    if (Math.abs(delta) >= 0.5) el.scrollTop += delta;
  }

  function renderState(v) {
    empty.hidden = !(v.order.length === 0 && !v.loading && !v.failed);
    loading.hidden = !v.loading;
    loadError.hidden = !v.failed;
    errorText.textContent = v.failed;
    emptyPath.textContent = session.currentWorkspace?.()?.path || session.state?.config?.workspace || '';
  }

  // ---------------------------------------------------------------- drawing entries

  // context is what renderers, actions and decorations get to draw with.
  function context(v) {
    const models = service('models');
    const ctx = {
      h, fmt, view: v, summary: session.summary(v),
      live: !!v.run || v.external,
      index: (id) => v.users.get(id) || 0,
      expanded: (entry) => {
        if (v.expanded.has(entry.id)) return v.expanded.get(entry.id);
        return entry.kind === 'tool' && entry.tool?.state === 'failed' && !!entry.tool?.error;
      },
      toggle: (id, open) => {
        v.expanded.set(id, open);
        schedule(id);
      },
      copy: (text, label) => cockpit.copy(text, label),
      markdown: (text, options) => (cockpit.has('markdown') ? cockpit.use('markdown').render(text, options) : h('p', { text })),
      button,
      actions: (entry) => actionsFor(entry, ctx),
      stream: (label, text, kind = 'out') => stream(label, text, kind),
      // The prompt's model, and whether it differs from the prompt before.
      modelTitle: (id) => models?.title?.(id, 'The model that answered this prompt') || id,
      modelProvider: (id) => models?.provider?.(id) || '',
      switched: (entry) => {
        const before = session.previousModel(v, entry.id);
        return !!before && !!entry.model && before !== entry.model;
      },
      arrived: (id) => v.arrived.delete(id),
      editable: cockpit.has('edit') && !session.runBlocked(v),
    };
    return ctx;
  }

  function button(label, onclick, title) {
    return h('button', { class: 'act', type: 'button', title, onclick: (event) => { event.stopPropagation(); onclick(); } }, label);
  }

  function actionsFor(entry, ctx) {
    const buttons = cockpit.contributions('timeline.action', { unique: 'id' })
      .filter((a) => (!a.kinds || a.kinds.includes(entry.kind)) && (!a.shown || cockpit.safely(() => a.shown(entry, ctx))))
      .map((a) => button(typeof a.label === 'function' ? a.label(entry) : a.label, () => cockpit.safely(() => a.run(entry, ctx)), typeof a.title === 'function' ? a.title(entry) : a.title));
    return h('span', { class: 'actions' }, buttons);
  }

  function decorations(entry, ctx) {
    return cockpit.contributions('timeline.decoration')
      .filter((d) => !d.kinds || d.kinds.includes(entry.kind))
      .map((d) => cockpit.safely(() => d.render(entry, ctx)))
      .filter((node) => node instanceof Node);
  }

  // draw builds one entry: the latest renderer of its kind that answers
  // (by order: the built-in ones come last).
  function draw(entry, ctx) {
    const renderers = cockpit.contributions('timeline.renderer').filter((r) => r.kind === entry.kind || r.kind === '*').reverse();
    for (const renderer of renderers) {
      const node = cockpit.safely(() => renderer.render(entry, ctx));
      if (node instanceof Node) return node;
    }
    return h('li', { class: 'entry', data: { kind: entry.kind, id: entry.id } }, h('div', { class: 'gutter' }), h('div', { class: 'body' }, h('div', { class: 'text', text: entry.text || '' })));
  }

  // shell is an entry's frame: its item, gutter and body.
  function shell(entry) {
    const li = h('li', { class: 'entry', id: `entry-${entry.id}`, data: { kind: entry.kind, id: entry.id } });
    const gutter = h('div', { class: 'gutter' });
    const body = h('div', { class: 'body' });
    li.append(gutter, body);
    const time = entry.at ? h('time', { datetime: entry.at, title: fmt.stamp(entry.at), text: fmt.clock(entry.at) }) : null;
    return { li, gutter, body, time };
  }

  function activateKeys(fn) {
    return (event) => {
      if (event.target !== event.currentTarget) return;
      if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); fn(); }
    };
  }

  const builtin = {
    user(entry, ctx) {
      const { li, gutter, body, time } = shell(entry);
      const n = ctx.index(entry.id);
      gutter.append(h('span', { class: 'idx', text: String(n).padStart(2, '0') }));
      if (entry.state) li.dataset.state = entry.state;
      // A plugin's editor takes the prompt's place while it is edited.
      for (const editor of cockpit.contributions('timeline.editor')) {
        const node = cockpit.safely(() => editor.editor(entry, ctx));
        if (node instanceof Node) {
          li.dataset.editing = 'true';
          body.append(h('header', { class: 'meta' }, h('span', { class: 'who', text: 'You' }), time, h('span', { class: 'flag editing', text: 'Editing' })), node);
          return li;
        }
      }
      const flag = entry.state === 'pending' ? h('span', { class: 'flag pending', text: 'Sending' })
        : entry.state === 'undelivered' ? h('span', { class: 'flag undelivered', text: 'Not delivered' }) : null;
      // Sent while the agent worked, which read it after its tool calls.
      const forced = entry.forced && h('span', {
        class: 'flag forced', text: '⚡ Forced in',
        title: 'Sent while the agent worked: it read this after the results of the tool calls it was making',
      });
      if (entry.forced) li.dataset.forced = 'true';
      if (ctx.arrived(entry.id)) li.dataset.arrived = 'true';
      // The model that answered the prompt; a session may use another for
      // every prompt, and one that differs from the prompt before is marked.
      const model = entry.model && h('span', {
        class: 'model-tag', title: ctx.modelTitle(entry.model) || entry.model,
        data: { provider: ctx.modelProvider(entry.model) || null, switched: ctx.switched(entry) ? 'true' : null },
      }, h('i', { class: 'model-dot', 'aria-hidden': 'true' }), entry.model);
      body.append(
        h('header', { class: 'meta' }, h('span', { class: 'who', text: 'You' }), time, model, flag, forced, ctx.actions(entry)),
        h('div', { class: 'text', text: entry.text }),
        ...decorations(entry, ctx));
      return li;
    },
    assistant(entry, ctx) {
      const { li, gutter, body, time } = shell(entry);
      gutter.append(h('span', { class: 'tick', text: fmt.short(entry.at) }));
      if (entry.phase) li.dataset.phase = entry.phase;
      body.append(
        h('header', { class: 'meta' }, h('span', { class: 'who', text: 'Agent' }),
          entry.phase === 'commentary' && h('span', { class: 'tag', text: 'Note' }), time, ctx.actions(entry)),
        h('div', { class: 'prose' }, ctx.markdown(entry.text, { onCopy: (text) => ctx.copy(text, 'Code copied') })),
        ...decorations(entry, ctx));
      return li;
    },
    reasoning(entry, ctx) {
      const { li, gutter, body } = shell(entry);
      gutter.append(h('span', { class: 'tick', text: fmt.short(entry.at) }));
      const open = ctx.expanded(entry);
      const gist = entry.text.split('\n').find((line) => line.trim()) || '';
      const toggle = () => ctx.toggle(entry.id, !open);
      body.append(h('div', { class: 'fold', role: 'button', tabindex: 0, 'aria-expanded': String(open), onclick: toggle, onkeydown: activateKeys(toggle) },
        h('span', { class: 'who', text: 'Thinking' }),
        h('span', { class: 'gist', text: gist.replace(/\*\*/g, '') }),
        h('span', { class: 'chev', 'aria-hidden': 'true' })));
      if (open) body.append(h('div', { class: 'prose thought' }, ctx.markdown(entry.text)));
      return li;
    },
    tool(entry, ctx) {
      const { li, gutter, body } = shell(entry);
      gutter.append(h('span', { class: 'tick', text: fmt.short(entry.at) }));
      body.append(renderTool(entry, ctx), ...decorations(entry, ctx));
      return li;
    },
    notice(entry, ctx) {
      const { li, body, time } = shell(entry);
      const open = entry.detail && ctx.expanded(entry);
      const toggle = () => ctx.toggle(entry.id, !open);
      body.append(h('div', entry.detail
        ? { class: 'notice', role: 'button', tabindex: 0, 'aria-expanded': String(!!open), onclick: toggle, onkeydown: activateKeys(toggle) }
        : { class: 'notice' }, h('span', { text: entry.text }), time));
      if (open) body.append(h('div', { class: 'prose thought' }, ctx.markdown(entry.detail)));
      return li;
    },
    error(entry, ctx) {
      const { li, gutter, body, time } = shell(entry);
      gutter.append(h('span', { class: 'tick', text: fmt.short(entry.at) }));
      body.append(
        h('header', { class: 'meta' }, h('span', { class: 'who', text: 'Error' }), time, ctx.actions(entry)),
        h('div', { class: 'errbox', text: entry.text }));
      return li;
    },
  };
  for (const [kind, render] of Object.entries(builtin)) cockpit.contribute('timeline.renderer', { kind, render, builtin: true, order: -1000 });

  // toolView is how a tool's calls read: as a plugin says, if one does.
  function toolView(name) {
    const custom = cockpit.contributions('tool.view').filter((t) => t.tool === name).pop();
    if (!custom) return null;
    const copy = (entry) => entry && { ...entry, tool: entry.tool && { ...entry.tool } };
    return {
      summary: custom.summary ? (entry) => cockpit.safely(() => custom.summary(copy(entry))) : null,
      render: custom.render ? (entry, helpers) => cockpit.safely(() => custom.render(copy(entry), helpers)) : null,
    };
  }

  function renderTool(entry, ctx) {
    const tool = entry.tool || {};
    const state = toolState(entry, ctx.live);
    // A plugin may say how its tool's calls read: a line, and the open call.
    const custom = toolView(tool.name);
    const summary = custom?.summary?.(entry);
    const open = ctx.expanded(entry);
    const shellTool = (tool.name || '').toLowerCase() === 'bash';
    const codeTool = (tool.name || '').toLowerCase() === 'code';
    const failed = state === 'failed';
    const exit = tool.exit_code;

    const meta = h('span', { class: 'tmeta' });
    if (state === 'running' && tool.started) {
      meta.append(h('span', { class: 'live', data: { since: tool.started }, text: fmt.duration(Date.now() - Date.parse(tool.started)) }));
    } else if (tool.started && tool.finished) {
      meta.append(h('span', { text: fmt.duration(Date.parse(tool.finished) - Date.parse(tool.started)) }));
    }
    if (exit != null && exit !== 0) meta.append(h('span', { class: 'exit', text: `Exit ${exit}` }));
    if (state !== 'done' && state !== 'running') meta.append(h('span', { class: `word ${state}`, text: TOOL_WORD[state] }));

    const calls = tool.calls?.length ? tool.calls : null;
    const toggle = () => ctx.toggle(entry.id, !open);
    // Closed, the calls a Code call's code made hang from its state glyph.
    const card = h('div', { class: 'tool', data: { state, exit: exit != null && exit !== 0 ? 'nonzero' : null, tree: calls && !open ? 'hung' : null } },
      h('div', { class: 'tool-head', role: 'button', tabindex: 0, 'aria-expanded': String(open), onclick: toggle, onkeydown: activateKeys(toggle) },
        h('span', { class: 'tstate', 'aria-label': TOOL_WORD[state], text: TOOL_GLYPH[state] }),
        h('span', { class: 'tname', text: tool.name || 'Tool' }),
        h('span', { class: `tinput${shellTool ? ' cmd' : ''}`, title: tool.input || '', text: typeof summary === 'string' ? summary : (tool.input || '').split('\n')[0] }),
        meta,
        h('span', { class: 'actions' }, tool.input && button('Copy', () => ctx.copy(tool.input, shellTool ? 'Command copied' : 'Copied'), 'Copy the command')),
        h('span', { class: 'chev', 'aria-hidden': 'true' })));

    const rendered = open && custom?.render ? custom.render(entry, { h, fmt, stream: (label, text) => stream(label, text, 'out'), copy: ctx.copy }) : null;
    if (rendered instanceof Node) {
      card.append(h('div', { class: 'tool-body plugin-view' }, rendered));
      if (calls) card.append(h('div', { class: 'tool-tree' }, renderCalls(entry, tool, ctx, state, true)));
    } else if (open) {
      const panel = h('div', { class: 'tool-body' });
      if (tool.input && (tool.input.includes('\n') || tool.input.length > 90)) panel.append(stream(codeTool ? 'Code' : 'Input', tool.input, 'input', tool.syntax));
      // The calls the code made come after the code and before what it
      // reported, as the model reads them.
      if (calls) {
        panel.append(h('section', { class: 'stream calls-stream' },
          h('header', null, h('span', { text: `Calls · ${calls.length}` }), callsSummary(calls, state)),
          renderCalls(entry, tool, ctx, state, true)));
      }
      if (tool.error) panel.append(h('div', { class: 'tool-error', text: tool.error }));
      if (tool.output) panel.append(stream('Output', tool.output, 'out'));
      if (tool.stderr) panel.append(stream('Stderr', tool.stderr, 'err'));
      if (tool.logs) panel.append(stream('Console', tool.logs, 'out'));
      if (tool.value) panel.append(stream('Return value', tool.value, 'out'));
      if (tool.files?.length) panel.append(h('div', { class: 'tool-files' }, tool.files.map((file) => h('code', { text: file }))));
      if (!tool.output && !tool.stderr && !tool.error && !calls && !tool.value && !tool.logs) {
        panel.append(h('div', { class: 'tool-empty', text: state === 'running' || state === 'queued' ? 'Waiting for output…' : 'No output' }));
      }
      card.append(panel);
    } else if (calls) {
      // Closed, the calls are the tree the run is, under why it failed.
      card.append(h('div', { class: 'tool-tree' },
        failed && tool.error && h('div', { class: 'tool-peek', text: tool.error.split('\n')[0] }),
        renderCalls(entry, tool, ctx, state, false)));
    } else if (failed && tool.error) {
      card.append(h('div', { class: 'tool-peek', text: tool.error.split('\n')[0] }));
    }
    return card;
  }

  // renderCalls draws the tool calls a Code call's code made as the tree
  // they are, a row each: its state, name and argument, when it ran within
  // the run and how long it took.
  //
  //   ✓ CODE  const [a, b] = await Promise.all([…
  //   ├─ ✓ bash      $ go test ./...          ━━━━━━━━───   4.2s
  //   ├─ ✓ read      main.go                  ━──────────   0.0s
  //   └─ ✕ edit      main.go                  ───────━───   0.0s
  //                  edit: oldString was not found in main.go
  //
  // The lanes put the calls the code made at once side by side, and those it
  // made one after another in steps. A click on a call shows what it was
  // given, where its row cannot, and what it gave; a failed call shows the
  // first line of its error, and opens with the card. Closed, a long tree
  // shows its first and last calls and how many are between.
  const FOLD = { over: 12, ends: 4 };

  function renderCalls(entry, tool, ctx, toolState, open) {
    const calls = tool.calls;
    const now = Date.now();
    const span = callSpan(calls, toolState, now);
    const all = open || calls.length <= FOLD.over || ctx.view.expanded.get(`${entry.id}:calls`) === true;
    const shown = (i) => all || i < FOLD.ends || i >= calls.length - FOLD.ends;
    const list = h('ol', { class: 'calls', 'aria-label': 'Tool calls the code made', data: { lanes: span ? 'true' : null } });
    const names = Math.min(16, Math.max(4, ...calls.filter((_, i) => shown(i)).map((call) => (call.name || '').length)));
    list.style.setProperty('--name', `${names}ch`);
    calls.forEach((call, index) => {
      if (shown(index)) list.append(renderCall(entry, call, index, ctx, toolState, open, span, now));
      else if (index === FOLD.ends) list.append(foldedCalls(entry, calls.slice(FOLD.ends, calls.length - FOLD.ends)));
    });
    return list;
  }

  function renderCall(entry, call, index, ctx, toolState, open, span, now) {
    const id = `${entry.id}:${index}`;
    const state = callState(call, toolState);
    const exit = call.exit_code;
    const nonzero = exit != null && exit !== 0;
    const input = call.input || '';
    const gist = call.gist || input.split('\n').find((line) => line.trim())?.trim() || '';
    const more = input.trim().includes('\n');
    // A call opens with the card when it failed; a click decides for good.
    const stored = ctx.view.expanded.get(id);
    const shown = stored ?? (open && (state === 'failed' || nonzero));
    const toggle = () => {
      ctx.view.expanded.set(id, !shown);
      schedule(entry.id);
    };
    const item = h('li', { class: 'call', data: { state, exit: nonzero ? 'nonzero' : null, open: String(shown) } },
      h('div', { class: 'call-head', role: 'button', tabindex: 0, 'aria-expanded': String(shown), onclick: toggle, onkeydown: activateKeys(toggle) },
        h('span', { class: 'tstate', 'aria-label': TOOL_WORD[state] || state, text: TOOL_GLYPH[state] || TOOL_GLYPH.done }),
        h('span', { class: 'cname', title: call.name, text: call.name }),
        h('span', { class: `cinput${call.name === 'bash' ? ' cmd' : ''}`, title: input }, gist, more && h('span', { class: 'cmore', text: ' …' })),
        nonzero && h('span', { class: 'cexit', text: `Exit ${exit}` }),
        span && lane(call, state, span, now),
        callTime(call, state)));
    if (shown) {
      const body = h('div', { class: 'call-body' });
      // What the call was given shows whole where its row cannot show it.
      if (more || (input.length > 72 && gist === input.trim())) body.append(h('pre', { class: 'call-in', text: input }));
      const text = call.error || call.output;
      if (text) {
        body.append(h('div', { class: `call-out${call.error ? ' err' : ''}` },
          h('pre', { text }),
          button('Copy', () => ctx.copy(text, call.error ? 'Error copied' : 'Output copied'), call.error ? 'Copy the error' : 'Copy the output')));
      } else {
        body.append(h('div', { class: 'call-none', text: state === 'running' ? 'Running…' : 'No output' }));
      }
      item.append(body);
    } else if (state === 'failed' && call.error) {
      item.append(h('div', { class: 'call-peek', text: call.error.split('\n')[0] }));
    }
    return item;
  }

  // foldedCalls stands for the calls a closed tree leaves out; a click shows
  // them.
  function foldedCalls(entry, hidden) {
    const failed = hidden.filter((call) => call.state === 'failed').length;
    const expand = () => {
      view().expanded.set(`${entry.id}:calls`, true);
      schedule(entry.id);
    };
    return h('li', { class: 'call folded' },
      h('div', { class: 'call-head', role: 'button', tabindex: 0, title: 'Show every call', onclick: expand, onkeydown: activateKeys(expand) },
        h('span', { class: 'tstate', text: '⋯' }),
        h('span', { class: 'cfold', text: `${hidden.length} more calls` }),
        failed && h('span', { class: 'cfold-failed', text: `${failed} failed` })));
  }

  // callState is the state a call shows: one still running when its run was
  // interrupted was interrupted with it.
  function callState(call, toolState) {
    const state = call.state || 'done';
    return state === 'running' && toolState === 'interrupted' ? 'interrupted' : state;
  }

  // callEnd is when a call ended: now, for one that runs.
  function callEnd(call, state, now) {
    if (call.finished) return Date.parse(call.finished);
    return state === 'running' ? now : Date.parse(call.started);
  }

  // callSpan is when the calls of a run ran, from the first start to the last
  // end: the scale their lanes share. A single call has no lane.
  function callSpan(calls, toolState, now) {
    let from = Infinity;
    let to = -Infinity;
    for (const call of calls) {
      const start = Date.parse(call.started);
      if (!Number.isFinite(start)) continue;
      from = Math.min(from, start);
      to = Math.max(to, callEnd(call, callState(call, toolState), now) || start);
    }
    return calls.length > 1 && to > from ? { from, length: to - from } : null;
  }

  // lane shows where in the span of the run a call ran.
  function lane(call, state, span, now) {
    const node = h('span', { class: 'lane', 'aria-hidden': 'true' });
    const start = Date.parse(call.started);
    if (!Number.isFinite(start)) return node;
    const end = Math.max(start, callEnd(call, state, now) || start);
    const bar = h('i');
    bar.style.left = `${(((start - span.from) / span.length) * 100).toFixed(2)}%`;
    bar.style.width = `${(((end - start) / span.length) * 100).toFixed(2)}%`;
    node.title = `Started ${fmt.duration(start - span.from)} into the run`;
    node.append(bar);
    return node;
  }

  // callTime is how long a call took, or has been running.
  function callTime(call, state) {
    const started = Date.parse(call.started);
    if (state === 'running' && Number.isFinite(started)) {
      return h('span', { class: 'ctime live', data: { since: call.started }, text: fmt.duration(Date.now() - started) });
    }
    const finished = Date.parse(call.finished);
    return h('span', { class: 'ctime', text: Number.isFinite(started) && Number.isFinite(finished) ? fmt.duration(finished - started) : '' });
  }

  // callsSummary says, at the head of the open tree, what did not go well.
  function callsSummary(calls, toolState) {
    const count = (state) => calls.filter((call) => callState(call, toolState) === state).length;
    const parts = [
      [count('running'), 'running', 'live'],
      [count('failed'), 'failed', 'failed'],
      [calls.filter((call) => call.exit_code != null && call.exit_code !== 0).length, 'nonzero exit', 'exit'],
    ].filter(([n]) => n > 0);
    if (!parts.length) return null;
    return h('span', { class: 'calls-sum' }, parts.map(([n, word, cls]) => h('span', { class: cls, text: `${n} ${word}` })));
  }

  // stream is a section of an open call: a label, and text, in the colours
  // of its tokens where syntax gives them.
  function stream(label, text, kind, syntax) {
    const count = fmt.lines(text);
    const classes = session.state?.config?.syntax_classes;
    return h('section', { class: `stream ${kind}` },
      h('header', null,
        h('span', { text: `${label} · ${count} ${count === 1 ? 'line' : 'lines'}` }),
        button('Copy', () => cockpit.copy(text, `${label} copied`))),
      syntax?.length && classes ? h('pre', { class: 'syntax' }, highlighted(text, syntax, classes)) : h('pre', { text }));
  }

  // highlighted is code as the pieces of its tokens: syntax is runs of a
  // length, in UTF-16 code units as a string counts them, and a class, by
  // its index in the classes the server names (cmd/internal/highlight).
  // Past the last run the code is plain.
  function highlighted(text, syntax, classes) {
    const pieces = [];
    let at = 0;
    for (let i = 0; i + 1 < syntax.length && at < text.length; i += 2) {
      const piece = text.slice(at, at + syntax[i]);
      const cls = classes[syntax[i + 1]];
      pieces.push(cls ? h('span', { class: `tk-${cls}`, text: piece }) : piece);
      at += syntax[i];
    }
    if (at < text.length) pieces.push(text.slice(at));
    return pieces;
  }

  // The actions of the timeline's own: copying what an entry says.
  cockpit.contribute('timeline.action', { id: 'copy', kinds: ['user'], label: 'Copy', order: 20, run: (entry) => cockpit.copy(entry.text, 'Prompt copied') });
  cockpit.contribute('timeline.action', { id: 'copy-answer', kinds: ['assistant'], label: 'Copy', order: 20, run: (entry) => cockpit.copy(entry.text, 'Answer copied') });
  cockpit.contribute('timeline.action', { id: 'copy-error', kinds: ['error'], label: 'Copy', order: 20, run: (entry) => cockpit.copy(entry.text, 'Error copied') });

  // ---------------------------------------------------------------- following the session

  cockpit.on('session:view', (v, { reload } = {}) => {
    if (!reload) {
      unseen = 0;
      jump.hidden = true;
      pending.fresh = true;
    }
    schedule();
  });
  cockpit.on('session:dirty', (v, id) => { if (v === view()) schedule(id); });
  // A plugin that changes how entries look has them drawn again.
  for (const point of ['timeline.renderer', 'timeline.action', 'timeline.decoration', 'timeline.editor', 'tool.view']) {
    cockpit.on(`point:${point}`, () => schedule());
  }
  cockpit.on('service', (name) => { if (name === 'markdown' || name === 'models' || name === 'edit') schedule(); });

  // Scrolling does not bubble; the capture sees it, whichever element the
  // layout's scroller is now. The view keeps to the end once it gets there,
  // until it is scrolled up; what comes into view is drawn in the frame.
  cockpit.listen(document, 'scroll', (event) => {
    const el = scroller();
    if (event.target !== el) return;
    const top = el.scrollTop;
    if (atEnd(el)) following = true;
    else if (top < lastTop) following = false;
    lastTop = top;
    request();
    if (unseen && nearBottom()) scrollToBottom();
    cockpit.emit('timeline:scroll');
  }, { capture: true, passive: true });

  // One clock drives every elapsed-time readout.
  cockpit.interval(() => {
    for (const el of list.querySelectorAll('[data-since]')) el.textContent = fmt.duration(Date.now() - Date.parse(el.dataset.since));
  }, 1000);

  // reveal draws an entry, brings it to the middle of the view and has it
  // flash: smoothly from near, at once from further.
  function reveal(id) {
    const v = view();
    const el = scroller();
    if (!v?.entries.has(id)) return;
    sync(v);
    if (!model.index.has(id)) return;
    const live = usable(el);
    const at = live ? anchorAt(el, v) : null;
    let node = nodes.get(id);
    if (!node) {
      node = drawItem(v.entries.get(id), context(v));
      commit(v);
      remember(id, node);
      if (live) restore(el, at);
    }
    if (!(node instanceof Element)) return;
    if (live) {
      // How far the view moves to have the entry in its middle.
      const offset = () => {
        const box = el.getBoundingClientRect();
        const rect = node.getBoundingClientRect();
        return rect.top + rect.height / 2 - (box.top + box.height / 2);
      };
      if (Math.abs(offset()) <= el.clientHeight) {
        // What the view passes and comes to is drawn before it moves: the
        // page's smooth scroll stops if the view is moved under it.
        settle(el, v, at, Infinity);
        for (let i = 0; i < 8 && fill(el, v, Infinity, { top: viewTop(el) + offset() }); i++) restore(el, at);
        if (offset() < -1) following = false;
        node.scrollIntoView({ block: 'center', behavior: 'smooth' });
      } else {
        node.scrollIntoView({ block: 'center' });
        const there = { id, offset: node.getBoundingClientRect().top - el.getBoundingClientRect().top };
        settle(el, v, there, performance.now() + BUDGET);
        held = anchorAt(el, v);
        following = atEnd(el);
      }
    }
    node.classList.remove('flash');
    void node.offsetWidth;
    node.classList.add('flash');
  }

  function setAllTools(open) {
    const v = view();
    for (const entry of v.entries.values()) {
      if (entry.kind === 'tool' || entry.kind === 'reasoning') v.expanded.set(entry.id, open);
    }
    schedule();
  }

  // promptInView is the prompt whose part of the transcript is in view: at
  // the bottom, the latest; above it, the last one that starts above a third
  // of the way down.
  function promptInView(v) {
    const prompts = v.order.filter((id) => v.entries.get(id)?.kind === 'user');
    if (!prompts.length) return null;
    const el = scroller();
    if (nearBottom() || v !== view() || !usable(el)) return v.entries.get(prompts[prompts.length - 1]);
    sync(v);
    const probe = viewTop(el) + el.clientHeight / 3;
    let found = prompts[0];
    for (const id of prompts) {
      if (model.pos[model.index.get(id)] > probe) break;
      found = id;
    }
    return v.entries.get(found);
  }

  // forget takes entries out that a rewind removed.
  function forget(ids) {
    for (const id of ids) {
      pending.ids.delete(id);
      const node = nodes.get(id);
      if (node) {
        letGo(id, node);
        node.remove();
      }
      heights.delete(id);
    }
  }

  cockpit.provide('timeline', {
    schedule, flushNow, reveal, scrollToBottom, nearBottom, promptInView, forget, setAllTools,
    node: (id) => nodes.get(id), scroller, toolState,
  });

  // ---------------------------------------------------------------- commands, keys, palette

  cockpit.commands.register({ name: 'expand', help: 'Expand tool output and thinking', order: 190, run: () => setAllTools(true) });
  cockpit.commands.register({ name: 'collapse', help: 'Collapse tool output and thinking', order: 200, run: () => setAllTools(false) });
  cockpit.keys.register({ key: 'e', views: ['sessions'], run: () => setAllTools(true) });
  cockpit.keys.register({ key: 'E', views: ['sessions'], run: () => setAllTools(false) });
  cockpit.keys.register({ key: 'g', views: ['sessions'], run: () => scrollToBottom() });
  cockpit.contribute('help.keys', { keys: ['E', ' ', '⇧', 'E'], text: 'Expand / collapse tool output', order: 170 });
  cockpit.contribute('help.keys', { keys: ['G'], text: 'Jump to latest', order: 180 });
  cockpit.palette.register({ group: 'Actions', icon: '↓', label: 'Jump to latest', hint: 'G', order: 110, run: scrollToBottom });
  cockpit.palette.register({ group: 'Actions', icon: '▸', label: 'Expand all tool output', hint: 'E', order: 120, run: () => setAllTools(true) });
  cockpit.palette.register({ group: 'Actions', icon: '▾', label: 'Collapse all tool output', hint: 'Shift E', order: 130, run: () => setAllTools(false) });

  if (view()) schedule();
}
