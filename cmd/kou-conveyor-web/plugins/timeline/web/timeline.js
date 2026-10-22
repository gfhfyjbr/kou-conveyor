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
import { fmt, h, svg } from '/kernel/dom.js';

// RUN_LIGHT is the light that goes round the card of a call that runs
// (timeline.css, .tool-run): its trail and its head, on the card's edge.
const RUN_LIGHT = '<svg class="tool-run" aria-hidden="true"><rect class="tool-run-tail" width="100%" height="100%" pathLength="100"/><rect class="tool-run-head" width="100%" height="100%" pathLength="100"/></svg>';
const TOOL_GLYPH = { queued: '□', running: '■', done: '✓', failed: '✕', canceled: '⊘', interrupted: '◌' };
const TOOL_WORD = { queued: 'Queued', running: 'Running', done: 'Done', failed: 'Failed', canceled: 'Stopped', interrupted: 'Interrupted' };
// TAIL is how many of the last lines its command printed a call that runs
// shows, closed.
const TAIL = 6;

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
  // Not a live region: the stream would be read out piece by piece, and the
  // entries drawn as the view scrolls with it. The header says the run's state.
  const list = h('ol', { class: 'entries', id: 'entries' });
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
  let lastGap = 0; // how far the view was from the end when last looked at
  // The item a press is on, kept as it is drawn until its click, and
  // whether it was to be drawn anew meanwhile (the press listeners).
  const press = { item: null, stale: false };

  // What the agent writes comes as it writes it: a preview (entry.streaming)
  // shows the answer or the thinking so far, drawn again as each piece
  // comes, until the entry it is recorded as takes its place (replaces).
  // Each piece fades in where it is, for FADE milliseconds from when it
  // came however often its entry is drawn again meanwhile, and a caret
  // ends the text. The motion is the user's to turn off (Settings), and
  // the system's reduced motion turns it off as well.
  const MOTION = 'timeline.stream-motion';
  const FADE = 420;
  const RISE = 320; // how long a preview takes to rise into place
  let motion = cockpit.prefs.get(MOTION, true) !== false;
  const reduced = window.matchMedia?.('(prefers-reduced-motion: reduce)');
  const moving = () => motion && !reduced?.matches;
  // id → { shown, pieces: [{ from, to, at }], born, gist, gistAt }: how
  // much of a preview's text was drawn, and the pieces still fading in.
  const written = new Map();
  // `${id}:${stream}` → where the user scrolled the output of a running
  // command to (its scrollTop), away from its end; output the user did not
  // scroll keeps to its end as it grows.
  const liveScroll = new Map();
  list.classList.toggle('still', !motion);
  if (reduced?.addEventListener) {
    const changed = () => schedule();
    reduced.addEventListener('change', changed);
    cockpit.onDispose(() => reduced.removeEventListener('change', changed));
  }

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

  // gapOf is how far the view is from the end. Holding an entry in place as
  // what is above it changes keeps it; what comes in below makes it bigger;
  // only scrolling down makes it smaller.
  function gapOf(el) {
    return el.scrollHeight - el.scrollTop - el.clientHeight;
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
    // A session just opened shows its end. A view scrolled up from the end,
    // however little, stays where it is.
    const stick = !live || pending.fresh || (following && nearBottom());
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
      written.clear();
      liveScroll.clear();
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
    for (const id of pending.ids) {
      const entry = v.entries.get(id);
      if (!entry) continue;
      // The entry a preview is recorded as, in the preview's place, is not
      // new: it takes the preview's item, to be drawn again where it is.
      if (entry.replaces) adopt(v, entry);
      if (!model.index.has(id) && !(entry.replaces && model.index.has(entry.replaces))) added++;
    }
    // What changed is drawn again, if it is drawn; how all entries look
    // changed, every one drawn is.
    const keep = keptIds(v);
    const marks = markSelection();
    const fresh = [];
    for (const id of pending.all ? new Set([...nodes.keys(), ...keep]) : pending.ids) {
      const entry = v.entries.get(id);
      if (!entry) continue;
      // Pressed on, it is drawn anew after its click.
      if (press.item && nodes.get(id) === press.item) {
        press.stale = true;
        continue;
      }
      if (nodes.has(id) || keep.has(id)) fresh.push([id, redraw(id, entry, ctx)]);
      // What streams grows where it is not drawn: its height is guessed from
      // what it says, not as it was when it was drawn last.
      else if (entry.streaming || v.live?.has(id)) heights.delete(id);
      resized(id);
    }
    if (pending.all) model.from = 0;
    pending.all = false;
    pending.fresh = false;
    pending.ids.clear();
    commit(v);
    putSelectionBack(marks);
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
    if (live) lastGap = gapOf(el);
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
    lastGap = gapOf(el);
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
    const marks = markSelection();
    commit(v);
    putSelectionBack(marks);
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
    followLive();
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
        // What a Write or an Edit call did shows in place of what it said,
        // but for a line.
        const changed = changeText(tool.change);
        if (changed != null) height += stream(changed) + 34;
        for (const text of [changed == null && tool.output, tool.stderr, tool.logs, tool.value]) if (text) height += stream(text);
        if (tool.files?.length) height += 34;
        if (!tool.output && !tool.stderr && !tool.error && !calls && !tool.value && !tool.logs && changed == null) height += 38;
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
        // A preview kept as it was drawn while pressed on, as the entry it
        // was recorded as came (replaces): the click is that entry's.
        if (!v.entries.has(id)) {
          for (const other of v.entries.values()) {
            if (other.replaces === id) {
              id = other.id;
              break;
            }
          }
        }
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

  // The page clicks what a press began and ended on, and an item drawn
  // anew is other elements: what the press began on is gone, and so is the
  // click. What streams is drawn anew with each piece that comes, which a
  // fast model sends more often than a click takes — its Thinking, or a
  // command that prints, would not open. An item pressed on stays as it is
  // drawn until its click, and is drawn anew in the frame after. A mouse's
  // click comes as the button is let go; a tap's a little after, or none
  // does: it is waited for TAP milliseconds.
  const TAP = 400;
  let tapping = null; // cancels the wait
  cockpit.listen(list, 'pointerdown', (event) => {
    release();
    let item = event.isPrimary && event.button === 0 ? event.target : null;
    while (item && item.parentNode !== list) item = item.parentNode;
    if (item && owner.has(item)) press.item = item;
  }, { capture: true });
  function release() {
    tapping?.();
    tapping = null;
    const { item, stale } = press;
    press.item = null;
    press.stale = false;
    const id = item ? owner.get(item) : undefined;
    if (stale && id !== undefined) schedule(id);
  }
  cockpit.listen(document, 'pointerup', (event) => {
    if (!press.item) return;
    tapping?.();
    tapping = cockpit.timeout(release, event.pointerType === 'mouse' ? 0 : TAP);
  }, { capture: true });
  for (const type of ['click', 'pointercancel', 'contextmenu']) cockpit.listen(document, type, release, { capture: true });
  cockpit.listen(window, 'blur', release);
  // A press that ended where the page did not see it (a menu took it): the
  // pointer moves with no button down.
  cockpit.listen(document, 'pointermove', (event) => { if (press.item && !tapping && !(event.buttons & 1)) release(); }, { capture: true, passive: true });

  // A click on a head opens or closes it, but not one that ends selecting
  // text in it: the command or path a head shows can be selected to copy.
  // A selection already there when the button went down does not count.
  let selectedAtPress = '';
  cockpit.listen(list, 'mousedown', () => { selectedAtPress = String(document.getSelection?.() || ''); }, { capture: true });
  function unlessSelecting(fn) {
    return (event) => {
      const selection = document.getSelection?.();
      const text = selection && !selection.isCollapsed ? selection.toString() : '';
      if (text.trim() && text !== selectedAtPress && event.currentTarget.contains(selection.anchorNode)) return;
      fn();
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
      const prose = h('div', { class: 'prose' }, ctx.markdown(entry.text, { onCopy: (text) => ctx.copy(text, 'Code copied') }));
      // A preview has no actions: what it says is not all said yet.
      body.append(
        h('header', { class: 'meta' }, h('span', { class: 'who', text: 'Agent' }),
          entry.phase === 'commentary' && h('span', { class: 'tag', text: 'Note' }), time, !entry.streaming && ctx.actions(entry)),
        prose,
        ...(entry.streaming ? [] : decorations(entry, ctx)));
      streamText(entry, li, prose);
      return li;
    },
    reasoning(entry, ctx) {
      const { li, gutter, body } = shell(entry);
      gutter.append(h('span', { class: 'tick', text: fmt.short(entry.at) }));
      const open = ctx.expanded(entry);
      // Closed, thinking as it comes shows what it is about now; written
      // (the model went on to its answer), it shows as it is recorded.
      const writing = entry.streaming && !entry.settled;
      const gist = writing ? latestGist(entry.text) : entry.text.split('\n').find((line) => line.trim()) || '';
      const toggle = () => ctx.toggle(entry.id, !open);
      const about = h('span', { class: 'gist', text: gist.replace(/\*\*/g, '') });
      body.append(h('div', { class: 'fold', role: 'button', tabindex: 0, 'aria-expanded': String(open), onclick: toggle, onkeydown: activateKeys(toggle) },
        h('span', { class: writing && moving() ? 'who shimmer' : 'who', text: 'Thinking' }),
        about,
        h('span', { class: 'chev', 'aria-hidden': 'true' })));
      const prose = open ? h('div', { class: 'prose thought' }, ctx.markdown(entry.text)) : null;
      if (prose) body.append(prose);
      streamText(entry, li, prose);
      return li;
    },
    tool(entry, ctx) {
      const { li, gutter, body } = shell(entry);
      gutter.append(h('span', { class: 'tick', text: fmt.short(entry.at) }));
      body.append(renderTool(entry, ctx), ...decorations(entry, ctx));
      // A call that runs is drawn again as its command prints.
      if (entry.tool?.state === 'running' || entry.tool?.state === 'queued') clocked(li);
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
    // What the command of a call that runs printed so far, as it prints it.
    const live = state === 'running' || state === 'queued' ? ctx.view?.live?.get(entry.id) : null;
    if (!live) {
      liveScroll.delete(`${entry.id}:out`);
      liveScroll.delete(`${entry.id}:err`);
    }

    const meta = h('span', { class: 'tmeta' }, changeStat(tool.change));
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
      h('div', { class: 'tool-head', role: 'button', tabindex: 0, 'aria-expanded': String(open), onclick: unlessSelecting(toggle), onkeydown: activateKeys(toggle) },
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
      // What a Write call wrote, or an Edit call changed, shows in place of
      // what it said of it, but for the first line.
      const change = changeView(entry, tool.change, state, ctx);
      if (change) panel.append(change);
      // Until the call ends, what its command prints shows as it prints it.
      const out = !tool.output && live?.out ? terminal(live.out) : '';
      const err = !tool.stderr && live?.err ? terminal(live.err) : '';
      if (tool.output) panel.append(change ? h('div', { class: 'fc-said', text: saidLine(tool.output) }) : stream('Output', tool.output, 'out'));
      else if (out) panel.append(tailed(stream('Output', out, 'out'), `${entry.id}:out`));
      if (tool.stderr) panel.append(stream('Stderr', tool.stderr, 'err'));
      else if (err) panel.append(tailed(stream('Stderr', err, 'err'), `${entry.id}:err`));
      if (live && (live.skipped > 0 || live.cut)) panel.append(h('div', { class: 'tool-skip', text: skippedNote(live) }));
      if (tool.logs) panel.append(stream('Console', tool.logs, 'out'));
      if (tool.value) panel.append(stream('Return value', tool.value, 'out'));
      if (tool.files?.length) panel.append(h('div', { class: 'tool-files' }, tool.files.map((file) => h('code', { text: file }))));
      if (!tool.output && !tool.stderr && !tool.error && !calls && !tool.value && !tool.logs && !out && !err && !change) {
        panel.append(h('div', { class: 'tool-empty', text: state === 'running' || state === 'queued' ? 'Waiting for output…' : 'No output' }));
      }
      card.append(panel);
    } else if (calls) {
      // Closed, the calls are the tree the run is, under why it failed.
      card.append(h('div', { class: 'tool-tree' },
        failed && tool.error && h('div', { class: 'tool-peek', text: tool.error.split('\n')[0] }),
        renderCalls(entry, tool, ctx, state, false)));
    } else if (live && terminal(live.all).trim()) {
      // Closed, a call that runs shows the last lines its command printed.
      card.append(liveTail(live.all, unlessSelecting(toggle)));
    } else if (failed && tool.error) {
      card.append(h('div', { class: 'tool-peek', text: tool.error.split('\n')[0] }));
    }
    // While the call runs, a light goes round its card.
    if (state === 'running') card.append(svg(RUN_LIGHT));
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
      h('div', { class: 'call-head', role: 'button', tabindex: 0, 'aria-expanded': String(shown), onclick: unlessSelecting(toggle), onkeydown: activateKeys(toggle) },
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
      h('div', { class: 'call-head', role: 'button', tabindex: 0, title: 'Show every call', onclick: unlessSelecting(expand), onkeydown: activateKeys(expand) },
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

  // ---------------------------------------------------------------- files written and edited

  // CHANGE_ROWS is how many lines of what a Write call wrote, or of what an
  // Edit call changed, an open call shows until asked for all of them.
  const CHANGE_ROWS = 400;

  // changeText is what changeView shows of a Write or an Edit call
  // (tool.change): what it wrote, or its diff; null if nothing.
  function changeText(change) {
    if (change?.kind === 'write') return change.content || '';
    return change?.kind === 'edit' && change.diff ? change.diff : null;
  }

  // changeView is what a Write call wrote, its lines numbered, or what an
  // Edit call changed: the lines it took out and put in, among the lines
  // around them, numbered as in the file where the edit said where it was;
  // the code in the colours of its tokens. A call that failed shows what it
  // was to do. Null when there is nothing to show.
  //
  //   DIFF · +2 −1                                       COPY
  //   41  41    func main() {
  //   42      -   println(1)
  //       42  +   println(2)
  //       43  +   println(3)
  //   43  44    }
  function changeView(entry, change, state, ctx) {
    const text = changeText(change);
    if (text == null) return null;
    const write = change.kind === 'write';
    const key = `${entry.id}:change`;
    const total = fmt.lines(text);
    const limit = ctx.view.expanded.get(key) === true ? Infinity : CHANGE_ROWS;
    const lines = codeLines(text, change.syntax, session.state?.config?.syntax_classes, limit);
    const failed = state === 'failed';
    let label;
    let body;
    if (write) {
      label = `${failed ? 'Not written' : 'Content'} · ${total} ${total === 1 ? 'line' : 'lines'}`;
      body = text
        ? h('div', { class: 'fc-rows', data: { kind: 'write' } }, lines.map((pieces, i) => h('div', { class: 'fc-row' },
          h('span', { class: 'fc-n', text: String(i + 1) }),
          h('span', { class: 'fc-c' }, pieces))))
        : h('div', { class: 'fc-empty', text: 'An empty file' });
      body.style.setProperty('--n', `${Math.max(2, String(lines.length).length)}ch`);
    } else {
      const { added, removed } = diffCounts(text);
      const numbered = change.line > 0;
      label = [failed ? 'Not applied' : 'Diff', `+${added} −${removed}`, change.replacements > 1 && `${change.replacements} replacements`].filter(Boolean).join(' · ');
      const { rows, last } = diffRows(text, lines, numbered);
      body = h('div', { class: 'fc-rows', data: { kind: 'edit', numbered: String(numbered) } }, rows);
      body.style.setProperty('--n', `${Math.max(2, String(last).length)}ch`);
    }
    const hidden = total - lines.length;
    const more = () => {
      ctx.view.expanded.set(key, true);
      schedule(entry.id);
    };
    return h('section', { class: 'stream fchange', data: { failed: failed ? 'true' : null } },
      h('header', null,
        h('span', { text: label }),
        button('Copy', () => cockpit.copy(text, write ? 'Content copied' : 'Diff copied'), write ? 'Copy what was written' : 'Copy the diff')),
      body,
      hidden > 0 ? h('button', { class: 'fc-more', type: 'button', onclick: more, text: `Show ${hidden} more ${hidden === 1 ? 'line' : 'lines'}` }) : null);
  }

  // diffRows draws the lines of an Edit call's diff (FileView.diff in
  // cmd/internal/cockpit), as codeLines gives them: a line taken out, put in
  // or kept, with its numbers before the edit and after where numbered, and
  // between two hunks a row for the lines left out. last is the largest
  // number.
  function diffRows(diff, lines, numbered) {
    const rows = [];
    let before = 0;
    let after = 0;
    diff.split('\n', lines.length).forEach((line, i) => {
      const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
      if (hunk) {
        const skipped = +hunk[1] - before;
        if (rows.length) rows.push(h('div', { class: 'fc-row hunk', text: `⋯ ${skipped} ${skipped === 1 ? 'line' : 'lines'}` }));
        before = +hunk[1];
        after = +hunk[2];
        return;
      }
      const sign = line.charAt(0);
      const kind = sign === '-' ? 'del' : sign === '+' ? 'add' : 'kept';
      rows.push(h('div', { class: `fc-row ${kind}` },
        numbered && h('span', { class: 'fc-n', text: kind === 'add' ? '' : String(before) }),
        numbered && h('span', { class: 'fc-n', text: kind === 'del' ? '' : String(after) }),
        h('span', { class: 'fc-s', text: kind === 'kept' ? ' ' : sign }),
        h('span', { class: 'fc-c' }, unsigned(lines[i]))));
      if (kind !== 'add') before++;
      if (kind !== 'del') after++;
    });
    return { rows, last: Math.max(before, after) - 1 };
  }

  // unsigned is the pieces of a line of a diff without its sign, the first
  // character: plain, so the start of the first piece.
  function unsigned(pieces = []) {
    const [first, ...rest] = pieces;
    if (first instanceof Node) {
      first.textContent = first.textContent.slice(1);
      return pieces;
    }
    return first && first.length > 1 ? [first.slice(1), ...rest] : rest;
  }

  // diffCounts counts the lines a diff puts in and takes out.
  function diffCounts(diff) {
    let added = 0;
    let removed = 0;
    for (const line of diff.split('\n')) {
      if (line.startsWith('+')) added++;
      else if (line.startsWith('-')) removed++;
    }
    return { added, removed };
  }

  // codeLines is code as its lines, the first limit of them, each the pieces
  // of its tokens, as highlighted makes them: a token over lines, a comment
  // of many, has a piece in each. A newline at the end starts no line.
  function codeLines(text, syntax, classes, limit = Infinity) {
    const lines = [[]];
    const put = (piece, cls) => {
      for (let start = 0; lines.length <= limit;) {
        const end = piece.indexOf('\n', start);
        const part = end < 0 ? piece.slice(start) : piece.slice(start, end);
        if (part) lines[lines.length - 1].push(cls ? h('span', { class: `tk-${cls}`, text: part }) : part);
        if (end < 0) return;
        lines.push([]);
        start = end + 1;
      }
    };
    let at = 0;
    if (syntax?.length && classes) {
      for (let i = 0; i + 1 < syntax.length && at < text.length && lines.length <= limit; i += 2) {
        put(text.slice(at, at + syntax[i]), classes[syntax[i + 1]]);
        at += syntax[i];
      }
    }
    if (at < text.length) put(text.slice(at), '');
    return lines.slice(0, Math.min(limit, fmt.lines(text)));
  }

  // changeStat says in the head of a Write or an Edit call how much it
  // changed: the lines it put in and took out, or the lines it wrote.
  function changeStat(change) {
    if (change?.kind === 'edit' && change.diff) {
      const { added, removed } = diffCounts(change.diff);
      return h('span', { class: 'fstat', title: `${added} added, ${removed} removed` },
        h('span', { class: 'add', text: `+${added}` }),
        h('span', { class: 'del', text: `−${removed}` }));
    }
    if (change?.kind !== 'write') return null;
    const n = fmt.lines(change.content || '');
    return h('span', { class: 'fstat', text: `${n} ${n === 1 ? 'line' : 'lines'}` });
  }

  // saidLine is the first line of what a Write or an Edit call said it did:
  // the lines around the change after it, the diff shows.
  function saidLine(output) {
    return output.split('\n', 1)[0].replace(/\s*The file now reads around the first change:\s*$/, '');
  }

  // ---------------------------------------------------------------- what streams

  // adopt has the entry a preview was recorded as take the preview's item,
  // to be drawn again in its place, and the height it stood in at.
  function adopt(v, entry) {
    const preview = entry.replaces;
    if (!preview || preview === entry.id || v.entries.has(preview)) return;
    const node = nodes.get(preview);
    if (node !== undefined && !nodes.has(entry.id)) {
      nodes.delete(preview);
      nodes.set(entry.id, node);
      owner.set(node, entry.id);
    }
    const known = heights.get(preview);
    if (known && !heights.has(entry.id)) heights.set(entry.id, known);
    heights.delete(preview);
    pending.ids.delete(preview);
  }

  // streamText draws a preview's text as it comes: the pieces of it that
  // came within FADE fade in, each as far into its fading as it is, and a
  // caret ends it. root is where the text is drawn (null: it is not, the
  // entry is closed). A preview written (settled: the model went on to
  // another part of its response) is drawn as no more to come: no caret,
  // and what of it was not drawn, as it was closed, shows as it is. The
  // entry a preview is recorded as has the preview's last pieces fade in
  // to the end.
  function streamText(entry, li, root) {
    const now = performance.now();
    if (!entry.streaming) {
      const was = entry.replaces ? written.get(entry.replaces) : null;
      if (!was) return;
      written.delete(entry.replaces);
      if (root && moving()) fadeIn(textNodes(root), was.pieces.filter((p) => now - p.at < FADE), now);
      return;
    }
    let rec = written.get(entry.id);
    if (!rec) written.set(entry.id, (rec = { shown: 0, pieces: [], born: now, drawn: false }));
    const drawn = rec.drawn; // whether its text was drawn the time before
    rec.drawn = !!root;
    if (!entry.settled) {
      li.dataset.streaming = 'true';
      clocked(li);
    }
    if (moving() && now - rec.born < RISE) {
      li.classList.add('rising');
      li.style.animationDelay = `${Math.round(rec.born - now)}ms`;
    }
    if (!root) return;
    const texts = textNodes(root);
    let length = 0;
    for (const text of texts) length += text.length;
    if (entry.settled && !drawn) rec.shown = length;
    // Markdown drawn again may come out shorter ("**" becomes bold): what
    // was shown past the end is not any more.
    if (length > rec.shown) rec.pieces.push({ from: rec.shown, to: length, at: now });
    rec.shown = length;
    rec.pieces = rec.pieces
      .filter((p) => now - p.at < FADE && p.from < length)
      .map((p) => (p.to > length ? { ...p, to: length } : p));
    if (moving()) fadeIn(texts, rec.pieces, now);
    if (entry.settled) return;
    const caret = h('span', { class: 'stream-caret', 'aria-hidden': 'true' });
    const last = textNodes(root).pop();
    if (!last) (root.lastElementChild || root).append(caret);
    else {
      // After the text, not in a piece that fades in: the caret shows whole.
      const at = last.parentElement?.classList.contains('fresh') ? last.parentElement : last;
      at.after(caret);
    }
  }

  // textNodes are the text of a preview drawn, in order, but for the
  // labels and buttons of its code blocks.
  function textNodes(root) {
    const out = [];
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
      acceptNode: (node) => (node.parentElement?.closest('figcaption, button, .stream-caret') ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
    });
    for (let node = walker.nextNode(); node; node = walker.nextNode()) out.push(node);
    return out;
  }

  // fadeIn puts each piece of the text, as far into it as it is, in a span
  // that fades in, as far into its fading as it is.
  function fadeIn(texts, pieces, now) {
    if (!pieces.length) return;
    let at = 0;
    for (const node of texts) {
      const from = at;
      const to = at + node.length;
      at = to;
      const parts = [];
      for (const piece of pieces) {
        const start = Math.max(piece.from, from);
        const end = Math.min(piece.to, to);
        if (start < end) parts.push([start - from, end - from, piece.at]);
      }
      if (!parts.length) continue;
      const text = node.data;
      const out = [];
      let cursor = 0;
      for (const [start, end, when] of parts) {
        if (start > cursor) out.push(text.slice(cursor, start));
        const span = h('span', { class: 'fresh', text: text.slice(start, end) });
        span.style.animationDelay = `${Math.round(when - now)}ms`;
        out.push(span);
        cursor = end;
      }
      if (cursor < text.length) out.push(text.slice(cursor));
      node.replaceWith(...out);
    }
  }

  // latestGist is what thinking as it comes is about now: the title of its
  // last part (a line in bold), or else its last line.
  function latestGist(text) {
    const lines = text.split('\n');
    for (let i = lines.length - 1; i >= 0; i--) {
      const title = /^\s*\*\*(.+?)\*\*:?\s*$/.exec(lines[i]);
      if (title) return title[1];
    }
    for (let i = lines.length - 1; i >= 0; i--) if (lines[i].trim()) return lines[i].trim();
    return '';
  }

  // clocked has the animations that loop in an item drawn again, which
  // would start over, go on from where they were: their delay (--clock) is
  // the page's clock.
  function clocked(li) {
    li.style.setProperty('--clock', `${-Math.round(performance.now())}ms`);
  }

  // tailed has the output of a command that runs, drawn again as it grows,
  // keep to its end, unless the user scrolled up in it: then it stays
  // where they put it (followLive).
  function tailed(section, key) {
    const pre = section.querySelector('pre');
    if (!pre) return section;
    pre.dataset.follow = key;
    pre.addEventListener('scroll', () => {
      if (!pre.isConnected) return;
      if (pre.scrollHeight - pre.scrollTop - pre.clientHeight < 4) liveScroll.delete(key);
      else liveScroll.set(key, pre.scrollTop);
    }, { passive: true });
    return section;
  }

  // followLive scrolls the output of each command that runs, as drawn, to
  // its end, or to where the user scrolled it.
  function followLive() {
    for (const pre of list.querySelectorAll('pre[data-follow]')) {
      const kept = liveScroll.get(pre.dataset.follow);
      const top = kept === undefined ? pre.scrollHeight - pre.clientHeight : kept;
      if (Math.abs(pre.scrollTop - top) >= 1) pre.scrollTop = top;
    }
  }

  // terminal is what a terminal shows of what a command printed: of a line
  // it wrote over (a progress bar, after \r), what it wrote last.
  function terminal(text) {
    if (!text.includes('\r')) return text;
    return text.replace(/\r\n/g, '\n').split('\n').map((line) => {
      if (!line.includes('\r')) return line;
      const parts = line.split('\r');
      for (let i = parts.length - 1; i >= 0; i--) if (parts[i]) return parts[i];
      return '';
    }).join('\n');
  }

  // liveTail is the last lines (TAIL) the command of a call that runs
  // printed, under the call closed.
  function liveTail(text, onclick) {
    const lines = terminal(text).replace(/\s+$/, '').split('\n');
    return h('div', { class: 'tool-live', onclick },
      h('pre', { data: { more: lines.length > TAIL ? 'true' : null }, text: lines.slice(-TAIL).join('\n') }));
  }

  // skippedNote says what of a command's output does not show while it runs.
  function skippedNote(live) {
    const notes = [];
    if (live.skipped > 0) notes.push(`${fmt.bytes(live.skipped)} it printed faster than it could be shown were skipped`);
    if (live.cut) notes.push('only the end of the output shows until the command is done');
    const text = notes.join('; ');
    return text.charAt(0).toUpperCase() + text.slice(1);
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
    const gap = gapOf(el);
    const nearer = gap < lastGap - 1; // a pixel's rounding is not a scroll
    lastGap = gap;
    if (atEnd(el)) following = true;
    else if (top < lastTop) following = false;
    lastTop = top;
    request();
    // Scrolled down near the end, the view catches up with what came
    // meanwhile; scrolled up from there, or held in place as entries above
    // it change, it stays where the user put it.
    if (unseen && nearer && nearBottom()) scrollToBottom();
    cockpit.emit('timeline:scroll');
  }, { capture: true, passive: true });

  // A selection dragged from the transcript onto the composer, or onto what
  // is just over it (the activity line, its clock ticking), ends in the
  // dock, past the list: what the agent writes next comes in inside it, and
  // it goes on growing once the button is let go. Let go, it ends at the
  // transcript's last character instead.
  cockpit.listen(document, 'pointerup', () => cockpit.timeout(endInList, 0), { capture: true });

  function endInList() {
    const selection = document.getSelection?.();
    const stage = scroller()?.parentElement;
    if (!selection?.rangeCount || selection.isCollapsed || !stage || !list.isConnected) return;
    const range = selection.getRangeAt(0);
    const end = document.createRange();
    end.setStart(list, list.childNodes.length);
    if (end.comparePoint(range.startContainer, range.startOffset) >= 0) return;
    if (end.comparePoint(range.endContainer, range.endOffset) <= 0 || !stage.contains(range.endContainer)) return;
    const text = lastText(list);
    if (!text || range.comparePoint(text, text.length) !== 0) return;
    const forward = selection.anchorNode === range.startContainer && selection.anchorOffset === range.startOffset;
    if (forward) selection.setBaseAndExtent(range.startContainer, range.startOffset, text, text.length);
    else selection.setBaseAndExtent(text, text.length, range.startContainer, range.startOffset);
  }

  // lastText is the last text shown in node.
  function lastText(node) {
    let deepest = node;
    while (deepest.lastChild) deepest = deepest.lastChild;
    const walker = document.createTreeWalker(node, NodeFilter.SHOW_TEXT);
    walker.currentNode = deepest;
    for (let text = deepest.nodeType === Node.TEXT_NODE ? deepest : walker.previousNode(); text; text = walker.previousNode()) {
      if (text.data.trim() && text.parentElement?.getClientRects().length) return text;
    }
    return null;
  }

  // Drawing an entry again, or laying the list out again, takes an end of
  // the selection that is in an entry out of it, to the list, where it
  // keeps to no text: the selection lets go of what it held there, or comes
  // to hold other entries as the list changes. Each end is marked where it
  // is in its entry first, and put back there in the entry drawn now.
  function markSelection() {
    const selection = document.getSelection?.();
    // In a text field, the selection is the field's own.
    if (!selection?.rangeCount || selection.isCollapsed || 'selectionStart' in (document.activeElement || {})) return null;
    const anchor = markAt(selection.anchorNode, selection.anchorOffset);
    const focus = markAt(selection.focusNode, selection.focusOffset);
    return anchor || focus ? { anchor, focus } : null;
  }

  function putSelectionBack(marks) {
    const selection = document.getSelection?.();
    if (!marks || !selection?.rangeCount) return;
    const back = (mark, node, offset) => {
      const item = mark && nodes.get(mark.id);
      if (!item?.isConnected || (item === mark.item && item.contains(node))) return [node, offset];
      return pointAt(item, mark);
    };
    const [anchor, anchorOffset] = back(marks.anchor, selection.anchorNode, selection.anchorOffset);
    const [focus, focusOffset] = back(marks.focus, selection.focusNode, selection.focusOffset);
    if (anchor === selection.anchorNode && anchorOffset === selection.anchorOffset && focus === selection.focusNode && focusOffset === selection.focusOffset) return;
    selection.setBaseAndExtent(anchor, anchorOffset, focus, focusOffset);
  }

  // markAt is where a point is in the entry it is in, if it is in one: the
  // way down to the element it is in, each step an element's tag and class
  // and how many before it have them, and how far into that element's text
  // it is. Drawn again, an entry has its text in the same elements, though
  // what is beside them (its actions, its flags) changed.
  function markAt(node, offset) {
    let item = node;
    while (item && item.parentNode !== list) item = item.parentNode;
    const id = item ? owner.get(item) : undefined;
    if (id === undefined) return null;
    let host = node.nodeType === Node.ELEMENT_NODE ? node : node.parentElement;
    // A piece of a preview's text that fades in is in a span of its own,
    // which the preview drawn again has elsewhere, or not at all.
    while (host !== item && host.classList.contains('fresh')) host = host.parentElement;
    const path = [];
    for (let el = host; el && el !== item; el = el.parentElement) {
      let nth = 0;
      for (let before = el.previousElementSibling; before; before = before.previousElementSibling) if (kindOf(before) === kindOf(el)) nth++;
      path.unshift([kindOf(el), nth]);
    }
    const before = document.createRange();
    before.setStart(host, 0);
    before.setEnd(node, offset);
    return { id, item, path, chars: before.toString().length };
  }

  function kindOf(el) {
    return `${el.tagName}.${el.getAttribute('class') || ''}`;
  }

  // pointAt is where a mark is in item: as far down its way as item has the
  // elements, that far into their text.
  function pointAt(item, mark) {
    let host = item;
    for (const [kind, nth] of mark.path) {
      const next = [...host.children].filter((child) => kindOf(child) === kind)[nth];
      if (!next) break;
      host = next;
    }
    let chars = mark.chars;
    let last = null;
    const walker = document.createTreeWalker(host, NodeFilter.SHOW_TEXT);
    for (let text = walker.nextNode(); text; text = walker.nextNode()) {
      if (chars <= text.length) return [text, chars];
      chars -= text.length;
      last = text;
    }
    return last ? [last, last.length] : [host, host.childNodes.length];
  }

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
      written.delete(id);
      liveScroll.delete(`${id}:out`);
      liveScroll.delete(`${id}:err`);
    }
  }

  // ---------------------------------------------------------------- settings

  // setMotion turns the motion of what streams on or off.
  function setMotion(on) {
    motion = !!on;
    cockpit.prefs.set(MOTION, motion);
    list.classList.toggle('still', !motion);
    schedule();
  }

  cockpit.contribute('settings.section', {
    id: 'transcript', title: 'Transcript', order: 10,
    render: () => {
      const input = h('input', { type: 'checkbox', id: 'stream-motion', onchange: (event) => setMotion(event.currentTarget.checked) });
      input.checked = motion;
      return h('div', { class: 'set-row' },
        h('label', { class: 'set-toggle' }, input, 'Animate text as it streams'),
        h('p', { class: 'set-note', text: 'The agent\'s answer and thinking show as they are written: new words fade in and a caret ends them. Off, the text just appears. The system\'s Reduce motion turns the animation off too.' }));
    },
  });

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
