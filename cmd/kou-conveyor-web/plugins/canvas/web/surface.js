// The board of a canvas: an endless grid, panned and zoomed, with the
// nodes on it in their frames and the edges between them. It draws the
// model's document and keeps it drawn as it changes, and turns what is
// done to it into the model's operations: selecting (a click, Shift, a
// frame dragged around nodes), moving and resizing nodes, wiring ports,
// panning (Space or the middle button and a drag, the wheel, a finger) and
// zooming (⌘ and the wheel, a pinch). What a node shows is its body's
// (nodes/, or a plugin's through canvas.node); how much of it, and which
// terminals draw live, the zoom says (lod.js).
//
// A wheel over a node pans the board, unless the node is the one selected
// or in use: then what can scroll in it scrolls — its feed, its terminal.
import { GRID, snap, clampZoom, boundsOf, overlaps, sizeOf, MIN_ZOOM, MAX_ZOOM } from './geometry.js';
import { createEdges } from './edges.js';
import { createFrame } from './frame.js';
import { levelOf, createBudget, LIVE, LOW } from './lod.js';

// The least size of a node, as the engine has it.
const MIN_W = 160;
const MIN_H = 90;
// How often the places of nodes being dragged go to the server, for the
// other pages showing the canvas: ten times a second at most.
const SEND_EVERY = 100;
// The room fitting leaves around the nodes.
const PAD = 48;
// The zooms ⌘= and ⌘− step through.
const STEPS = [0.1, 0.15, 0.25, 0.35, 0.5, 0.6, 0.75, 1, 1.25, 1.5, 2];

const typing = (el) => el instanceof HTMLElement && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName));
const reduced = () => matchMedia('(prefers-reduced-motion: reduce)').matches;

