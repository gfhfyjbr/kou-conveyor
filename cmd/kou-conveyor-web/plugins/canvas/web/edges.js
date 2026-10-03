// The edges of the canvas: one SVG in the world, under the nodes. An edge
// is a curve from an output port, on a node's right edge, to an input, on
// another's left; a dot runs along it as a message goes, and its label
// counts what went since the page opened. Dragging from a port draws a new
// edge to the port it is let go on — a node will do, its first port of the
// kind — or, let go where nothing is, asks what to add there, wired.
import { curve, portPoint, portsOf } from './geometry.js';

const NS = 'http://www.w3.org/2000/svg';

// s builds an SVG element: the kernel's h makes HTML ones.
function s(tag, attrs = {}, ...children) {
  const el = document.createElementNS(NS, tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value == null || value === false) continue;
    if (key === 'class') el.setAttribute('class', value);
    else if (key === 'data') for (const [name, v] of Object.entries(value)) { if (v != null) el.dataset[name] = v; }
    else if (key.startsWith('on')) el.addEventListener(key.slice(2), value);
    else el.setAttribute(key, String(value));
  }
  for (const child of children) if (child) el.append(child);
  return el;
}

export function createEdges(env) {
  const { cockpit } = env;
  const root = s('svg', { class: 'cv-edges', width: '1', height: '1', 'aria-hidden': 'true' },
    s('defs', {},
      s('marker', { id: 'cv-arrow', viewBox: '0 0 10 10', refX: '9', refY: '5', markerWidth: '7', markerHeight: '7', orient: 'auto-start-reverse', markerUnits: 'userSpaceOnUse' },
        s('path', { d: 'M0,1 L9,5 L0,9 z', class: 'cv-arrow' }))));
  const layer = s('g', { class: 'cv-edge-layer' });
  const flows = s('g', { class: 'cv-flows' });
  const ghost = s('path', { class: 'cv-ghost', d: '' });
  root.append(layer, flows, ghost);

  const items = new Map(); // edge → { g, hit, line, label, text, d }
  const counts = new Map(); // edge → messages delivered along it, as the page saw
  const counted = new Set();
  let selected = null;

  const model = () => env.model();

  function count(m) {
    if (!m?.edge || m.state !== 'delivered' || counted.has(m.id)) return false;
    counted.add(m.id);
    counts.set(m.edge, (counts.get(m.edge) || 0) + 1);
    return true;
  }

  function reset() {
    counts.clear();
    counted.clear();
    for (const m of model()?.messages || []) count(m);
  }

  // ends are the points an edge runs between, as its nodes stand now.
  function ends(e) {
    const from = env.nodeOf(e.from.node);
    const to = env.nodeOf(e.to.node);
    if (!from || !to) return null;
    return [portPoint(from, 'out', e.from.port, env.kinds), portPoint(to, 'in', e.to.port, env.kinds)];
  }

  function create(e) {
    const hit = s('path', { class: 'cv-edge-hit' });
    const line = s('path', { class: 'cv-edge-line', 'marker-end': 'url(#cv-arrow)' });
    const text = s('text', { class: 'cv-edge-text', 'text-anchor': 'middle', 'dominant-baseline': 'central' });
    const box = s('rect', { class: 'cv-edge-box', rx: '0', ry: '0' });
    const label = s('g', { class: 'cv-edge-label' }, box, text);
    const g = s('g', { class: 'cv-edge', data: { id: e.id } }, hit, line, label);
    g.addEventListener('pointerdown', (event) => {
      if (event.button !== 0) return;
      event.stopPropagation();
      env.onSelect(e.id, event);
    });
    g.addEventListener('dblclick', (event) => {
      event.stopPropagation();
      env.onOpen?.(e.id);
    });
    const item = { g, hit, line, label, text, box, d: '' };
    items.set(e.id, item);
    layer.append(g);
    return item;
  }

  function draw(e, item = items.get(e.id)) {
    const points = ends(e);
    if (!points) {
      item.g.hidden = true;
      return;
    }
    item.g.hidden = false;
    const d = curve(points[0], points[1]);
    if (d !== item.d) {
      item.d = d;
      item.hit.setAttribute('d', d);
      item.line.setAttribute('d', d);
    }
    const m = model();
    const waiting = m ? m.queue.filter((q) => q.edge === e.id && q.state === 'awaiting_approval').length : 0;
    const n = counts.get(e.id) || 0;
    const words = [];
    if (e.mode === 'off') words.push('off');
    if (waiting) words.push(`${waiting} to approve`);
    else if (e.mode === 'approve') words.push('approve');
    if (n) words.push(String(n));
    const label = words.join(' · ');
    item.label.style.display = label ? '' : 'none';
    if (label) {
      if (item.text.textContent !== label) item.text.textContent = label;
      const mid = midpoint(item.line, points);
      const width = 8 + label.length * 6.2;
      item.box.setAttribute('x', String(-width / 2));
      item.box.setAttribute('y', '-8');
      item.box.setAttribute('width', String(width));
      item.box.setAttribute('height', '16');
      item.label.setAttribute('transform', `translate(${mid.x},${mid.y})`);
    }
    item.g.dataset.mode = e.mode || 'auto';
    item.g.dataset.waiting = waiting ? 'true' : '';
    item.g.dataset.selected = selected === e.id ? 'true' : '';
  }

  function midpoint(path, points) {
    try {
      const length = path.getTotalLength();
      if (length > 0) return path.getPointAtLength(length / 2);
    } catch { /* not drawn yet */ }
    return { x: (points[0].x + points[1].x) / 2, y: (points[0].y + points[1].y) / 2 };
  }

  // sync draws the edges as the document has them.
  function sync() {
    const m = model();
    const list = m?.doc.edges || [];
    const want = new Set(list.map((e) => e.id));
    for (const [id, item] of items) {
      if (!want.has(id)) {
        item.g.remove();
        items.delete(id);
      }
    }
    for (const e of list) draw(e, items.get(e.id) || create(e));
    if (selected && !want.has(selected)) selected = null;
  }

  // move has the edges of nodes that move follow them.
  function move(ids) {
    const set = new Set(ids);
    for (const e of model()?.doc.edges || []) {
      if (set.has(e.from.node) || set.has(e.to.node)) draw(e);
    }
  }

  function select(id) {
    selected = id || null;
    for (const [eid, item] of items) item.g.dataset.selected = eid === selected ? 'true' : '';
  }

  // flow runs a dot along the edge a message went by.
  function flow(message) {
    if (!count(message)) return;
    const e = model()?.edge(message.edge);
    const item = e && items.get(e.id);
    if (!item || item.g.hidden) return;
    draw(e, item);
    if (matchMedia('(prefers-reduced-motion: reduce)').matches) return;
    let length = 0;
    try {
      length = item.line.getTotalLength();
    } catch {
      return;
    }
    if (!length) return;
    const dot = s('circle', { class: 'cv-flow', r: '4' });
    flows.append(dot);
    const start = performance.now();
    const duration = Math.min(900, 320 + length / 2);
    const step = (now) => {
      const t = Math.min(1, (now - start) / duration);
      const eased = 1 - (1 - t) ** 3;
      const p = item.line.getPointAtLength(eased * length);
      dot.setAttribute('cx', String(p.x));
      dot.setAttribute('cy', String(p.y));
      if (t < 1 && dot.isConnected) requestAnimationFrame(step);
      else dot.remove();
    };
    requestAnimationFrame(step);
  }

  // ---------------------------------------------------------------- drawing a new edge

  let connecting = null;

  // beginConnect follows a drag from a port until it is let go.
  function beginConnect(event, port) {
    const m = model();
    const from = { node: port.dataset.node, port: port.dataset.port, dir: port.dataset.dir };
    const node = m?.node(from.node);
    if (!node || m.readOnly) return;
    event.preventDefault();
    event.stopPropagation();
    const start = portPoint(node, from.dir, from.port, env.kinds);
    const wanted = from.dir === 'out' ? 'in' : 'out';
    env.world().dataset.connecting = wanted;
    for (const el of env.world().querySelectorAll(`.cv-port[data-dir="${wanted}"]`)) {
      if (el.dataset.node !== from.node) el.dataset.candidate = 'true';
    }
    const stop = new AbortController();
    const { signal } = stop;
    let target = null;
    connecting = stop;
    const target_ = env.viewport();
    try {
      target_.setPointerCapture(event.pointerId);
    } catch { /* gone */ }

    const pick = (clientX, clientY) => {
      const el = document.elementFromPoint(clientX, clientY);
      let found = el?.closest?.(`.cv-port[data-dir="${wanted}"]`);
      if (found && found.dataset.node === from.node) found = null;
      if (!found) {
        const frame = el?.closest?.('.cv-node');
        const other = frame && frame.dataset.id !== from.node ? m.node(frame.dataset.id) : null;
        if (other) {
          const ports = portsOf(other, env.kinds)[wanted === 'in' ? 'inputs' : 'outputs'];
          if (ports.length) found = frame.querySelector(`.cv-port[data-dir="${wanted}"][data-port="${CSS.escape(ports[0])}"]`);
        }
      }
      return found || null;
    };

    const draw = (clientX, clientY) => {
      const at = env.worldAt(clientX, clientY);
      const next = pick(clientX, clientY);
      if (next !== target) {
        if (target) delete target.dataset.over;
        target = next;
        if (target) target.dataset.over = 'true';
      }
      let end = at;
      if (target) {
        const other = m.node(target.dataset.node);
        if (other) end = portPoint(other, wanted, target.dataset.port, env.kinds);
      }
      ghost.setAttribute('d', from.dir === 'out' ? curve(start, end) : curve(end, start));
    };
    draw(event.clientX, event.clientY);

    const finish = (commit, clientX, clientY) => {
      if (signal.aborted) return;
      stop.abort();
      connecting = null;
      ghost.setAttribute('d', '');
      delete env.world().dataset.connecting;
      for (const el of env.world().querySelectorAll('.cv-port[data-candidate], .cv-port[data-over]')) {
        delete el.dataset.candidate;
        delete el.dataset.over;
      }
      if (!commit) return;
      if (target) {
        const other = { node: target.dataset.node, port: target.dataset.port };
        const a = from.dir === 'out' ? { node: from.node, port: from.port } : other;
        const b = from.dir === 'out' ? other : { node: from.node, port: from.port };
        env.onConnect(a, b);
      } else if (clientX !== undefined) {
        env.onDropEmpty(env.worldAt(clientX, clientY), from, { clientX, clientY });
      }
    };
    target_.addEventListener('pointermove', (e) => cockpit.safely(() => draw(e.clientX, e.clientY)), { signal });
    target_.addEventListener('pointerup', (e) => finish(true, e.clientX, e.clientY), { signal });
    target_.addEventListener('pointercancel', () => finish(false), { signal });
    window.addEventListener('keydown', (e) => {
      if (e.key !== 'Escape') return;
      e.preventDefault();
      e.stopPropagation();
      finish(false);
    }, { capture: true, signal });
  }

  return {
    element: root,
    sync,
    move,
    select,
    selected: () => selected,
    flow,
    reset,
    beginConnect,
    redraw: (id) => {
      const e = model()?.edge(id);
      if (e && items.has(id)) draw(e);
    },
    dispose() {
      connecting?.abort();
      root.remove();
    },
  };
}
