// The canvas in view, as the page holds it: its document, what runs for its
// nodes — their statuses, their shells — and the messages that wait and
// those that went, followed over the canvas's events (the engine's hub.go:
// a snapshot first, then each change). It changes the canvas by batches of
// operations, which apply whole or not at all, keeps what the user did to
// undo it (⌘Z) and do it again (⌘⇧Z), and tells whoever listens what
// changed:
//
//   change (what)      the document: 'load', 'ops', 'geometry', 'runtime'
//   status (node)      a node's status
//   message (m)        a message came, went, or waits
//   output (event)     a node gave an output
//   agent (event)      an agent's transcript grew
//   terminal (node)    a node's shell changed
//   activity ()        the Activity list grew
//   notice (event)     something to say
//   stream (state)     the events: connecting, live, reconnecting, closed
//   deleted ()         the canvas was deleted
//
// A canvas the page has not saved yet — one just made from New session —
// is not on the server: its first batch makes it ("create").

const WAITING = new Set(['pending', 'awaiting_approval']);
const KEEP_MESSAGES = 200;
const KEEP_ACTIVITY = 200;
const KEEP_FEED = 200;

export function createModel(cockpit, ws, id, { title = '' } = {}) {
  const listeners = new Map();
  const blank = () => ({ version: 1, id, title, live: true, rev: 0, settings: {}, nodes: [], edges: [] });
  const m = {
    ws, id,
    doc: blank(),
    exists: false,
    loading: true,
    failed: '',
    readOnly: false,
    deleted: false,
    stream: 'idle',
    status: new Map(),
    terminals: new Map(),
    hooks: new Map(),
    queue: [],
    messages: [],
    activity: [],
    feeds: new Map(),
    outputs: new Map(),
    logs: new Map(),
    // dragging: the nodes the user moves now, whose places the server's
    // echoes must not take back.
    dragging: new Set(),
    undoStack: [],
    redoStack: [],
  };
  let source = null;
  let closed = false;

  function on(kind, fn) {
    if (!listeners.has(kind)) listeners.set(kind, new Set());
    listeners.get(kind).add(fn);
    return () => listeners.get(kind)?.delete(fn);
  }
  function emit(kind, ...args) {
    for (const fn of [...(listeners.get(kind) || [])]) cockpit.safely(fn, ...args);
  }

  const path = (rest = '') => cockpit.wsPath(ws, `/canvases/${encodeURIComponent(id)}${rest}`);
  const api = (rest, options) => cockpit.api(path(rest), options);

  // ---------------------------------------------------------------- reading

  const node = (nid) => m.doc.nodes.find((n) => n.id === nid) || null;
  const edge = (eid) => m.doc.edges.find((e) => e.id === eid) || null;
  const pendingFor = (nid) => m.queue.filter((q) => q.to?.node === nid).length;
  const awaitingFor = (nid) => m.queue.filter((q) => q.to?.node === nid && q.state === 'awaiting_approval');
  const titleOf = (nid) => node(nid)?.title || nid;

  async function load() {
    m.loading = true;
    emit('change', 'loading');
    let data;
    try {
      data = await api('');
    } catch (error) {
      if (closed) return;
      m.loading = false;
      if (error.status === 404) {
        // Not saved yet: a fresh canvas.
        m.exists = false;
        m.doc = blank();
        emit('change', 'load');
        return;
      }
      m.failed = error.message || 'The canvas did not load';
      emit('change', 'load');
      return;
    }
    if (closed) return;
    m.exists = true;
    snapshot(data);
    connect();
  }

  function snapshot(data) {
    m.loading = false;
    m.failed = '';
    m.doc = data.doc || blank();
    m.doc.nodes ||= [];
    m.doc.edges ||= [];
    m.readOnly = !!data.read_only;
    m.status = new Map(Object.entries(data.status || {}));
    m.terminals = new Map(Object.entries(data.terminals || {}));
    m.hooks = new Map(Object.entries(data.hooks || {}));
    m.queue = (data.queue || []).filter((q) => WAITING.has(q.state));
    const known = new Map(m.messages.map((x) => [x.id, x]));
    for (const x of data.messages || []) known.set(x.id, x);
    m.messages = [...known.values()].sort((a, b) => Date.parse(a.at) - Date.parse(b.at)).slice(-KEEP_MESSAGES);
    emit('change', 'load');
  }

  function connect() {
    if (source || closed || !m.exists) return;
    setStream('connecting');
    const s = new EventSource(path('/events'));
    source = s;
    s.onopen = () => { if (source === s) setStream('live'); };
    s.onerror = () => {
      if (source !== s) return;
      // EventSource tries again by itself, from the last event it had.
      setStream(s.readyState === EventSource.CLOSED ? 'closed' : 'reconnecting');
      if (s.readyState === EventSource.CLOSED) {
        source = null;
        if (!closed && !m.deleted) setTimeout(() => { if (!closed && !source) load(); }, 2000);
      }
    };
    s.onmessage = (message) => {
      if (source !== s) return;
      let event;
      try {
        event = JSON.parse(message.data);
      } catch {
        return;
      }
      handle(event);
    };
  }

  function setStream(state) {
    if (m.stream === state) return;
    m.stream = state;
    emit('stream', state);
  }

  function close() {
    closed = true;
    source?.close();
    source = null;
    listeners.clear();
  }

  // ---------------------------------------------------------------- events

  function handle(event) {
    switch (event.type) {
      case 'snapshot':
        snapshot(event);
        return;
      case 'ops':
        applyChanges(event.changes || []);
        if (event.rev) m.doc.rev = event.rev;
        if (event.actor?.kind === 'node') noteOps(event);
        emit('change', 'ops', event);
        return;
      case 'status': {
        m.status.set(event.node, event.status);
        emit('status', event.node);
        return;
      }
      case 'runtime': {
        const n = node(event.node);
        if (n) n.runtime = event.runtime || {};
        emit('change', 'runtime', event);
        return;
      }
      case 'terminal':
        m.terminals.set(event.node, event.terminal);
        emit('terminal', event.node);
        return;
      case 'message':
        takeMessage(event.message);
        return;
      case 'output':
        m.outputs.set(event.node, event);
        remember({ at: event.at, kind: 'output', node: event.node, text: `«${titleOf(event.node)}» ${event.port && event.port !== 'out' ? `put out on ${event.port}` : 'put out'}: ${event.title || event.preview || ''}` });
        emit('output', event);
        return;
      case 'agent': {
        const list = m.feeds.get(event.node);
        if (list && event.entry) {
          const at = list.findIndex((e) => e.id === event.entry.id);
          if (at >= 0) list[at] = event.entry;
          else list.push(event.entry);
          if (list.length > KEEP_FEED) list.splice(0, list.length - KEEP_FEED);
        }
        emit('agent', event);
        return;
      }
      case 'log': {
        const list = m.logs.get(event.node) || [];
        list.push(event);
        if (list.length > 20) list.splice(0, list.length - 20);
        m.logs.set(event.node, list);
        emit('output', { ...event, log: true });
        return;
      }
      case 'notice':
        remember({ at: event.at, kind: 'notice', level: event.level, node: event.node, edge: event.edge, text: event.text });
        emit('notice', event);
        return;
      case 'deleted':
        m.deleted = true;
        source?.close();
        source = null;
        setStream('closed');
        emit('deleted');
        return;
    }
  }

  function applyChanges(changes) {
    for (const change of changes) {
      switch (change.op) {
        case 'node.add':
        case 'node.update': {
          const next = change.node;
          if (!next) break;
          const at = m.doc.nodes.findIndex((n) => n.id === next.id);
          if (at < 0) {
            m.doc.nodes.push(next);
            break;
          }
          const current = m.doc.nodes[at];
          if (m.dragging.has(next.id)) Object.assign(next, { x: current.x, y: current.y, w: current.w, h: current.h });
          m.doc.nodes[at] = next;
          break;
        }
        case 'node.remove':
          m.doc.nodes = m.doc.nodes.filter((n) => n.id !== change.id);
          m.doc.edges = m.doc.edges.filter((e) => e.from.node !== change.id && e.to.node !== change.id);
          m.status.delete(change.id);
          m.terminals.delete(change.id);
          m.queue = m.queue.filter((q) => q.to?.node !== change.id);
          break;
        case 'edge.add':
        case 'edge.update': {
          const next = change.edge;
          if (!next) break;
          const at = m.doc.edges.findIndex((e) => e.id === next.id);
          if (at < 0) m.doc.edges.push(next);
          else m.doc.edges[at] = next;
          break;
        }
        case 'edge.remove':
          m.doc.edges = m.doc.edges.filter((e) => e.id !== change.id);
          break;
        case 'canvas.update':
          if (change.canvas) Object.assign(m.doc, { title: change.canvas.title, live: change.canvas.live, settings: change.canvas.settings || m.doc.settings });
          break;
      }
    }
  }

  // noteOps says in Activity what an agent did to the canvas.
  function noteOps(event) {
    const who = titleOf(event.actor.id);
    for (const change of event.changes || []) {
      let text = '';
      switch (change.op) {
        case 'node.add': text = `«${who}» made «${change.node?.title}»`; break;
        case 'node.remove': text = `«${who}» removed a node`; break;
        case 'edge.add': text = `«${who}» wired «${titleOf(change.edge?.from?.node)}» to «${titleOf(change.edge?.to?.node)}»`; break;
        case 'edge.remove': text = `«${who}» took an edge away`; break;
        case 'canvas.update': text = `«${who}» changed the canvas`; break;
        default: continue;
      }
      remember({ at: new Date().toISOString(), kind: 'ops', node: event.actor.id, text });
    }
  }

  function takeMessage(x) {
    if (!x?.id) return;
    const at = m.messages.findIndex((y) => y.id === x.id);
    if (at >= 0) m.messages[at] = x;
    else {
      m.messages.push(x);
      if (m.messages.length > KEEP_MESSAGES) m.messages.splice(0, m.messages.length - KEEP_MESSAGES);
    }
    const waiting = m.queue.findIndex((y) => y.id === x.id);
    if (WAITING.has(x.state)) {
      if (waiting >= 0) m.queue[waiting] = x;
      else m.queue.push(x);
    } else if (waiting >= 0) {
      m.queue.splice(waiting, 1);
    }
    if (x.state === 'delivered' || x.state === 'dropped' || x.state === 'awaiting_approval') {
      const from = x.from?.node && x.from.node !== x.to?.node ? `«${titleOf(x.from.node)}» → ` : '';
      const what = x.state === 'delivered' ? 'delivered' : x.state === 'dropped' ? `dropped${x.reason ? `: ${x.reason}` : ''}` : 'waits for approval';
      remember({ at: x.delivered_at || x.at, kind: 'message', level: x.state === 'dropped' ? 'warn' : '', node: x.to?.node, edge: x.edge, text: `${from}«${titleOf(x.to?.node)}» ${what}`, message: x.id });
    }
    emit('message', x);
  }

  function remember(item) {
    m.activity.push(item);
    if (m.activity.length > KEEP_ACTIVITY) m.activity.splice(0, m.activity.length - KEEP_ACTIVITY);
    emit('activity');
  }

  // ---------------------------------------------------------------- changing

  // apply sends a batch. The user's batches are kept to undo; inverse,
  // when given, is what undoes it, else it is worked out from the
  // document as it is before the batch.
  async function apply(ops, { undo = true, label = '', inverse = null } = {}) {
    if (m.readOnly) throw new Error('This canvas was made by a newer kou-conveyor: it is read only here.');
    const before = undo && !inverse ? snapshotOf(ops) : null;
    const body = { ops };
    if (!m.exists) Object.assign(body, { create: true, title: m.doc.title || '' });
    const result = await api('/ops', { method: 'POST', body });
    if (!m.exists) {
      m.exists = true;
      // The canvas is on the server now: its events say the rest.
      m.doc.rev = result?.rev || 0;
      load();
      emit('created');
    }
    if (undo) {
      const back = inverse || inverseOf(ops, result, before);
      if (back.length) {
        m.undoStack.push({ label, ops: back, redo: redoOf(ops, result) });
        if (m.undoStack.length > 100) m.undoStack.shift();
        m.redoStack = [];
      }
    }
    return result;
  }

  // snapshotOf keeps what a batch is about to change.
  function snapshotOf(ops) {
    const nodes = new Map();
    const edges = new Map();
    for (const op of ops) {
      if (op.id && node(op.id)) nodes.set(op.id, structuredClone(node(op.id)));
      if (op.id && edge(op.id)) edges.set(op.id, structuredClone(edge(op.id)));
      if (op.op === 'node.remove') {
        for (const e of m.doc.edges) if (e.from.node === op.id || e.to.node === op.id) edges.set(e.id, structuredClone(e));
      }
    }
    return { nodes, edges, canvas: { title: m.doc.title, live: m.doc.live } };
  }

  function inverseOf(ops, result, before) {
    const back = [];
    ops.forEach((op, index) => {
      const made = result?.ids?.[String(index)];
      switch (op.op) {
        case 'node.add':
          if (made) back.unshift({ op: 'node.remove', id: made });
          break;
        case 'node.remove':
          back.unshift({ op: 'node.add', node: { id: op.id, restore: true } });
          break;
        case 'node.update': {
          const was = before.nodes.get(op.id);
          if (was) back.unshift({ op: 'node.update', id: op.id, set: previous(was, op.set || {}) });
          break;
        }
        case 'edge.add':
          if (made) back.unshift({ op: 'edge.remove', id: made });
          break;
        case 'edge.remove': {
          const was = before.edges.get(op.id);
          if (was) back.unshift({ op: 'edge.add', edge: edgeSpec(was) });
          break;
        }
        case 'edge.update': {
          const was = before.edges.get(op.id);
          if (was) back.unshift({ op: 'edge.update', id: op.id, set: previous(was, op.set || {}) });
          break;
        }
        case 'canvas.update': {
          const set = {};
          for (const key of Object.keys(op.set || {})) if (key in before.canvas) set[key] = before.canvas[key];
          if (Object.keys(set).length) back.unshift({ op: 'canvas.update', set });
          break;
        }
      }
    });
    return back;
  }

  // redoOf is a batch that does it again: what it added comes back as it
  // was (its node restored while it can be).
  function redoOf(ops, result) {
    return ops.map((op, index) => {
      const made = result?.ids?.[String(index)];
      if (op.op === 'node.add' && made) return { op: 'node.add', node: { id: made, restore: true } };
      if (op.op === 'edge.add' && made) return { op: 'edge.add', edge: { ...op.edge, id: made } };
      return op;
    });
  }

  function previous(was, set) {
    const out = {};
    for (const key of Object.keys(set)) {
      if (key === 'config') {
        const config = {};
        for (const k of Object.keys(set.config || {})) config[k] = was.config?.[k] ?? null;
        out.config = config;
      } else {
        out[key] = was[key] ?? (key === 'header' ? null : '');
      }
    }
    return out;
  }

  const edgeSpec = (e) => ({ id: e.id, from: e.from, to: e.to, template: e.template || '', mode: e.mode || '', deliver: e.deliver || '', header: e.header ?? null });

  async function undo() {
    const step = m.undoStack.pop();
    if (!step) return false;
    try {
      await apply(step.ops, { undo: false });
      m.redoStack.push(step);
    } catch (error) {
      cockpit.toast(`It cannot be undone: ${error.message}`, 'error');
    }
    return true;
  }

  async function redo() {
    const step = m.redoStack.pop();
    if (!step) return false;
    try {
      await apply(step.redo, { undo: false });
      m.undoStack.push(step);
    } catch (error) {
      cockpit.toast(`It cannot be done again: ${error.message}`, 'error');
    }
    return true;
  }

  // moveLocal places nodes at once, before the server hears of it.
  function moveLocal(places) {
    for (const [nid, rect] of places) {
      const n = node(nid);
      if (n) Object.assign(n, rect);
    }
    emit('change', 'geometry', [...places.keys()]);
  }

  // feed is an agent node's transcript: its tail read once, then what its
  // events add.
  async function feed(nid, { tail = 40 } = {}) {
    if (m.feeds.has(nid)) return m.feeds.get(nid);
    const list = [];
    m.feeds.set(nid, list);
    try {
      const data = await api(`/nodes/${encodeURIComponent(nid)}/transcript?tail=${tail}`);
      const known = new Set(list.map((e) => e.id));
      list.unshift(...(data.entries || []).filter((e) => !known.has(e.id)));
      list.running = !!data.running;
      list.session = data.session || '';
    } catch {
      /* not an agent yet, or gone: its events fill it */
    }
    return list;
  }

  // hasSession says an agent node's session has a transcript: none has
  // before a run of it began.
  async function hasSession(nid) {
    const data = await api(`/nodes/${encodeURIComponent(nid)}/transcript?tail=1`);
    return !!data?.exists;
  }

  // made says the canvas is on the server now, made by a request other
  // than a batch (the foreman's first question, a template): its events
  // say the rest.
  function made() {
    if (m.exists || closed) return;
    m.exists = true;
    load();
    emit('created');
  }

  async function foreman(text) {
    const result = await api('/foreman', { method: 'POST', body: { text, title: m.doc.title || '' } });
    made();
    return result;
  }

  // build puts a template's nodes on the canvas, making it if need be.
  async function build(template) {
    const result = await cockpit.api(cockpit.wsPath(ws, '/canvases'), { method: 'POST', body: { id, title: m.doc.title || '', template } });
    made();
    return result;
  }

  // refreshHooks reads the addresses of the webhook sources anew: a node
  // added has one only once the server made it.
  let hooksAsked = 0;
  async function refreshHooks() {
    if (!m.exists || closed || Date.now() - hooksAsked < 2000) return;
    hooksAsked = Date.now();
    let data;
    try {
      data = await api('');
    } catch {
      return;
    }
    if (closed) return;
    const next = new Map(Object.entries(data.hooks || {}));
    const changed = [...next].filter(([nid, url]) => m.hooks.get(nid) !== url).map(([nid]) => nid);
    m.hooks = next;
    for (const nid of changed) emit('change', 'runtime', { node: nid, runtime: node(nid)?.runtime || {} });
  }

  return Object.assign(m, {
    on, emit, load, close, apply, undo, redo, moveLocal, feed, hasSession, foreman, build, refreshHooks,
    node, edge, pendingFor, awaitingFor, titleOf, path, api,
    send: (nid, body) => api(`/nodes/${encodeURIComponent(nid)}/send`, { method: 'POST', body }),
    read: (nid, what = 'tail', lines = 0) => api(`/nodes/${encodeURIComponent(nid)}/read?what=${encodeURIComponent(what)}${lines ? `&lines=${lines}` : ''}`),
    stop: (nid) => api(`/nodes/${encodeURIComponent(nid)}/stop`, { method: 'POST', body: {} }),
    restart: (nid, resume = false) => api(`/nodes/${encodeURIComponent(nid)}/restart`, { method: 'POST', body: { resume } }),
    fire: (nid, text = '') => api(`/nodes/${encodeURIComponent(nid)}/fire`, { method: 'POST', body: { text } }),
    rotate: (nid) => api(`/nodes/${encodeURIComponent(nid)}/token`, { method: 'POST', body: {} }),
    removeWorktree: (nid) => api(`/nodes/${encodeURIComponent(nid)}/worktree`, { method: 'DELETE' }),
    approve: (mid, text) => api(`/messages/${encodeURIComponent(mid)}/approve`, { method: 'POST', body: text === undefined ? {} : { text } }),
    drop: (mid) => api(`/messages/${encodeURIComponent(mid)}`, { method: 'DELETE' }),
    message: (mid) => api(`/messages/${encodeURIComponent(mid)}`),
    saveTemplate: (name, description) => api('/template', { method: 'POST', body: { name, description } }),
    patch: (set) => api('', { method: 'PATCH', body: set }),
    remove: (query = '') => api(query ? `?${query}` : '', { method: 'DELETE' }),
  });
}