export function createSurface(model, env) {
  const { cockpit, h } = env;
  const stop = new AbortController();
  const on = (target, type, fn, options = {}) => target.addEventListener(type, (event) => cockpit.safely(fn, event), { ...options, signal: stop.signal });

  const view = { x: 0, y: 0, zoom: 1 };
  const frames = new Map(); // node → { id, frame, body, visible, level, granted, w, h }
  const selection = new Set();
  const budget = createBudget();
  let edgeSelected = null;
  let entered = null; // the node whose body has the keys
  let pinned = null; // the node in focus, live however far the board is zoomed out
  let placed = false; // the view was placed once: restored, or fitted
  let pageShown = false;
  let spaceHeld = false;
  let pointer = null; // where the pointer was last over the board
  let gesture = null; // the drag going on: { abort() }
  let pendingSelect = null; // nodes to select once they are drawn
  let disposed = false;

  // ---------------------------------------------------------------- elements

  const layer = h('div', { class: 'cv-nodes' });
  const edges = createEdges({
    cockpit,
    model: () => model,
    kinds: env.kinds,
    nodeOf: (id) => model.node(id),
    onSelect: (id, event) => selectEdge(id, event),
    onOpen: (id) => env.inspect?.({ edge: id }),
    onConnect: (from, to) => connect(from, to),
    onDropEmpty: (at, from, client) => dropEmpty(at, from, client),
    world: () => world,
    viewport: () => viewport,
    worldAt: (x, y) => worldAt(x, y),
  });
  const world = h('div', { class: 'cv-world' }, edges.element, layer);
  const marquee = h('div', { class: 'cv-marquee', hidden: true });
  // The menus the board opens where the pointer is hang from this.
  const anchor = h('button', { class: 'cv-anchor', type: 'button', tabindex: '-1', 'aria-hidden': 'true' });
  const viewport = h('div', {
    class: 'cv-viewport', tabindex: '0', role: 'application',
    'aria-label': 'The canvas: Tab goes from node to node, Enter enters one, A adds one',
  }, world, marquee, anchor);

  // ---------------------------------------------------------------- the view

  function applyView() {
    world.style.transform = `translate(${view.x}px, ${view.y}px) scale(${view.zoom})`;
    // The grid's dots, four snaps apart, further when they would crowd.
    let step = GRID * 4 * view.zoom;
    while (step < 12) step *= 4;
    viewport.style.setProperty('--cv-step', `${step}px`);
    viewport.style.setProperty('--cv-ox', `${view.x}px`);
    viewport.style.setProperty('--cv-oy', `${view.y}px`);
    // The styles keep lines, the selection and a far node's title legible
    // by the zoom; a terminal takes the pointer only at 1 (canvas.css).
    viewport.style.setProperty('--cv-zoom', String(view.zoom));
    const unit = Math.abs(view.zoom - 1) < 0.001 ? 'true' : '';
    if ((viewport.dataset.unit || '') !== unit) viewport.dataset.unit = unit;
    const level = levelOf(view.zoom);
    if (viewport.dataset.lod !== level) viewport.dataset.lod = level;
    env.onView?.({ ...view });
    scheduleLod();
    saveView();
  }

  let animation = 0;
  function setView(next, { animate = false } = {}) {
    cancelAnimationFrame(animation);
    const target = { x: next.x ?? view.x, y: next.y ?? view.y, zoom: clampZoom(next.zoom ?? view.zoom) };
    if (!animate || reduced()) {
      Object.assign(view, target);
      applyView();
      return;
    }
    const from = { ...view };
    const start = performance.now();
    const step = (now) => {
      const t = Math.min(1, (now - start) / 240);
      const e = 1 - (1 - t) ** 3;
      view.zoom = Math.exp(Math.log(from.zoom) + (Math.log(target.zoom) - Math.log(from.zoom)) * e);
      view.x = from.x + (target.x - from.x) * e;
      view.y = from.y + (target.y - from.y) * e;
      applyView();
      if (t < 1) animation = requestAnimationFrame(step);
    };
    animation = requestAnimationFrame(step);
  }

  let saveTimer = 0;
  let unsaved = null;
  function saveView() {
    if (!placed) return;
    clearTimeout(saveTimer);
    unsaved = { x: Math.round(view.x), y: Math.round(view.y), zoom: Number(view.zoom.toFixed(3)) };
    saveTimer = setTimeout(flushView, 400);
  }
  function flushView() {
    clearTimeout(saveTimer);
    if (unsaved) env.saveView?.(unsaved);
    unsaved = null;
  }

  // worldAt is the point of the world under a point of the screen.
  function worldAt(clientX, clientY) {
    const r = viewport.getBoundingClientRect();
    return { x: (clientX - r.left - view.x) / view.zoom, y: (clientY - r.top - view.y) / view.zoom };
  }

  // area is the part of the board nothing covers: the inspector, open on
  // its right, and the foreman's field at its foot cover the rest.
  function area() {
    const rect = viewport.getBoundingClientRect();
    const inset = env.insets?.() || {};
    const left = inset.left || 0;
    const top = inset.top || 0;
    return {
      rect, left, top,
      width: Math.max(1, rect.width - left - (inset.right || 0)),
      height: Math.max(1, rect.height - top - (inset.bottom || 0)),
    };
  }

  // shownBox is the world the area shows.
  function shownBox() {
    const a = area();
    return { x: (a.left - view.x) / view.zoom, y: (a.top - view.y) / view.zoom, w: a.width / view.zoom, h: a.height / view.zoom };
  }
  const inSight = (n) => overlaps(n, shownBox());

  function zoomAt(zoom, clientX, clientY, options) {
    const r = viewport.getBoundingClientRect();
    const px = clientX - r.left;
    const py = clientY - r.top;
    const z = clampZoom(zoom);
    const wx = (px - view.x) / view.zoom;
    const wy = (py - view.y) / view.zoom;
    setView({ zoom: z, x: px - wx * z, y: py - wy * z }, options);
  }

  // zoomBy steps the zoom in (1) or out (-1), about the middle.
  function zoomBy(direction) {
    const z = view.zoom;
    const next = direction > 0
      ? STEPS.find((s) => s > z + 0.001) ?? MAX_ZOOM
      : [...STEPS].reverse().find((s) => s < z - 0.001) ?? MIN_ZOOM;
    const a = area();
    zoomAt(next, a.rect.left + a.left + a.width / 2, a.rect.top + a.top + a.height / 2, { animate: true });
  }

  // fit shows the nodes given, or all, as large as they fit up to max.
  function fit(ids = null, { animate = true, max = 1 } = {}) {
    const list = (ids?.length ? ids.map((id) => model.node(id)) : model.doc.nodes).filter(Boolean);
    const a = area();
    if (a.rect.width < 20 || a.rect.height < 20) return false;
    const b = boundsOf(list);
    if (!b) {
      // Nothing yet: the origin, where the first node goes, in the middle.
      setView({ zoom: 1, x: Math.round(a.left + a.width / 2 - 380), y: Math.round(a.top + Math.max(24, a.height / 2 - 300)) }, { animate });
      return true;
    }
    const z = clampZoom(Math.min(max, (a.width - PAD * 2) / Math.max(1, b.w), (a.height - PAD * 2) / Math.max(1, b.h)));
    setView({ zoom: z, x: a.left + (a.width - b.w * z) / 2 - b.x * z, y: a.top + (a.height - b.h * z) / 2 - b.y * z }, { animate });
    return true;
  }

  // zoom100 shows the selection at its size, or zooms to 100% about the
  // middle.
  function zoom100() {
    const ids = [...selection];
    if (!ids.length) return zoomAt(1, ...middle(), { animate: true });
    const b = boundsOf(ids.map((id) => model.node(id)).filter(Boolean));
    if (!b) return undefined;
    const a = area();
    return setView({ zoom: 1, x: a.left + (a.width - b.w) / 2 - b.x, y: a.top + (a.height - b.h) / 2 - b.y }, { animate: true });
  }

  function middle() {
    const a = area();
    return [a.rect.left + a.left + a.width / 2, a.rect.top + a.top + a.height / 2];
  }

  function center(id, { zoom = view.zoom, animate = true } = {}) {
    const n = model.node(id);
    if (!n) return;
    const a = area();
    const z = clampZoom(zoom);
    // A node taller than the area shows from its top.
    const y = n.h * z > a.height - 16 ? a.top + 8 - n.y * z : a.top + (a.height - n.h * z) / 2 - n.y * z;
    setView({ zoom: z, x: a.left + (a.width - n.w * z) / 2 - n.x * z, y }, { animate });
  }

  // focusMode shows a node at 100%, in the middle — smaller if it does not
  // fit, as on a phone — and keeps it live.
  function focusMode(id) {
    const n = model.node(id);
    if (!n) return;
    const a = area();
    pinned = id;
    setSelection([id]);
    budget.touch(id);
    const z = Math.min(1, (a.width - 32) / n.w, (a.height - 32) / n.h);
    center(id, { zoom: Math.max(LOW, z) });
    scheduleLod(260);
  }

  // ---------------------------------------------------------------- levels of detail

  let lodTimer = 0;
  let lodZoom = 0;
  function scheduleLod(delay = 150) {
    clearTimeout(lodTimer);
    lodTimer = setTimeout(lodNow, delay);
  }

  // lodNow tells each body whether it is in view, how much of it shows,
  // and whether it draws live.
  function lodNow() {
    clearTimeout(lodTimer);
    lodTimer = 0;
    if (disposed) return;
    const level = levelOf(view.zoom);
    const r = viewport.getBoundingClientRect();
    const margin = 160;
    const box = { x: (-margin - view.x) / view.zoom, y: (-margin - view.y) / view.zoom, w: (r.width + margin * 2) / view.zoom, h: (r.height + margin * 2) / view.zoom };
    const wanting = [];
    const next = new Map();
    for (const [id] of frames) {
      const n = model.node(id);
      const visible = !!n && pageShown && r.width > 0 && overlaps(n, box);
      const own = id === pinned && level !== 'low' ? 'full' : level;
      next.set(id, { visible, level: own });
      if (visible && own === 'full' && n.kind === 'terminal' && !n.proposed) wanting.push(id);
    }
    const granted = budget.grant(wanting);
    if (pinned && wanting.includes(pinned)) granted.add(pinned);
    for (const [id, item] of frames) {
      const { visible, level: own } = next.get(id);
      if (item.frame.el.dataset.lod !== own) item.frame.el.dataset.lod = own;
      if (visible !== item.visible) {
        item.visible = visible;
        call(item, visible ? 'shown' : 'hidden');
      }
      const live = granted.has(id);
      if (own !== item.level || live !== item.granted) {
        item.level = own;
        item.granted = live;
        call(item, 'lod', own, live);
      }
    }
    checkEntered();
    if (lodZoom !== view.zoom) {
      lodZoom = view.zoom;
      for (const item of frames.values()) if (item.visible) call(item, 'zoomed', view.zoom);
    }
  }

  // ---------------------------------------------------------------- frames

  const call = (item, name, ...args) => {
    const fn = item?.body?.[name];
    return typeof fn === 'function' ? cockpit.safely(() => fn.apply(item.body, args)) : undefined;
  };

  function kindLabel(n) {
    let label = '';
    if (n.kind === 'terminal') label = (env.kinds.harness(n)?.title || n.preset || '').replace(/…$/, '');
    else if (n.kind === 'agent') label = n.preset === 'foreman' ? 'foreman' : 'kou';
    else if (n.kind === 'source') label = env.kinds.source(n)?.title || n.preset || '';
    return label && label.toLowerCase() !== (n.title || '').toLowerCase() ? label : '';
  }

  function draw(item, n = model.node(item.id)) {
    if (!n) return;
    item.frame.update(n, {
      status: model.status.get(n.id),
      terminal: model.terminals.get(n.id),
      pendingCount: model.pendingFor(n.id),
      awaiting: model.awaitingFor(n.id),
      selected: selection.has(n.id),
      kindLabel: kindLabel(n),
    });
    const inside = entered === n.id ? 'true' : '';
    if (item.frame.el.dataset.entered !== inside) item.frame.el.dataset.entered = inside;
  }

  // contextFor is what a node's body is given to work with.
  function contextFor(id) {
    return {
      cockpit, h, fmt: env.fmt, model, kinds: env.kinds,
      zoom: () => view.zoom,
      touch: () => budget.touch(id),
      toast: (text, kind) => cockpit.toast(text, kind),
      update: (set, label = '') => model.apply([{ op: 'node.update', id, set }], { label }).catch((error) => cockpit.toast(error.message, 'error')),
      apply: (ops, options) => model.apply(ops, options),
      markdown: (text) => env.markdown(text),
      modelTitle: (m) => env.modelTitle(m),
      inspect: () => env.inspect?.({ node: id }),
      openSession: (session) => env.openSession?.(session),
      select: () => setSelection([id]),
      focus: () => enter(id),
      center: () => center(id),
    };
  }

  function makeFrame(n) {
    const frame = createFrame(n, {
      h, fmt: env.fmt, kinds: env.kinds, short: env.short, modelTitle: env.modelTitle, titleOf: (x) => model.titleOf(x),
    });
    const item = { id: n.id, frame, body: null, visible: false, level: '', granted: false, w: n.w, h: n.h, sizing: 0 };
    let body = null;
    try {
      body = env.bodyFor(n, contextFor(n.id));
    } catch (error) {
      console.error(error);
    }
    if (!body || !(body.node instanceof Node)) {
      body = { node: h('p', { class: 'cv-no-body', text: `Nothing here draws a ${n.kind} node${n.preset ? ` (${n.preset})` : ''}: is its plugin on?` }) };
    }
    item.body = body;
    frame.body.append(body.node);
    layer.append(frame.el);
    frames.set(n.id, item);
    return item;
  }

  function dropFrame(id) {
    const item = frames.get(id);
    if (!item) return;
    frames.delete(id);
    budget.forget(id);
    cancelAnimationFrame(item.sizing);
    call(item, 'dispose');
    item.frame.dispose();
    if (entered === id) entered = null;
    if (pinned === id) pinned = null;
  }

  function resized(item) {
    if (item.sizing) return;
    item.sizing = requestAnimationFrame(() => {
      item.sizing = 0;
      call(item, 'resized');
    });
  }

  // sync draws the document as the model has it.
  function sync() {
    if (disposed) return;
    const nodes = model.doc.nodes;
    const want = new Set(nodes.map((n) => n.id));
    for (const id of [...frames.keys()]) if (!want.has(id)) dropFrame(id);
    for (const n of nodes) {
      const fresh = !frames.has(n.id);
      const item = frames.get(n.id) || makeFrame(n);
      draw(item, n);
      if (fresh) continue;
      call(item, 'update', n);
      if (item.w !== n.w || item.h !== n.h) {
        item.w = n.w;
        item.h = n.h;
        resized(item);
      }
    }
    let changed = false;
    for (const id of [...selection]) {
      if (want.has(id)) continue;
      selection.delete(id);
      changed = true;
    }
    edges.sync();
    if (edgeSelected && !model.edge(edgeSelected)) {
      edgeSelected = null;
      edges.select(null);
      changed = true;
    }
    sizeEdges();
    if (pendingSelect && pendingSelect.every((id) => want.has(id))) {
      const ids = pendingSelect;
      pendingSelect = null;
      setSelection(ids);
      const n = model.node(ids[0]);
      if (n && placed && !inSight(n)) center(ids[0]);
      changed = false;
    }
    if (changed) selected();
    scheduleLod(0);
  }

  // moved draws the nodes that moved, and their edges.
  function moved(ids) {
    for (const id of ids) {
      const item = frames.get(id);
      const n = model.node(id);
      if (!item || !n) continue;
      item.frame.place(n);
      if (item.w !== n.w || item.h !== n.h) {
        item.w = n.w;
        item.h = n.h;
        resized(item);
      }
    }
    edges.move(ids);
  }

  // sizeEdges makes the edges' SVG as large as the nodes and some: what
  // is outside it shows, but the pointer does not find it there.
  function sizeEdges() {
    const b = boundsOf(model.doc.nodes) || { x: 0, y: 0, w: 0, h: 0 };
    const m = 800;
    const x = Math.floor(b.x - m);
    const y = Math.floor(b.y - m);
    const w = Math.ceil(b.w + m * 2);
    const hh = Math.ceil(b.h + m * 2);
    const box = `${x} ${y} ${w} ${hh}`;
    if (edges.element.getAttribute('viewBox') === box) return;
    edges.element.setAttribute('viewBox', box);
    edges.element.setAttribute('width', String(w));
    edges.element.setAttribute('height', String(hh));
    edges.element.style.left = `${x}px`;
    edges.element.style.top = `${y}px`;
  }

  // ---------------------------------------------------------------- selection

  const selected = () => env.onSelect?.({ nodes: [...selection], edge: edgeSelected });

  function setSelection(ids, { edge = null } = {}) {
    const next = new Set(ids);
    const same = next.size === selection.size && [...next].every((id) => selection.has(id)) && edge === edgeSelected;
    if (same) return;
    const touched = new Set([...selection, ...next]);
    selection.clear();
    for (const id of next) if (model.node(id)) selection.add(id);
    edgeSelected = edge;
    edges.select(edge);
    for (const id of touched) {
      const item = frames.get(id);
      if (item) draw(item);
    }
    selected();
  }

  // pick selects the node a click was on: with Shift, it joins the
  // selection or leaves it.
  function pick(id, event) {
    budget.touch(id);
    if (event?.shiftKey) {
      const next = new Set(selection);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      setSelection(next);
    } else if (!selection.has(id) || selection.size > 1 || edgeSelected) {
      setSelection([id]);
    }
  }

  function selectEdge(id, event) {
    viewport.focus({ preventScroll: true });
    setSelection(event?.shiftKey ? [...selection] : [], { edge: id });
  }

  // The node whose body has the keys: Enter took them there, or a click.
  function setEntered(id) {
    if (entered === id) return;
    const was = entered;
    entered = id;
    for (const x of [was, id]) {
      const item = x && frames.get(x);
      if (item) item.frame.el.dataset.entered = entered === x ? 'true' : '';
    }
    if (id) {
      budget.touch(id);
      if (!selection.has(id) || selection.size > 1) setSelection([id]);
    }
    env.onEnter?.(id);
  }

  // checkEntered lets go of the node shown entered once its body no longer
  // has the keys: the element that had them was removed — a terminal that
  // stopped drawing live — and no focusout said so.
  function checkEntered() {
    if (entered && !frames.get(entered)?.frame.el.contains(document.activeElement)) setEntered(null);
  }

  // enter gives a node the keys: a terminal's shell, an agent's field.
  // A terminal draws live only near enough: the board goes to it first.
  function enter(id = [...selection][0]) {
    const item = frames.get(id);
    const n = model.node(id);
    if (!item || !n) return false;
    setSelection([id]);
    budget.touch(id);
    if (n.kind === 'terminal' && !call(item, 'live')) {
      if (view.zoom < LIVE || !inSight(n)) focusMode(id);
      else {
        pinned = id;
        lodNow();
      }
    } else if (!inSight(n)) {
      center(id);
    }
    let ok = false;
    try {
      ok = !!item.body.focus?.();
    } catch (error) {
      console.error(error);
    }
    return ok;
  }

  // exit takes the keys back to the board.
  function exit() {
    const active = document.activeElement;
    if (active && layer.contains(active)) active.blur?.();
    for (const item of frames.values()) if (item.id === entered) call(item, 'blur');
    setEntered(null);
    viewport.focus({ preventScroll: true });
  }

  // next selects the next node, or the one before, in reading order.
  function next(step) {
    const list = [...model.doc.nodes].sort((a, b) => (a.y - b.y) || (a.x - b.x));
    if (!list.length) return;
    const current = [...selection][0];
    let at = list.findIndex((n) => n.id === current);
    at = at < 0 ? (step > 0 ? 0 : list.length - 1) : (at + step + list.length) % list.length;
    const n = list[at];
    setSelection([n.id]);
    if (!inSight(n)) center(n.id);
    viewport.focus({ preventScroll: true });
  }

  // ---------------------------------------------------------------- changing the canvas

  const failed = (what) => (error) => cockpit.toast(`${what}: ${error.message}`, 'error');

  async function removeNodes(ids, keep = null) {
    const list = ids.map((id) => model.node(id)).filter(Boolean);
    if (!list.length || model.readOnly) return;
    const ops = list.map((n) => (keep ? { op: 'node.remove', id: n.id, keep } : { op: 'node.remove', id: n.id }));
    const what = list.length > 1 ? `${list.length} nodes` : `«${list[0].title}»`;
    try {
      await model.apply(ops, { label: `Delete ${what}` });
    } catch (error) {
      failed('It was not deleted')(error);
      return;
    }
    viewport.focus({ preventScroll: true });
    const shells = list.some((n) => n.kind === 'terminal');
    env.undoBar?.(`Deleted ${what}${shells ? ' · a shell ends in 15s' : ''}`);
  }

  function removeSelected() {
    if (selection.size) return removeNodes([...selection]);
    if (edgeSelected && !model.readOnly) {
      const id = edgeSelected;
      return model.apply([{ op: 'edge.remove', id }], { label: 'Delete the edge' })
        .then(() => env.undoBar?.('Edge deleted'), failed('The edge was not deleted'));
    }
    return undefined;
  }

  async function duplicateSelected() {
    const list = [...selection].map((id) => model.node(id)).filter(Boolean);
    if (!list.length || model.readOnly) return;
    const ops = list.map((n) => ({
      op: 'node.add',
      node: {
        kind: n.kind, preset: n.preset || '', plugin: n.plugin || '', title: `${n.title} copy`,
        x: n.x + 32, y: n.y + 32, w: n.w, h: n.h,
        ...(n.config && Object.keys(n.config).length ? { config: n.config } : {}),
        ...(n.access ? { access: n.access } : {}),
      },
    }));
    try {
      const result = await model.apply(ops, { label: list.length > 1 ? 'Duplicate nodes' : `Duplicate «${list[0].title}»` });
      const ids = Object.values(result?.ids || {});
      if (ids.length) {
        pendingSelect = ids;
        if (ids.every((id) => model.node(id))) sync();
      }
    } catch (error) {
      failed('It was not duplicated')(error);
    }
  }

  // add puts a node on the canvas: at a point of the world (its top left;
  // wired from the left, its input there; to the right, its output), or
  // in the middle of the board.
  async function add(spec, at = null, { connect = null, label = '' } = {}) {
    if (model.readOnly) return null;
    const node = { ...spec };
    const [w, hh] = sizeOf(node.kind, node.preset);
    const width = node.w || w;
    const height = node.h || hh;
    let point = at;
    if (!point) {
      const [cx, cy] = middle();
      const c = worldAt(cx, cy);
      point = { x: c.x - width / 2, y: c.y - height / 2 };
    } else if (connect?.to) {
      point = { x: point.x - width, y: point.y - 54 };
    } else if (connect?.from) {
      point = { x: point.x, y: point.y - 54 };
    }
    node.x = snap(point.x);
    node.y = snap(point.y);
    if (connect) node.connect = connect;
    const result = await model.apply([{ op: 'node.add', node }], { label: label || `Add ${spec.title || spec.preset || spec.kind}` });
    const made = result?.ids?.['0'] || '';
    if (made) {
      pendingSelect = [made];
      if (model.node(made)) sync();
    }
    return made;
  }

  function connect(from, to) {
    if (from.node === to.node || model.readOnly) return;
    if (model.doc.edges.some((e) => e.from.node === from.node && e.from.port === from.port && e.to.node === to.node && e.to.port === to.port)) {
      cockpit.toast('They are wired so already');
      return;
    }
    model.apply([{ op: 'edge.add', edge: { from, to } }], { label: `Wire «${model.titleOf(from.node)}» to «${model.titleOf(to.node)}»` })
      .catch(failed('They were not wired'));
  }

  function dropEmpty(at, from, client) {
    if (model.readOnly) return;
    placeAnchor(client.clientX, client.clientY);
    const wire = from.dir === 'out' ? { from: `${from.node}:${from.port}` } : { to: `${from.node}:${from.port}` };
    env.addMenu?.(anchor, at, { connect: wire });
  }

  function approve(id) {
    model.apply([{ op: 'node.update', id, set: { proposed: false } }], { label: 'Approve the node' }).catch(failed('It was not approved'));
  }
  function discard(id) {
    model.apply([{ op: 'node.remove', id }], { label: 'Discard the node' }).catch(failed('It was not discarded'));
  }

  // rename edits a node's title in its header.
  function rename(id) {
    const item = frames.get(id);
    const n = model.node(id);
    if (!item || !n || model.readOnly || item.frame.head.querySelector('.cv-rename')) return;
    const title = item.frame.title;
    const input = h('input', { class: 'cv-rename', type: 'text', maxlength: '120', spellcheck: 'false', autocomplete: 'off', 'aria-label': 'The node\'s title' });
    input.value = n.title || '';
    let done = false;
    const finish = (save) => {
      if (done) return;
      done = true;
      const value = input.value.trim();
      input.remove();
      title.hidden = false;
      if (save && value && value !== n.title) {
        model.apply([{ op: 'node.update', id, set: { title: value } }], { label: 'Rename' }).catch(failed('It was not renamed'));
      }
      if (document.activeElement === document.body) viewport.focus({ preventScroll: true });
    };
    input.addEventListener('keydown', (event) => {
      event.stopPropagation();
      if (event.key === 'Enter') {
        event.preventDefault();
        finish(true);
        viewport.focus({ preventScroll: true });
      } else if (event.key === 'Escape') {
        event.preventDefault();
        finish(false);
        viewport.focus({ preventScroll: true });
      }
    });
    input.addEventListener('blur', () => finish(true));
    title.hidden = true;
    title.after(input);
    input.focus();
    input.select();
  }

  function placeAnchor(x, y) {
    anchor.style.left = `${Math.round(x)}px`;
    anchor.style.top = `${Math.round(y)}px`;
    return anchor;
  }

  // nodeMenu is a node's ⋯: its body's own items, then what every node has.
  function nodeMenu(id, at) {
    const item = frames.get(id);
    const n = model.node(id);
    if (!item || !n || !cockpit.has('menu')) return;
    const own = (call(item, 'menu') || []).filter(Boolean);
    const editable = !model.readOnly;
    const items = [
      ...own,
      own.length ? { separator: true } : null,
      n.proposed && editable ? { icon: '✓', label: 'Approve', detail: `Proposed by «${model.titleOf(n.created_by)}»`, run: () => approve(id) } : null,
      { icon: '⌖', label: 'Focus', hint: 'F', detail: 'At 100%, in the middle', run: () => focusMode(id) },
      editable ? { icon: '✎', label: 'Rename', run: () => rename(id) } : null,
      { icon: '⚙', label: 'Inspect', detail: 'Its settings, what waits for it', run: () => env.inspect?.({ node: id }) },
      editable ? { icon: '⧉', label: 'Duplicate', hint: '⌘D', run: () => { setSelection([id]); duplicateSelected(); } } : null,
      editable ? { separator: true } : null,
      editable && n.kind === 'agent' && n.runtime?.session
        ? { icon: '×', label: 'Delete the node', detail: 'Its session stays in the list', danger: true, run: () => removeNodes([id]) }
        : null,
      editable && n.kind === 'agent' && n.runtime?.session
        ? { icon: '×', label: 'Delete it and its session', danger: true, confirm: 'Delete the session too?', run: () => removeNodes([id], { session: false }) }
        : null,
      editable && !(n.kind === 'agent' && n.runtime?.session)
        ? { icon: '×', label: 'Delete', hint: '⌫', danger: true, run: () => removeNodes([id]) }
        : null,
    ].filter(Boolean);
    cockpit.use('menu').open(at, items, { compact: true });
  }

  function edgeMenu(id, at) {
    const e = model.edge(id);
    if (!e || !cockpit.has('menu')) return;
    const editable = !model.readOnly;
    const mode = (m) => () => model.apply([{ op: 'edge.update', id, set: { mode: m } }], { label: `Edge: ${m}` }).catch(failed('The edge was not changed'));
    cockpit.use('menu').open(at, [
      { icon: '⚙', label: 'Inspect', detail: 'Its template, its messages', run: () => env.inspect?.({ edge: id }) },
      editable && (e.mode || 'auto') !== 'auto' ? { icon: '→', label: 'Let messages through', run: mode('auto') } : null,
      editable && e.mode !== 'approve' ? { icon: '✋', label: 'Ask before each message', run: mode('approve') } : null,
      editable && e.mode !== 'off' ? { icon: '⏸', label: 'Turn it off', run: mode('off') } : null,
      editable ? { separator: true } : null,
      editable ? { icon: '×', label: 'Delete', hint: '⌫', danger: true, run: () => { setSelection([], { edge: id }); removeSelected(); } } : null,
    ].filter(Boolean), { compact: true });
  }

  // ---------------------------------------------------------------- pointers

  // track follows a drag of a pointer until it is let go: up runs when it
  // is, end in any case — let go, cancelled, or another gesture taking
  // over.
  function track(event, { move, up, end }) {
    endGesture();
    const g = new AbortController();
    gesture = g;
    g.signal.addEventListener('abort', () => cockpit.safely(() => end?.()));
    try {
      viewport.setPointerCapture(event.pointerId);
    } catch { /* gone already */ }
    const options = { signal: g.signal };
    viewport.addEventListener('pointermove', (e) => {
      if (e.pointerId === event.pointerId) cockpit.safely(move, e);
    }, options);
    const finish = (e, ok) => {
      if (e.pointerId !== event.pointerId) return;
      if (gesture === g) gesture = null;
      g.abort();
      if (ok) cockpit.safely(() => up?.(e));
    };
    viewport.addEventListener('pointerup', (e) => finish(e, true), options);
    viewport.addEventListener('pointercancel', (e) => finish(e, false), options);
  }

  function endGesture() {
    const g = gesture;
    gesture = null;
    g?.abort();
  }

  function beginPan(event) {
    event.preventDefault();
    const start = { x: event.clientX, y: event.clientY, vx: view.x, vy: view.y };
    viewport.dataset.panning = 'true';
    track(event, {
      move: (e) => setView({ x: start.vx + e.clientX - start.x, y: start.vy + e.clientY - start.y }),
      end: () => { delete viewport.dataset.panning; },
    });
  }

  function beginMarquee(event) {
    viewport.focus({ preventScroll: true });
    const additive = event.shiftKey;
    const base = additive ? [...selection] : [];
    const r = viewport.getBoundingClientRect();
    const from = worldAt(event.clientX, event.clientY);
    let moved = false;
    track(event, {
      move: (e) => {
        if (!moved && Math.hypot(e.clientX - event.clientX, e.clientY - event.clientY) < 4) return;
        moved = true;
        marquee.style.left = `${Math.min(e.clientX, event.clientX) - r.left}px`;
        marquee.style.top = `${Math.min(e.clientY, event.clientY) - r.top}px`;
        marquee.style.width = `${Math.abs(e.clientX - event.clientX)}px`;
        marquee.style.height = `${Math.abs(e.clientY - event.clientY)}px`;
        marquee.hidden = false;
        const to = worldAt(e.clientX, e.clientY);
        const box = { x: Math.min(from.x, to.x), y: Math.min(from.y, to.y), w: Math.abs(to.x - from.x), h: Math.abs(to.y - from.y) };
        const ids = new Set(base);
        for (const n of model.doc.nodes) if (overlaps(n, box)) ids.add(n.id);
        setSelection(ids);
      },
      up: () => {
        if (!moved && !additive) setSelection([]);
      },
      end: () => { marquee.hidden = true; },
    });
  }

  function beginDrag(event, id) {
    event.preventDefault();
    viewport.focus({ preventScroll: true });
    if (event.shiftKey) {
      pick(id, event);
      if (!selection.has(id)) return;
    } else if (!selection.has(id) || edgeSelected) {
      setSelection([id]);
    }
    budget.touch(id);
    if (model.readOnly) return;
    const ids = [...selection].filter((x) => model.node(x));
    const start = new Map(ids.map((x) => [x, { x: model.node(x).x, y: model.node(x).y }]));
    let moving = false;
    let last = null;
    let sentAt = 0;
    let timer = 0;
    // The places go one after the other, the last one last.
    let chain = Promise.resolve();
    const send = (places, final) => {
      const ops = [...places].map(([x, p]) => ({ op: 'node.update', id: x, set: { x: p.x, y: p.y } }));
      if (!final) {
        chain = chain.then(() => model.apply(ops, { undo: false })).catch(() => {});
        return chain;
      }
      const changed = ops.filter((op) => start.get(op.id).x !== op.set.x || start.get(op.id).y !== op.set.y);
      const inverse = changed.map((op) => ({ op: 'node.update', id: op.id, set: { ...start.get(op.id) } }));
      chain = chain
        .then(() => (changed.length ? model.apply(changed, { label: changed.length > 1 ? 'Move nodes' : 'Move', inverse }) : null))
        .catch(failed('The move was not saved'));
      return chain;
    };
    track(event, {
      move: (e) => {
        if (!moving && Math.hypot(e.clientX - event.clientX, e.clientY - event.clientY) < 3) return;
        if (!moving) {
          moving = true;
          for (const x of ids) model.dragging.add(x);
          viewport.dataset.dragging = 'true';
          cockpit.use('menu')?.close?.();
        }
        const dx = (e.clientX - event.clientX) / view.zoom;
        const dy = (e.clientY - event.clientY) / view.zoom;
        const places = new Map();
        for (const [x, p] of start) places.set(x, { x: snap(p.x + dx), y: snap(p.y + dy) });
        last = places;
        model.moveLocal(places);
        const now = performance.now();
        clearTimeout(timer);
        if (now - sentAt >= SEND_EVERY) {
          sentAt = now;
          send(places, false);
        } else {
          timer = setTimeout(() => {
            sentAt = performance.now();
            send(last, false);
          }, SEND_EVERY - (now - sentAt));
        }
      },
      up: () => {
        if (!moving && !event.shiftKey) setSelection([id]);
      },
      end: () => {
        clearTimeout(timer);
        delete viewport.dataset.dragging;
        if (!moving || !last) return;
        send(last, true).finally(() => {
          for (const x of ids) model.dragging.delete(x);
          sizeEdges();
        });
      },
    });
  }

  function beginResize(event, id) {
    event.preventDefault();
    event.stopPropagation();
    if (!selection.has(id) || selection.size > 1) setSelection([id]);
    if (model.readOnly) return;
    const n = model.node(id);
    const start = { w: n.w, h: n.h };
    let size = null;
    track(event, {
      move: (e) => {
        const w = Math.max(MIN_W, snap(start.w + (e.clientX - event.clientX) / view.zoom));
        const hh = Math.max(MIN_H, snap(start.h + (e.clientY - event.clientY) / view.zoom));
        if (!size && w === start.w && hh === start.h) return;
        if (!size) {
          model.dragging.add(id);
          viewport.dataset.resizing = 'true';
        }
        size = { w, h: hh };
        model.moveLocal(new Map([[id, size]]));
      },
      end: () => {
        delete viewport.dataset.resizing;
        if (!size) return;
        const changed = size.w !== start.w || size.h !== start.h;
        const done = changed
          ? model.apply([{ op: 'node.update', id, set: size }], { label: 'Resize', inverse: [{ op: 'node.update', id, set: start }] })
          : Promise.resolve();
        done.catch(failed('The size was not saved')).finally(() => {
          model.dragging.delete(id);
          sizeEdges();
        });
      },
    });
  }

  // Two fingers pinch and pan the board at once.
  const touches = new Map();
  function beginPinch() {
    endGesture();
    const [a, b] = [...touches.values()];
    const r = viewport.getBoundingClientRect();
    const start = { dist: Math.hypot(a.x - b.x, a.y - b.y) || 1, mid: { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 }, view: { ...view } };
    const wx = (start.mid.x - r.left - start.view.x) / start.view.zoom;
    const wy = (start.mid.y - r.top - start.view.y) / start.view.zoom;
    const g = new AbortController();
    gesture = g;
    viewport.addEventListener('pointermove', (e) => {
      if (!touches.has(e.pointerId) || touches.size < 2) return;
      touches.set(e.pointerId, { x: e.clientX, y: e.clientY });
      const [p, q] = [...touches.values()];
      const dist = Math.hypot(p.x - q.x, p.y - q.y) || 1;
      const mid = { x: (p.x + q.x) / 2, y: (p.y + q.y) / 2 };
      const z = clampZoom((start.view.zoom * dist) / start.dist);
      setView({ zoom: z, x: mid.x - r.left - wx * z, y: mid.y - r.top - wy * z });
    }, { signal: g.signal });
    const done = (e) => {
      touches.delete(e.pointerId);
      if (touches.size >= 2) return;
      if (gesture === g) gesture = null;
      g.abort();
    };
    viewport.addEventListener('pointerup', done, { signal: g.signal });
    viewport.addEventListener('pointercancel', done, { signal: g.signal });
  }

  on(viewport, 'pointermove', (event) => {
    pointer = { x: event.clientX, y: event.clientY };
    if (event.pointerType === 'touch' && touches.has(event.pointerId)) touches.set(event.pointerId, { x: event.clientX, y: event.clientY });
  });
  on(viewport, 'pointerup', (event) => touches.delete(event.pointerId));
  on(viewport, 'pointercancel', (event) => touches.delete(event.pointerId));
  on(viewport, 'pointerleave', () => { pointer = null; });

  on(viewport, 'pointerdown', (event) => {
    pointer = { x: event.clientX, y: event.clientY };
    if (event.pointerType === 'touch') {
      touches.set(event.pointerId, { x: event.clientX, y: event.clientY });
      if (touches.size === 2) {
        beginPinch();
        return;
      }
      if (touches.size > 2) return;
    }
    const target = event.target instanceof Element ? event.target : null;
    if (!target) return;
    if (event.button === 1 || (event.button === 0 && spaceHeld)) {
      beginPan(event);
      return;
    }
    if (event.button !== 0) return;
    const port = target.closest('.cv-port');
    if (port) {
      if (!model.readOnly) edges.beginConnect(event, port);
      return;
    }
    const el = target.closest('.cv-node');
    if (!el) {
      if (target.closest('.cv-edge')) return;
      if (event.pointerType === 'touch') beginPan(event);
      else beginMarquee(event);
      return;
    }
    const id = el.dataset.id;
    if (!frames.has(id)) return;
    if (target.closest('.cv-resize')) {
      beginResize(event, id);
      return;
    }
    if (target.closest('.cv-head, .cv-foot') && !target.closest('button, a, input, textarea, select')) {
      beginDrag(event, id);
      return;
    }
    if (target.closest('.cv-head button, .cv-proposal, .cv-pending')) {
      if (!selection.has(id)) setSelection([id]);
      return;
    }
    // The body: the node is selected, and the body has the pointer.
    pick(id, event);
  });
  // The middle button pans; it must not scroll the page by itself.
  on(viewport, 'mousedown', (event) => {
    if (event.button === 1) event.preventDefault();
  });

  const delta = (event) => {
    const k = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? viewport.clientHeight : 1;
    let x = event.deltaX * k;
    let y = event.deltaY * k;
    if (event.shiftKey && !x) [x, y] = [y, 0];
    return { x, y };
  };

  // scrollsInside says whether a wheel scrolls something in the node it is
  // over, rather than the board: in a node selected or in use, a part that
  // can still scroll that way — or its terminal, which scrolls its own.
  function scrollsInside(event) {
    const el = event.target instanceof Element ? event.target : null;
    const frameEl = el?.closest('.cv-node');
    if (!frameEl) return false;
    const id = frameEl.dataset.id;
    if (!selection.has(id) && entered !== id) return false;
    if (el.closest('.term-mount')) return true;
    const d = delta(event);
    for (let x = el; x && x !== frameEl; x = x.parentElement) {
      if (!(x.dataset?.scroll || x.tagName === 'TEXTAREA' || x.tagName === 'PRE')) continue;
      if (d.y > 0 && x.scrollTop + x.clientHeight < x.scrollHeight - 1) return true;
      if (d.y < 0 && x.scrollTop > 0) return true;
      if (d.x > 0 && x.scrollLeft + x.clientWidth < x.scrollWidth - 1) return true;
      if (d.x < 0 && x.scrollLeft > 0) return true;
    }
    return false;
  }

  on(viewport, 'wheel', (event) => {
    if (event.ctrlKey || event.metaKey) {
      event.preventDefault();
      event.stopPropagation();
      const d = Math.max(-60, Math.min(60, delta(event).y));
      zoomAt(view.zoom * Math.exp(-d * 0.006), event.clientX, event.clientY);
      return;
    }
    if (scrollsInside(event)) return;
    event.preventDefault();
    event.stopPropagation();
    const d = delta(event);
    cancelAnimationFrame(animation);
    setView({ x: view.x - d.x, y: view.y - d.y });
  }, { passive: false, capture: true });

  // Safari's pinch on a trackpad.
  let gestureZoom = 1;
  on(viewport, 'gesturestart', (event) => {
    event.preventDefault();
    gestureZoom = view.zoom;
  });
  on(viewport, 'gesturechange', (event) => {
    event.preventDefault();
    zoomAt(gestureZoom * event.scale, event.clientX, event.clientY);
  });
  on(viewport, 'gestureend', (event) => event.preventDefault());

  // Space held: a drag pans.
  const releaseSpace = () => {
    spaceHeld = false;
    delete viewport.dataset.space;
  };
  on(window, 'keydown', (event) => {
    if (event.code !== 'Space' || !pageShown || event.metaKey || event.ctrlKey || event.altKey) return;
    const target = event.target instanceof Element ? event.target : null;
    if (typing(target) || target?.closest('[data-keys="own"], button, a')) return;
    if (target && target !== document.body && !viewport.contains(target)) return;
    event.preventDefault();
    if (!spaceHeld) {
      spaceHeld = true;
      viewport.dataset.space = 'true';
    }
  });
  on(window, 'keyup', (event) => {
    if (event.code === 'Space') releaseSpace();
  });
  on(window, 'blur', releaseSpace);

  on(viewport, 'contextmenu', (event) => {
    const target = event.target instanceof Element ? event.target : null;
    if (!target || target.closest('.term-mount, textarea, input')) return;
    event.preventDefault();
    placeAnchor(event.clientX, event.clientY);
    const el = target.closest('.cv-node');
    if (el) {
      const id = el.dataset.id;
      if (!selection.has(id)) setSelection([id]);
      nodeMenu(id, anchor);
      return;
    }
    const edgeEl = target.closest('.cv-edge');
    if (edgeEl) {
      setSelection([], { edge: edgeEl.dataset.id });
      edgeMenu(edgeEl.dataset.id, anchor);
      return;
    }
    if (!model.readOnly) env.addMenu?.(anchor, worldAt(event.clientX, event.clientY));
  });

  on(viewport, 'dblclick', (event) => {
    const target = event.target instanceof Element ? event.target : null;
    if (!target) return;
    const el = target.closest('.cv-node');
    if (!el) {
      if (target.closest('.cv-edge') || model.readOnly) return;
      placeAnchor(event.clientX, event.clientY);
      env.addMenu?.(anchor, worldAt(event.clientX, event.clientY));
      return;
    }
    const id = el.dataset.id;
    if (target.closest('.cv-title')) {
      event.preventDefault();
      rename(id);
    } else if (target.closest('.cv-head') && !target.closest('button, input')) {
      focusMode(id);
    } else if (model.node(id)?.kind === 'terminal' && !target.closest('.term-mount, button, input, textarea, a')) {
      // A terminal away from its size, which the pointer does not reach:
      // shown at it, with the keys.
      focusMode(id);
      enter(id);
    }
  });

  on(layer, 'click', (event) => {
    const target = event.target instanceof Element ? event.target : null;
    const el = target?.closest('.cv-node');
    if (!el) return;
    const id = el.dataset.id;
    const more = target.closest('.cv-more');
    if (more) {
      event.stopPropagation();
      if (cockpit.has('menu') && cockpit.use('menu').anchor?.() === more) cockpit.use('menu').close();
      else nodeMenu(id, more);
      return;
    }
    if (target.closest('.cv-pending')) {
      env.inspect?.({ node: id, pending: true });
      return;
    }
    const act = target.closest('.cv-proposal [data-act]');
    if (act) {
      if (act.dataset.act === 'approve') approve(id);
      else discard(id);
    }
  });

  on(layer, 'focusin', (event) => {
    const el = event.target.closest?.('.cv-node');
    if (el && el.querySelector('.cv-body')?.contains(event.target)) setEntered(el.dataset.id);
  });
  on(layer, 'focusout', (event) => {
    const el = event.target.closest?.('.cv-node');
    if (!el || el.contains(event.relatedTarget)) return;
    if (entered === el.dataset.id) setEntered(null);
  });

  // ---------------------------------------------------------------- the model's news

  const offs = [
    model.on('change', (what, data) => {
      if (what === 'geometry') {
        moved(data || []);
      } else if (what === 'runtime') {
        const item = frames.get(data?.node);
        const n = model.node(data?.node);
        if (!item || !n) return;
        draw(item, n);
        call(item, 'update', n);
        call(item, 'runtime');
        checkEntered();
      } else if (what === 'load' || what === 'ops') {
        sync();
        if (what === 'load') placeOnce();
      }
    }),
    model.on('status', (id) => {
      const item = frames.get(id);
      if (!item) return;
      draw(item);
      call(item, 'status');
      checkEntered();
    }),
    model.on('terminal', (id) => {
      const item = frames.get(id);
      if (!item) return;
      draw(item);
      call(item, 'terminal');
    }),
    model.on('message', (m) => {
      edges.flow(m);
      if (m.edge) edges.redraw(m.edge);
      const item = frames.get(m.to?.node);
      if (!item) return;
      draw(item);
      // Something typed into a shell: the node says who.
      const from = m.from?.node && model.node(m.from.node);
      if (m.state === 'delivered' && from && model.node(m.to.node)?.kind === 'terminal') item.frame.showTyping(from.title || from.id);
    }),
  ];

  // placeOnce places the view the first time the board can be seen: as it
  // was left, or fitted to the nodes.
  function placeOnce() {
    if (placed || model.loading || disposed) return;
    const r = viewport.getBoundingClientRect();
    if (r.width < 20 || r.height < 20) return;
    const saved = env.savedView?.();
    if (saved && [saved.x, saved.y, saved.zoom].every(Number.isFinite) && saved.zoom > 0) {
      placed = true;
      setView({ x: saved.x, y: saved.y, zoom: saved.zoom });
    } else {
      placed = fit(null, { animate: false });
    }
  }

  const resizeObserver = new ResizeObserver(() => {
    if (!placed) placeOnce();
    scheduleLod();
  });
  resizeObserver.observe(viewport);

  // Busy nodes count their seconds.
  const ticker = setInterval(() => {
    for (const item of frames.values()) if (item.frame.busy()) item.frame.tick();
  }, 1000);

  edges.reset();
  applyView();
  sync();

  return {
    element: viewport,
    sync,
    fit: (ids, options) => fit(ids, options),
    zoom100,
    zoomBy,
    zoom: () => view.zoom,
    view: () => ({ ...view }),
    center,
    focusMode,
    select: (ids) => setSelection(ids || []),
    selectEdge: (id) => setSelection([], { edge: id }),
    selected: () => [...selection],
    selectedEdge: () => edgeSelected,
    clear: () => setSelection([]),
    enter,
    exit,
    entered: () => entered,
    next,
    add,
    removeSelected,
    removeNodes,
    duplicateSelected,
    rename,
    nodeMenu,
    // pointerWorld is the point of the world under the pointer, or the
    // middle of the board.
    pointerWorld() {
      const r = viewport.getBoundingClientRect();
      if (pointer && pointer.x >= r.left && pointer.x <= r.right && pointer.y >= r.top && pointer.y <= r.bottom) {
        return { at: worldAt(pointer.x, pointer.y), client: { ...pointer } };
      }
      const [x, y] = middle();
      return { at: null, client: { x, y } };
    },
    placeAnchor,
    focus: () => viewport.focus({ preventScroll: true }),
    contains: (el) => viewport.contains(el),
    shown() {
      pageShown = true;
      placeOnce();
      scheduleLod(0);
    },
    hidden() {
      pageShown = false;
      releaseSpace();
      endGesture();
      lodNow();
    },
    dispose() {
      if (disposed) return;
      flushView();
      disposed = true;
      stop.abort();
      endGesture();
      cancelAnimationFrame(animation);
      clearInterval(ticker);
      clearTimeout(lodTimer);
      resizeObserver.disconnect();
      for (const off of offs) off();
      for (const id of [...frames.keys()]) dropFrame(id);
      edges.dispose();
      viewport.remove();
    },
  };
}
