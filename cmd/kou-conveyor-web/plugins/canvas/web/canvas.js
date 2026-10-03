// canvas: the Canvas view (layout.view "canvas", #/w/<workspace>/c/<id>) —
// a board of terminals, agents, event sources and notes, wired output to
// input, which the server's engine runs (cmd/internal/canvas). The page has
// its own bar (the canvas's title, what is busy, its agents, Live or
// Paused, + Add, the zoom and the canvas's actions), the board
// (surface.js), the window of its agents over the board (agents.js), the
// inspector over its right (inspector.js), the foreman's field at its foot
// (foreman.js) and, on a phone, the list of its nodes in its place. The
// rail lists the workspace's canvases, the nodes of the one in view and
// what happened on it (rail.js).
//
// A new session becomes a canvas by the Chat · Canvas switch in the bar
// (what was written goes to the foreman); /canvas, the palette and the
// rail's tab open one too, and the session list shows the workspace's
// canvases among the sessions (session-list.rows), hiding the sessions of
// their agents. A session that is a node's agent shows its canvas in the
// bar, the way back to it.
//
// Plugins add to it through
//
//   canvas.node       { kind | preset, order, create(node, ctx) → body }: what
//                     a node shows (nodes/*.js are the canvas's own)
//   canvas.add        { id, group, title, icon, order, shown(canvas), create(at) }:
//                     items of the + Add menu
//   canvas.template   { id, title, description, build() → ops }: canvases an
//                     empty one can start from
//   canvas.inspector  { kinds, order, title, render(node, ctx) }: sections of
//                     a node's inspector
//
// and it provides the canvas service: open(id, ws), create({ template,
// title }), current(), addNode(spec, at), select(ids), focusNode(id), fit().
// Without the server's canvases (-canvas=off) it shows nothing.
import { createModel } from './model.js';
import { createSurface } from './surface.js';
import { createInspector } from './inspector.js';
import { createRail, outlineRows } from './rail.js';
import { createForeman } from './foreman.js';
import { createAgents } from './agents.js';
import { form, missing } from './forms.js';
import { createTerminalBody } from './nodes/terminal.js';
import { createAgentBody } from './nodes/agent.js';
import { createSourceBody } from './nodes/source.js';
import { createNoteBody } from './nodes/note.js';

const ROUTE = /^#\/w\/([A-Za-z0-9-]{1,80})\/c\/([A-Za-z0-9-]{1,128})$/;
const MENU = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2 4h12M2 8h12M2 12h12" stroke="currentColor" stroke-width="1.4"/></svg>';
const DOTS = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3 8h1.5M7.25 8h1.5M11.5 8H13" stroke="currentColor" stroke-width="2"/></svg>';
const FIT = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2 6V2h4M10 2h4v4M14 10v4h-4M6 14H2v-4" fill="none" stroke="currentColor" stroke-width="1.4"/></svg>';
// A phone shows the list of the nodes in place of the board.
const NARROW = 720;
// How many canvases' views (where the board was left) are kept.
const VIEWS_KEPT = 60;
// How long the bar that undoes a deletion stays.
const UNDO_FOR = 15000;
// The canvas's menus are compact: a row a line, the details in tooltips.
const COMPACT = { compact: true };

export default function activate(cockpit) {
  const { h, svg, fmt, prefs } = cockpit;
  const hot = cockpit.hot.data;
  const session = cockpit.use('session');
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const layout = () => service('layout');
  const state = () => session.state || {};
  const mac = /Mac|iPhone|iPad/.test(navigator.platform || '') || navigator.userAgentData?.platform === 'macOS';
  const mod = mac ? '⌘' : 'Ctrl+';
  // The server runs canvases: /api/config says so.
  const enabled = () => state().config?.canvas === true;
  const shown = () => cockpit.store.get('view') === 'canvas';
  const canvasHash = (ws, id) => `#/w/${ws}/c/${id}`;

  // ui is the canvas in view: its workspace and ID, its model, its board.
  const ui = {
    ws: null, id: null, model: null, surface: null, offs: [], ticket: 0,
    fitAfter: false, galleryClosed: false, revealAfter: '', outline: true, building: false,
  };

  // ---------------------------------------------------------------- what the workspace has

  // The kinds of nodes a workspace's plugins offer: presets of terminals,
  // sources of events, the agent's settings.
  const kindsByWs = new Map();
  const kindsAsked = new Map();
  function loadKinds(ws, { force = false } = {}) {
    if (!ws) return Promise.resolve(null);
    if (!force && kindsByWs.has(ws)) return Promise.resolve(kindsByWs.get(ws));
    if (!force && kindsAsked.has(ws)) return kindsAsked.get(ws);
    const asked = cockpit.api(cockpit.wsPath(ws, '/canvas/kinds')).then((data) => {
      kindsByWs.set(ws, data || {});
      return kindsByWs.get(ws);
    }, () => {
      if (!kindsByWs.has(ws)) kindsByWs.set(ws, { harnesses: [], sources: [], agent: null });
      return kindsByWs.get(ws);
    }).finally(() => kindsAsked.delete(ws));
    kindsAsked.set(ws, asked);
    return asked;
  }
  const same = (a, b) => (a || '') === (b || '');
  const kinds = {
    get data() { return kindsByWs.get(ui.ws || state().ws) || null; },
    harnesses: () => kinds.data?.harnesses || [],
    sources: () => kinds.data?.sources || [],
    harness: (n) => kinds.harnesses().find((x) => x.id === (n?.preset || 'shell') && same(x.plugin, n?.plugin)) || null,
    source: (n) => kinds.sources().find((x) => x.id === n?.preset && same(x.plugin, n?.plugin)) || null,
    agent: () => kinds.data?.agent || null,
  };

  // The templates a canvas can start from.
  const templatesByWs = new Map();
  async function loadTemplates(ws, { force = false } = {}) {
    if (!ws || (!force && templatesByWs.has(ws))) return;
    try {
      const data = await cockpit.api(cockpit.wsPath(ws, '/canvas/templates'));
      templatesByWs.set(ws, data?.templates || []);
    } catch {
      if (!templatesByWs.has(ws)) templatesByWs.set(ws, []);
    }
    scheduleDraw();
  }

  // The workspace's canvases, as the server sums them up.
  const lists = new Map(); // ws → { canvases, at }
  const listing = new Map();
  function refreshList(ws = state().ws, { force = false } = {}) {
    if (!ws || !enabled()) return Promise.resolve();
    const known = lists.get(ws);
    if (!force && known && Date.now() - known.at < 5000) return Promise.resolve();
    if (listing.has(ws)) return listing.get(ws);
    const asked = (async () => {
      try {
        const data = await cockpit.api(cockpit.wsPath(ws, '/canvases'));
        lists.set(ws, { canvases: data?.canvases || [], at: Date.now() });
      } catch {
        if (!known) lists.set(ws, { canvases: [], at: Date.now() });
        else return;
      }
      rail.render('list');
      drawCrumb();
      service('session-list')?.render?.();
    })().finally(() => listing.delete(ws));
    listing.set(ws, asked);
    return asked;
  }
  const canvasesOf = (ws) => lists.get(ws)?.canvases || [];

  function metaOf(c) {
    const plural = (n, word) => `${n} ${word}${n === 1 ? '' : 's'}`;
    return [
      'canvas',
      c.agents ? plural(c.agents, 'agent') : '',
      c.terminals ? plural(c.terminals, 'terminal') : '',
      !c.agents && !c.terminals && c.nodes ? plural(c.nodes, 'node') : '',
      c.live ? 'live' : 'paused',
      c.busy ? `${c.busy} busy` : '',
      c.waiting ? `${c.waiting} waiting` : '',
      fmt.ago(c.updated_at),
    ].filter(Boolean).join(' · ');
  }

  // ---------------------------------------------------------------- helpers for the nodes

  function markdown(text) {
    const box = h('div', { class: 'prose' });
    const md = service('markdown');
    if (md?.render) box.append(md.render(String(text || '')));
    else box.textContent = String(text || '');
    return box;
  }

  const modelTitle = (id) => (id ? service('models')?.find?.(id)?.name || id : '');
  const modelList = () => {
    service('models')?.want?.();
    return (service('models')?.catalog?.()?.models || []).filter((m) => !m.media).map((m) => ({ id: m.id, name: m.name || m.id }));
  };

  // short is a path as the user reads it: in the workspace, from it; in
  // the home folder, from ~.
  function short(path) {
    if (!path) return '';
    const w = session.currentWorkspace?.();
    if (w?.path) {
      if (path === w.path) return '.';
      if (path.startsWith(`${w.path}/`)) return path.slice(w.path.length + 1);
      if (w.display?.startsWith('~')) {
        const home = w.path.slice(0, w.path.length - w.display.length + 1);
        if (home && (path === home || path.startsWith(`${home}/`))) return `~${path.slice(home.length)}`;
      }
    }
    return path;
  }

  // The bodies of the kinds of nodes: the canvas's own, which a plugin's
  // canvas.node for a kind or a preset replaces.
  for (const [kind, create] of [['terminal', createTerminalBody], ['agent', createAgentBody], ['source', createSourceBody], ['note', createNoteBody]]) {
    cockpit.contribute('canvas.node', { kind, order: 0, create });
  }
  function bodyFor(node, ctx) {
    let best = null;
    let score = -1;
    for (const item of cockpit.contributions('canvas.node')) {
      if (typeof item.create !== 'function') continue;
      let s = -1;
      if (item.preset) {
        const named = String(item.preset);
        const slash = named.lastIndexOf('/');
        const plugin = slash >= 0 ? named.slice(0, slash) : null;
        const id = slash >= 0 ? named.slice(slash + 1) : named;
        if (id === node.preset && (plugin === null || same(plugin, node.plugin)) && (!item.kind || item.kind === node.kind)) s = 2;
      } else if (item.kind === node.kind) {
        s = 1;
      }
      // Of those that fit as well, the last by order and arrival.
      if (s >= 0 && s >= score) {
        best = item;
        score = s;
      }
    }
    return best ? best.create(node, ctx) : null;
  }

  // ---------------------------------------------------------------- the page

  const wsCrumb = h('button', {
    class: 'crumb-ws', type: 'button', title: 'Switch workspace (W)',
    onclick: (event) => service('workspaces')?.openMenu?.(event.currentTarget),
  }, 'workspace');
  const titleEl = h('strong', { class: 'cv-crumb-title', data: { editable: 'true' }, title: 'Rename the canvas (F2)', onclick: () => renameCanvas() }, 'Canvas');
  const idEl = h('code');
  const modeChat = h('button', { type: 'button', role: 'radio', 'aria-checked': 'false', title: 'A session with the agent instead', onclick: () => canvasToChat() }, 'Chat');
  const modeCanvas = h('button', { type: 'button', role: 'radio', 'aria-checked': 'true' }, 'Canvas');
  const pageMode = h('div', { class: 'cv-mode', role: 'radiogroup', 'aria-label': 'What it is', hidden: true }, modeChat, modeCanvas);
  const crumbs = h('div', { class: 'crumbs' }, wsCrumb, h('i', { text: '/' }), h('b', { class: 'cv-crumb-glyph', 'aria-hidden': 'true', text: '◧' }), titleEl, idEl, pageMode);

  const liveWord = h('b', { text: 'Live' });
  const liveButton = h('button', {
    class: 'cv-live', type: 'button', role: 'switch', 'aria-checked': 'true',
    title: 'Live: its sources run and its messages go. Click to pause it.',
    onclick: () => setLive(!(ui.model?.doc.live !== false)),
  }, h('i', { 'aria-hidden': 'true' }), liveWord);
  const counts = h('span', { class: 'cv-counts', 'aria-live': 'polite' });
  const addButton = h('button', {
    class: 'act strong cv-add', type: 'button', title: 'Add a node (A)', 'aria-haspopup': 'menu', 'aria-expanded': 'false',
    onclick: (event) => service('menu')?.toggle?.(event.currentTarget, () => addItems(null, null), COMPACT),
  }, '+ Add');
  const fitButton = h('button', { class: 'icon', type: 'button', title: `Fit everything (${mod}0)`, 'aria-label': 'Fit everything', onclick: () => ui.surface?.fit() }, svg(FIT));
  const zoomButton = h('button', {
    class: 'act cv-zoom', type: 'button', title: 'Zoom', 'aria-haspopup': 'menu', 'aria-expanded': 'false',
    onclick: (event) => zoomMenu(event.currentTarget),
  }, '100%');
  const moreButton = h('button', {
    class: 'icon', type: 'button', title: 'Canvas actions', 'aria-label': 'Canvas actions', 'aria-haspopup': 'menu', 'aria-expanded': 'false',
    onclick: (event) => service('menu')?.toggle?.(event.currentTarget, () => canvasActions(), COMPACT),
  }, svg(DOTS));
  // The agents on the canvas, in a window over the board, and the bar's
  // switch of it.
  const agents = createAgents({
    h, fmt, prefs, kinds, modelTitle,
    model: () => ui.model,
    selected: () => ui.surface?.selected() || [],
    reveal: (id) => reveal(id),
    enter: (id) => ui.surface?.enter(id),
  });
  cockpit.onDispose(() => agents.dispose());
  const bar = h('header', { class: 'bar cv-bar' },
    h('button', { class: 'icon rail-toggle', type: 'button', 'aria-label': 'Menu', onclick: () => layout()?.rail?.() }, svg(MENU)),
    crumbs,
    h('div', { class: 'bar-right' }, counts, agents.button, liveButton, addButton, fitButton, zoomButton, moreButton));

  // What shows over the board while it has nothing: the templates.
  const gallery = h('div', { class: 'cv-empty', hidden: true });
  // Loading, failed, deleted, read only, reconnecting.
  const banner = h('div', { class: 'cv-banner', hidden: true, role: 'status' });
  // The list of the nodes a phone shows in place of the board.
  const narrowList = h('div', { class: 'cv-narrow', role: 'list', 'aria-label': 'The nodes' });
  narrowList.dataset.scroll = 'true';
  const outlineBack = h('button', { class: 'act cv-outline-back', type: 'button', hidden: true, onclick: () => showOutline(true) }, '☰ Nodes');
  const undoText = h('span');
  const undoBar = h('div', { class: 'cv-undo', hidden: true, role: 'status' }, undoText,
    h('button', { class: 'act strong', type: 'button', onclick: () => { hideUndo(); undo(); } }, `Undo ${mod}Z`),
    h('button', { class: 'icon small', type: 'button', 'aria-label': 'Dismiss', onclick: () => hideUndo() }, '×'));

  const inspector = createInspector({
    cockpit, h, fmt, kinds,
    model: () => ui.model,
    models: modelList,
    short,
    openSession: (id) => openAgentSession(id),
    remove: (id) => ui.surface?.removeNodes([id]),
    removeEdge: (id) => {
      ui.surface?.selectEdge(id);
      ui.surface?.removeSelected();
    },
    rename: (title) => renameTo(title),
    setLive: (live) => setLive(live),
    onToggle: (open) => {
      main.dataset.inspector = open ? 'open' : '';
    },
    onClose: () => {
      if (shown()) ui.surface?.focus();
    },
  });
  const foreman = createForeman({
    h, cockpit,
    model: () => ui.model,
    markdown,
    reveal: (id) => reveal(id),
    done: () => ui.surface?.focus(),
    sent: (result) => {
      if (result?.created) cockpit.toast('The foreman is on the canvas: it answers in its node');
    },
  });

  const main = h('div', { class: 'cv-main' }, gallery, banner, narrowList, outlineBack, agents.element, inspector.element, foreman.element, undoBar);
  const page = h('section', { class: 'cv-page', 'aria-label': 'Canvas' }, bar, main);

  // ---------------------------------------------------------------- the rail

  const rail = createRail({
    cockpit, h, fmt, kinds,
    model: () => ui.model,
    canvases: () => canvasesOf(state().ws),
    current: () => (ui.model && ui.ws === state().ws ? { id: ui.id, title: ui.model.doc.title, fresh: !ui.model.exists } : null),
    hash: (id) => canvasHash(state().ws, id),
    open: (id) => {
      layout()?.rail?.(false);
      go(state().ws, id);
    },
    create: () => {
      layout()?.rail?.(false);
      createCanvas();
    },
    back: () => backToSessions(),
    metaOf,
    menuOf: (c) => canvasMenu(state().ws, c),
    selected: () => ui.surface?.selected() || [],
    reveal: (id) => {
      layout()?.rail?.(false);
      reveal(id);
    },
    revealEdge: (id) => {
      layout()?.rail?.(false);
      revealEdge(id);
    },
    enter: (id) => {
      layout()?.rail?.(false);
      ui.surface?.enter(id);
    },
  });

  // ---------------------------------------------------------------- opening a canvas

  // go shows a canvas, saying so in the address.
  function go(ws, id, { replace = false } = {}) {
    const hash = canvasHash(ws, id);
    if (location.hash !== hash) history[replace ? 'replaceState' : 'pushState'](null, '', hash);
    routeCanvas(ws, id);
  }

  // routeCanvas is the address #/w/<ws>/c/<id>.
  function routeCanvas(ws, id) {
    if (!state().configured) return; // the session plugin routes again once it is
    if (!enabled()) {
      cockpit.toast('This server runs no canvases (kou-conveyor-web -canvas=off).', 'error', 'canvas-off');
      history.replaceState(null, '', session.workspaceHash(state().ws));
      session.routeSessions();
      return;
    }
    const workspaces = state().workspaces || [];
    if (workspaces.length && !workspaces.some((w) => w.id === ws)) {
      cockpit.toast('That workspace is not in the list any more', 'error');
      history.replaceState(null, '', session.workspaceHash(state().ws));
      session.routeSessions();
      return;
    }
    if (ws !== state().ws) session.selectWorkspace(ws);
    session.ensureView?.();
    open(ws, id);
    layout()?.show?.('canvas');
  }

  // open makes a canvas the one in view: a canvas not on the server yet is
  // a fresh one, saved by what is first done to it.
  async function open(ws, id, { title = '' } = {}) {
    if (ui.model && ui.ws === ws && ui.id === id && !ui.model.deleted && !ui.model.failed) {
      drawAll();
      return;
    }
    close();
    const ticket = ++ui.ticket;
    const m = createModel(cockpit, ws, id, { title });
    Object.assign(ui, { ws, id, model: m, fitAfter: false, galleryClosed: false, outline: true });
    hot.current = { ws, id };
    ui.offs = listen(m);
    drawAll();
    const kindsReady = loadKinds(ws);
    loadTemplates(ws);
    refreshList(ws);
    m.load();
    await kindsReady;
    if (ticket !== ui.ticket || ui.model !== m) return;
    mount(m);
  }

  // mount puts the board of the model on the page.
  function mount(m) {
    const ws = m.ws;
    const id = m.id;
    const s = createSurface(m, {
      cockpit, h, fmt, kinds,
      bodyFor,
      markdown,
      modelTitle,
      short,
      openSession: (sid) => openAgentSession(sid),
      inspect: (target) => inspect(target),
      addMenu: (anchor, at, options) => openAddMenu(anchor, at, options),
      onSelect: ({ nodes, edge }) => selectionChanged(nodes, edge),
      onEnter: () => rail.render('outline'),
      onView: (v) => {
        const text = `${Math.round(v.zoom * 100)}%`;
        if (zoomButton.textContent !== text) zoomButton.textContent = text;
      },
      insets: () => insets(),
      savedView: () => savedView(ws, id),
      saveView: (v) => saveView(ws, id, v),
      undoBar: (text) => showUndo(text),
    });
    ui.surface = s;
    main.prepend(s.element);
    if (shown()) s.shown();
    if (!m.loading) revealPending();
    drawAll();
  }

  // close puts the canvas in view away.
  function close() {
    ui.ticket++;
    for (const off of ui.offs) off();
    ui.offs = [];
    ui.surface?.dispose();
    ui.surface = null;
    ui.model?.close();
    ui.model = null;
    ui.ws = null;
    ui.id = null;
    hot.current = null;
    inspector.hide();
    hideUndo();
    closeAsk(null);
  }
  cockpit.onDispose(() => close());

  // listen follows what the model says, for the parts around the board.
  function listen(m) {
    return [
      m.on('change', (what, data) => {
        if (what === 'geometry') return;
        scheduleDraw();
        rail.render('outline');
        refreshInspector();
        if (what === 'load') {
          if (m.exists) prefs.set(`canvas.last.${m.ws}`, m.id);
          revealPending();
        }
        if ((what === 'load' || what === 'ops') && ui.fitAfter && m.doc.nodes.length) {
          ui.fitAfter = false;
          requestAnimationFrame(() => ui.surface?.fit());
        }
        if (what === 'ops') {
          const renamed = (data?.changes || []).some((c) => c.op === 'canvas.update');
          refreshList(m.ws, { force: renamed });
          if (renamed) rail.render('list');
        }
      }),
      m.on('status', () => {
        scheduleDraw();
        rail.render('outline');
        refreshInspector();
      }),
      m.on('message', () => {
        scheduleDraw();
        rail.render('outline');
        refreshInspector();
      }),
      m.on('terminal', () => refreshInspector()),
      m.on('activity', () => rail.render('activity')),
      m.on('output', (event) => {
        foreman.answered(event);
        if (inspector.target()?.edge) refreshInspector();
      }),
      m.on('notice', (event) => {
        if (event.level === 'error') cockpit.toast(event.text, 'error', 'canvas-notice');
      }),
      m.on('stream', () => scheduleDraw()),
      m.on('created', () => {
        prefs.set(`canvas.last.${m.ws}`, m.id);
        refreshList(m.ws, { force: true });
        pollLive();
        scheduleDraw();
      }),
      m.on('deleted', () => {
        cockpit.toast('This canvas was deleted', 'error', 'canvas-deleted');
        refreshList(m.ws, { force: true });
        scheduleDraw();
      }),
    ];
  }

  // revealPending shows the node asked for before the canvas was there:
  // the agent whose session led back to it.
  function revealPending() {
    const id = ui.revealAfter;
    if (!id || !ui.surface || !ui.model || ui.model.loading) return;
    ui.revealAfter = '';
    if (!ui.model.node(id)) return;
    requestAnimationFrame(() => requestAnimationFrame(() => reveal(id)));
  }

  // createCanvas opens a new canvas: nothing is saved until something is
  // done to it.
  function createCanvas({ title = '', template = '' } = {}) {
    const ws = state().ws;
    if (!ws) return cockpit.toast('Pick a workspace first', 'error');
    if (!enabled()) return cockpit.toast('This server runs no canvases (kou-conveyor-web -canvas=off).', 'error', 'canvas-off');
    const id = cockpit.uuid();
    history.pushState(null, '', canvasHash(ws, id));
    session.ensureView?.();
    open(ws, id, { title });
    layout()?.show?.('canvas');
    if (template) build(template);
    return id;
  }

  // openLast goes to the canvas last open in the workspace, or a new one.
  async function openLast() {
    const ws = state().ws;
    if (!ws) return;
    if (ui.model && ui.ws === ws && !ui.model.deleted) {
      go(ws, ui.id);
      return;
    }
    if (!lists.has(ws)) await refreshList(ws);
    const list = canvasesOf(ws);
    const last = prefs.get(`canvas.last.${ws}`, '');
    const id = list.some((c) => c.id === last) ? last : list[0]?.id;
    if (id) go(ws, id);
    else createCanvas();
  }

  function backToSessions() {
    layout()?.rail?.(false);
    const sessions = cockpit.contributions('layout.view', { unique: 'id' }).find((v) => v.id === 'sessions');
    if (sessions?.select) cockpit.safely(sessions.select);
    else layout()?.show?.('sessions');
  }

  async function openAgentSession(sid) {
    if (!sid) return;
    const ws = ui.ws || state().ws;
    // An agent none of whose runs began has no session to open yet.
    const n = ui.model?.doc.nodes.find((x) => x.runtime?.session === sid);
    if (n && !(await ui.model.hasSession(n.id).catch(() => true))) {
      cockpit.toast(`«${n.title || 'The agent'}» has no session yet: none of its runs began.`, 'error');
      return;
    }
    if (ws !== state().ws) session.selectWorkspace(ws);
    session.openSession(sid);
  }

  // ---------------------------------------------------------------- Chat · Canvas

  // A new session's switch, in the stage's bar: Canvas takes its ID for a
  // canvas, and what was written there to the foreman.
  const sessionChat = h('button', { type: 'button', role: 'radio', 'aria-checked': 'true' }, 'Chat');
  const sessionCanvas = h('button', {
    type: 'button', role: 'radio', 'aria-checked': 'false',
    title: 'A canvas: terminals, agents and events, wired together',
    onclick: () => sessionToCanvas(),
  }, 'Canvas');
  const modeSwitch = h('div', { class: 'cv-mode', role: 'radiogroup', 'aria-label': 'What the new session is', hidden: true }, sessionChat, sessionCanvas);

  function sessionToCanvas() {
    const v = session.view?.();
    const ws = state().ws;
    if (!v?.fresh || !ws || !enabled()) return;
    const composer = service('composer');
    const draft = composer?.value?.() || '';
    if (draft) composer.clear?.({ images: false });
    history.pushState(null, '', canvasHash(ws, v.id));
    open(ws, v.id);
    layout()?.show?.('canvas');
    foreman.set(draft);
    requestAnimationFrame(() => foreman.focus());
  }

  function canvasToChat() {
    const m = ui.model;
    const ws = ui.ws || state().ws;
    const text = foreman.value();
    foreman.set('');
    // A canvas that was saved keeps its ID: the chat takes another.
    const v = session.view?.();
    if (m?.exists || !v?.fresh || v.id !== ui.id) session.newSession({ push: false });
    history.pushState(null, '', session.workspaceHash(ws));
    layout()?.show?.('sessions');
    session.routeSessions();
    if (text) service('composer')?.set?.(text, { focus: true, end: true });
    else service('composer')?.focus?.();
  }

  // ---------------------------------------------------------------- the bar

  let drawFrame = 0;
  function scheduleDraw() {
    if (drawFrame) return;
    drawFrame = requestAnimationFrame(() => {
      drawFrame = 0;
      drawAll();
    });
  }
  cockpit.onDispose(() => cancelAnimationFrame(drawFrame));

  function drawAll() {
    drawChrome();
    drawBanner();
    drawGallery();
    drawNarrow();
    foreman.draw();
    agents.draw();
    rail.render();
  }

  function drawChrome() {
    const m = ui.model;
    const w = session.currentWorkspace?.();
    const wsName = w?.name || state().config?.workspace_name || 'workspace';
    if (wsCrumb.textContent !== wsName) wsCrumb.textContent = wsName;
    const title = m?.doc.title || 'Canvas';
    if (titleEl.textContent !== title) titleEl.textContent = title;
    titleEl.dataset.editable = m && !m.readOnly && !m.deleted ? 'true' : 'false';
    idEl.textContent = !m ? '' : m.exists ? m.id.slice(0, 8) : 'unsaved';
    const fresh = !!m && !m.exists && !m.loading && m.doc.nodes.length === 0;
    pageMode.hidden = !fresh;

    const editable = !!m && !m.readOnly && !m.deleted && !m.loading;
    const live = m?.doc.live !== false;
    liveButton.setAttribute('aria-checked', String(live));
    liveButton.dataset.live = live ? 'true' : 'false';
    liveWord.textContent = live ? 'Live' : 'Paused';
    liveButton.title = live ? 'Live: its sources run and its messages go. Click to pause it.' : 'Paused: no source runs, and messages wait. Click to make it live.';
    liveButton.disabled = !editable || !m.exists;
    addButton.disabled = !editable;
    moreButton.disabled = !m;

    // What runs on quietly — a server, a watcher — is not busy.
    let busy = 0;
    let waiting = 0;
    if (m) {
      for (const [id, st] of m.status) {
        if (!m.node(id)) continue;
        if ((st?.state === 'busy' && !st.quiet) || st?.state === 'starting') busy++;
        else if (st?.state === 'waiting') waiting++;
      }
    }
    const pending = m ? m.queue.length : 0;
    const text = [busy ? `${busy} busy` : '', waiting ? `${waiting} waiting` : '', pending ? `${pending} pending` : ''].filter(Boolean).join(' · ');
    if (counts.textContent !== text) counts.textContent = text;
    counts.dataset.state = waiting || m?.queue.some((q) => q.state === 'awaiting_approval') ? 'waiting' : busy ? 'busy' : '';
  }

  function drawBanner() {
    const m = ui.model;
    const parts = [];
    let kind = '';
    if (!m) {
      kind = 'none';
      parts.push(h('span', { text: 'No canvas open.' }),
        h('button', { class: 'act strong', type: 'button', onclick: () => createCanvas() }, 'New canvas'));
    } else if (m.failed) {
      kind = 'error';
      parts.push(h('b', { text: 'It did not load' }), h('span', { text: m.failed }),
        h('button', { class: 'act strong', type: 'button', onclick: () => m.load() }, 'Try again'));
    } else if (m.loading) {
      kind = 'loading';
      parts.push(h('span', { class: 'meter', 'aria-hidden': 'true' }, h('i'), h('i'), h('i'), h('i'), h('i')), h('span', { text: 'Loading the canvas…' }));
    } else if (m.deleted) {
      kind = 'error';
      parts.push(h('b', { text: 'Deleted' }), h('span', { text: 'This canvas was deleted: what it ran ended.' }),
        h('button', { class: 'act', type: 'button', onclick: () => backToSessions() }, 'Back to the sessions'));
    } else if (m.readOnly) {
      kind = 'warn';
      parts.push(h('b', { text: 'Read only' }), h('span', { text: 'A newer kou-conveyor made this canvas: it shows here, but does not change.' }));
    } else if (m.stream === 'reconnecting') {
      kind = 'warn';
      parts.push(h('span', { class: 'meter', 'aria-hidden': 'true' }, h('i'), h('i'), h('i'), h('i'), h('i')), h('span', { text: 'Reconnecting…' }));
    }
    banner.hidden = !kind;
    banner.dataset.kind = kind;
    if (banner.dataset.key !== `${kind}|${m?.failed || ''}`) {
      banner.dataset.key = `${kind}|${m?.failed || ''}`;
      banner.replaceChildren(...parts);
    }
  }

  // drawGallery offers an empty canvas the templates to start from.
  function drawGallery() {
    const m = ui.model;
    const show = !!m && !m.loading && !m.failed && !m.readOnly && !m.deleted && m.doc.nodes.length === 0 && !ui.galleryClosed && !(narrow && ui.outline);
    gallery.hidden = !show;
    if (!show) return;
    // A template without nodes is the Empty card below.
    const templates = (templatesByWs.get(ui.ws) || []).filter((t) => t.nodes > 0);
    const own = cockpit.contributions('canvas.template', { unique: 'id' });
    const key = JSON.stringify([ui.ws, ui.building, templates.map((t) => t.id), own.map((t) => t.id)]);
    if (gallery.dataset.key === key) return;
    gallery.dataset.key = key;
    const card = (title, description, detail, run, extra = '') => h('button', {
      class: `cv-card${extra ? ` ${extra}` : ''}`, type: 'button', disabled: ui.building,
      onclick: () => cockpit.safely(run),
    }, h('b', { text: title }), description ? h('span', { text: description }) : null, detail ? h('small', { text: detail }) : null);
    const cards = [
      ...templates.map((t) => card(t.title || t.id, t.description, [
        `${t.nodes} node${t.nodes === 1 ? '' : 's'}`, t.edges ? `${t.edges} edge${t.edges === 1 ? '' : 's'}` : '', (t.kinds || []).join(', '),
        t.source && t.source !== 'builtin' ? t.source : '',
      ].filter(Boolean).join(' · '), () => build(t.id))),
      ...own.map((t) => card(t.title || t.id, t.description, t.plugin ? `from ${t.plugin}` : '', () => buildOwn(t))),
      card('Empty', 'Start from nothing: + Add, or ask the foreman below.', '', () => {
        ui.galleryClosed = true;
        drawGallery();
        ui.surface?.focus();
      }, 'cv-card-empty'),
    ];
    gallery.replaceChildren(
      h('div', { class: 'cv-empty-head' },
        h('span', { class: 'label', text: 'An empty canvas' }),
        h('p', { text: 'Terminals, agents and sources of events, wired output to input: what one puts out goes into the next. Start from a template, add nodes (A, or double-click the board), or tell the foreman what to build.' })),
      h('div', { class: 'cv-gallery' }, ...cards));
  }

  // build puts a server template's nodes on the canvas in view.
  async function build(template) {
    const m = ui.model;
    if (!m || ui.building) return;
    ui.building = true;
    drawGallery();
    try {
      ui.fitAfter = true;
      await m.build(template);
    } catch (error) {
      ui.fitAfter = false;
      cockpit.toast(`The template did not build: ${error.message}`, 'error');
    } finally {
      ui.building = false;
      scheduleDraw();
    }
  }

  // buildOwn puts a plugin's template on the canvas, as one batch.
  async function buildOwn(template) {
    const m = ui.model;
    if (!m || ui.building) return;
    const ops = cockpit.safely(() => template.build?.());
    if (!Array.isArray(ops) || !ops.length) return;
    ui.building = true;
    try {
      ui.fitAfter = true;
      await m.apply(ops, { label: `Template ${template.title || template.id}` });
    } catch (error) {
      ui.fitAfter = false;
      cockpit.toast(`The template did not build: ${error.message}`, 'error');
    } finally {
      ui.building = false;
      scheduleDraw();
    }
  }

  // ---------------------------------------------------------------- a phone

  const narrowQuery = matchMedia(`(max-width: ${NARROW}px)`);
  let narrow = narrowQuery.matches;
  cockpit.listen(narrowQuery, 'change', (event) => {
    narrow = event.matches;
    ui.outline = true;
    drawNarrow();
    drawGallery();
  });

  function showOutline(on) {
    ui.outline = on;
    drawNarrow();
    drawGallery();
  }

  function drawNarrow() {
    const m = ui.model;
    const listing = narrow && ui.outline && !!m && !m.loading;
    main.dataset.narrow = narrow ? 'true' : '';
    main.dataset.outline = listing ? 'true' : '';
    outlineBack.hidden = !(narrow && !ui.outline && m);
    if (!listing) return;
    const rows = outlineRows({ h, kinds }, m, {
      selected: ui.surface?.selected() || [],
      onPick: (id) => focusOnPhone(id),
      onEnter: (id) => focusOnPhone(id),
    });
    narrowList.replaceChildren(
      h('div', { class: 'cv-narrow-head' },
        h('span', { class: 'label', text: `${m.doc.nodes.length} node${m.doc.nodes.length === 1 ? '' : 's'}` }),
        h('button', { class: 'act', type: 'button', onclick: () => showOutline(false) }, 'The board')),
      ...(rows.length ? rows : [h('p', { class: 'none', text: 'No nodes yet: + Add, or ask the foreman.' })]));
  }

  function focusOnPhone(id) {
    showOutline(false);
    requestAnimationFrame(() => ui.surface?.focusMode(id));
  }

  // ---------------------------------------------------------------- the board's neighbours

  // insets are the parts of the board the inspector and the foreman's
  // field cover.
  function insets() {
    let right = 0;
    if (inspector.isOpen()) {
      const width = inspector.element.getBoundingClientRect().width;
      if (width && width < main.clientWidth * 0.6) right = width + 12;
    }
    return { right, bottom: foreman.height() };
  }

  function inspect(target) {
    if (!target) return;
    if (target.node && ui.model?.node(target.node)?.kind === 'agent') service('models')?.want?.();
    inspector.show(target);
  }

  let inspectorFrame = 0;
  function refreshInspector() {
    if (!inspector.isOpen() || inspectorFrame) return;
    inspectorFrame = requestAnimationFrame(() => {
      inspectorFrame = 0;
      inspector.refresh();
    });
  }
  cockpit.onDispose(() => cancelAnimationFrame(inspectorFrame));

  // The inspector follows the selection while it is open.
  function selectionChanged(nodes, edge) {
    rail.render('outline');
    agents.draw();
    if (narrow) drawNarrow();
    if (!inspector.isOpen()) return;
    const target = inspector.target();
    if (target?.canvas) return;
    if (nodes.length === 1 && target?.node !== nodes[0]) inspector.show({ node: nodes[0] });
    else if (!nodes.length && edge && target?.edge !== edge) inspector.show({ edge });
  }

  // reveal selects a node and brings it into view.
  function reveal(id) {
    const s = ui.surface;
    if (!s || !ui.model?.node(id)) return;
    if (narrow) {
      focusOnPhone(id);
      return;
    }
    s.select([id]);
    s.center(id);
    s.focus();
  }

  function revealEdge(id) {
    const s = ui.surface;
    const e = ui.model?.edge(id);
    if (!s || !e) return;
    if (narrow) showOutline(false);
    s.selectEdge(id);
    s.fit([e.from.node, e.to.node], { max: Math.max(s.zoom(), 0.6) });
    s.focus();
  }

  // The places the board was left, by canvas, the latest kept.
  function savedView(ws, id) {
    const all = prefs.get('canvas.views', null);
    return all && typeof all === 'object' ? all[`${ws}/${id}`] || null : null;
  }
  function saveView(ws, id, v) {
    const all = prefs.get('canvas.views', null) || {};
    const key = `${ws}/${id}`;
    delete all[key];
    all[key] = v;
    const keys = Object.keys(all);
    for (const old of keys.slice(0, Math.max(0, keys.length - VIEWS_KEPT))) delete all[old];
    prefs.set('canvas.views', all);
  }
  function forgetView(ws, id) {
    const all = prefs.get('canvas.views', null);
    if (!all?.[`${ws}/${id}`]) return;
    delete all[`${ws}/${id}`];
    prefs.set('canvas.views', all);
  }

  // ---------------------------------------------------------------- undo

  let undoTimer = 0;
  function showUndo(text) {
    clearTimeout(undoTimer);
    undoText.textContent = text;
    undoBar.hidden = false;
    undoTimer = setTimeout(hideUndo, UNDO_FOR);
  }
  function hideUndo() {
    clearTimeout(undoTimer);
    undoBar.hidden = true;
  }
  cockpit.onDispose(() => clearTimeout(undoTimer));

  async function undo() {
    const m = ui.model;
    if (!m || m.readOnly) return;
    if (!(await m.undo())) cockpit.toast('Nothing to undo', 'info', 'canvas-undo');
  }
  async function redo() {
    const m = ui.model;
    if (!m || m.readOnly) return;
    if (!(await m.redo())) cockpit.toast('Nothing to do again', 'info', 'canvas-undo');
  }

  // ---------------------------------------------------------------- the canvas itself

  function renameCanvas() {
    const m = ui.model;
    if (!m || m.readOnly || m.deleted || crumbs.querySelector('.rename')) return;
    const input = h('input', { class: 'rename', type: 'text', maxlength: '120', spellcheck: 'false', 'aria-label': 'The canvas\'s title' });
    input.value = m.doc.title || '';
    let done = false;
    const finish = (save) => {
      if (done) return;
      done = true;
      const value = input.value.trim();
      input.remove();
      titleEl.hidden = false;
      if (save && value && value !== m.doc.title) renameTo(value);
    };
    input.addEventListener('keydown', (event) => {
      event.stopPropagation();
      if (event.key === 'Enter') {
        event.preventDefault();
        finish(true);
        ui.surface?.focus();
      } else if (event.key === 'Escape') {
        event.preventDefault();
        finish(false);
        ui.surface?.focus();
      }
    });
    input.addEventListener('blur', () => finish(true));
    titleEl.hidden = true;
    titleEl.after(input);
    input.focus();
    input.select();
  }

  // renameTo titles the canvas: one not saved yet keeps the title for
  // when it is.
  async function renameTo(title) {
    const m = ui.model;
    if (!m || !title) return;
    if (!m.exists) {
      m.doc.title = title;
      drawAll();
      return;
    }
    try {
      await m.apply([{ op: 'canvas.update', set: { title } }], { label: 'Rename the canvas' });
    } catch (error) {
      cockpit.toast(`It was not renamed: ${error.message}`, 'error');
    }
  }

  async function setLive(live) {
    const m = ui.model;
    if (!m || !m.exists || m.readOnly) return;
    try {
      await m.apply([{ op: 'canvas.update', set: { live } }], { label: live ? 'Make it live' : 'Pause it' });
      pollLive();
    } catch (error) {
      cockpit.toast(error.message, 'error');
    }
  }

  function zoomMenu(anchor) {
    const s = ui.surface;
    if (!s) return;
    service('menu')?.toggle?.(anchor, () => [
      { icon: '+', label: 'Zoom in', hint: `${mod}=`, run: () => s.zoomBy(1) },
      { icon: '−', label: 'Zoom out', hint: `${mod}−`, run: () => s.zoomBy(-1) },
      { icon: '1', label: 'Zoom to 100%', hint: `${mod}1`, detail: 'The selection at its size', run: () => s.zoom100() },
      { icon: '⤢', label: 'Fit everything', hint: `${mod}0`, run: () => s.fit() },
    ], COMPACT);
  }

  function canvasActions() {
    const m = ui.model;
    if (!m) return [];
    const saved = m.exists && !m.deleted;
    const editable = saved && !m.readOnly;
    const agents = m.doc.nodes.some((n) => n.kind === 'agent' && n.runtime?.session);
    const ws = m.ws;
    const id = m.id;
    return [
      { icon: '⚙', label: 'Canvas settings', detail: 'Live, what agents may do, the limits of messages', run: () => inspect({ canvas: true }) },
      !m.readOnly && !m.deleted ? { icon: '✎', label: 'Rename', hint: 'F2', run: () => renameCanvas() } : null,
      saved ? { icon: '◧', label: 'Save as template…', detail: 'Its nodes and edges, for other canvases to start from', run: () => saveTemplate() } : null,
      saved ? { icon: '⤓', label: 'Export as JSON', detail: 'Its nodes and edges, without what runs', run: () => exportJSON() } : null,
      saved ? { icon: '⧉', label: 'Copy the link', run: () => cockpit.copy(`${location.origin}${location.pathname}${canvasHash(ws, id)}`, 'Link copied') } : null,
      { icon: '↗', label: 'Pop out', detail: 'In a window of its own', run: () => popOut(ws, id) },
      editable ? { separator: true } : null,
      editable ? { icon: '×', label: 'Delete the canvas', detail: agents ? 'Its shells end; its agents\' sessions stay' : 'Its shells end', danger: true, confirm: 'Delete the canvas?', run: () => deleteCanvas(ws, id, { sessions: false }) } : null,
      editable && agents ? { icon: '×', label: 'Delete it and its agents\' sessions', danger: true, confirm: 'Delete the sessions too?', run: () => deleteCanvas(ws, id, { sessions: true }) } : null,
    ].filter(Boolean);
  }

  // canvasMenu is what a canvas's row offers, in the rail and the session
  // list.
  function canvasMenu(ws, c) {
    return [
      { icon: '◧', label: 'Open', run: () => go(ws, c.id) },
      { icon: '✎', label: 'Rename…', run: () => renameRow(ws, c) },
      { icon: '↗', label: 'Pop out', detail: 'In a window of its own', run: () => popOut(ws, c.id) },
      { separator: true },
      { icon: '×', label: 'Delete the canvas', detail: c.agents ? 'Its shells end; its agents\' sessions stay' : 'Its shells end', danger: true, confirm: 'Delete the canvas?', run: () => deleteCanvas(ws, c.id, { sessions: false }) },
      c.agents ? { icon: '×', label: 'Delete it and its agents\' sessions', danger: true, confirm: 'Delete the sessions too?', run: () => deleteCanvas(ws, c.id, { sessions: true }) } : null,
    ].filter(Boolean);
  }

  async function renameRow(ws, c) {
    if (ui.model && ui.ws === ws && ui.id === c.id && shown()) {
      renameCanvas();
      return;
    }
    const values = await ask({
      title: 'Rename the canvas', submit: 'Rename',
      schema: { type: 'object', required: ['title'], properties: { title: { type: 'string', title: 'Title' } } },
      values: { title: c.title || '' },
    });
    if (!values?.title || values.title === c.title) return;
    try {
      await cockpit.api(cockpit.wsPath(ws, `/canvases/${encodeURIComponent(c.id)}`), { method: 'PATCH', body: { title: values.title } });
      refreshList(ws, { force: true });
    } catch (error) {
      cockpit.toast(`It was not renamed: ${error.message}`, 'error');
    }
  }

  async function deleteCanvas(ws, id, { sessions = false } = {}) {
    try {
      await cockpit.api(cockpit.wsPath(ws, `/canvases/${encodeURIComponent(id)}${sessions ? '' : '?keep_sessions=1'}`), { method: 'DELETE' });
    } catch (error) {
      cockpit.toast(`It was not deleted: ${error.message}`, 'error');
      return;
    }
    cockpit.toast(sessions ? 'Canvas deleted, with its agents\' sessions' : 'Canvas deleted');
    if (prefs.get(`canvas.last.${ws}`, '') === id) prefs.set(`canvas.last.${ws}`, null);
    forgetView(ws, id);
    if (ui.ws === ws && ui.id === id) {
      close();
      if (shown()) backToSessions();
    }
    await refreshList(ws, { force: true });
    session.refreshSessions?.();
    pollLive();
  }

  function popOut(ws, id) {
    const url = `${location.origin}${location.pathname}${canvasHash(ws, id)}`;
    const opened = window.open(url, `kou-canvas-${id}`, 'popup,width=1360,height=880');
    if (!opened) cockpit.toast('The browser did not open the window: allow pop-ups for this page.', 'error');
  }

  async function saveTemplate() {
    const m = ui.model;
    if (!m?.exists) return;
    const values = await ask({
      title: 'Save as template', submit: 'Save',
      note: 'Its nodes, their settings and its edges go to .harness/canvas-templates in the workspace — not what runs, nor its messages. An empty canvas offers it then.',
      schema: {
        type: 'object', required: ['name'],
        properties: {
          name: { type: 'string', title: 'Name', pattern: '^[A-Za-z0-9][A-Za-z0-9 ._-]{0,63}$', description: 'Letters, digits, spaces, dots, dashes and underscores.' },
          description: { type: 'string', title: 'What it is for' },
        },
      },
      values: { name: m.doc.title || '' },
    });
    if (!values?.name) return;
    try {
      const saved = await m.saveTemplate(values.name, values.description || '');
      cockpit.toast(`Saved as the template «${saved?.title || values.name}»`);
      loadTemplates(m.ws, { force: true });
    } catch (error) {
      cockpit.toast(`It was not saved: ${error.message}`, 'error');
    }
  }

  function exportJSON() {
    const m = ui.model;
    if (!m) return;
    const doc = structuredClone(m.doc);
    for (const n of doc.nodes || []) delete n.runtime;
    const name = (doc.title || 'canvas').replace(/[^A-Za-z0-9._-]+/g, '-').replace(/^-+|-+$/g, '') || 'canvas';
    const url = URL.createObjectURL(new Blob([`${JSON.stringify(doc, null, 2)}\n`], { type: 'application/json' }));
    const link = h('a', { href: url, download: `${name}.canvas.json` });
    document.body.append(link);
    link.click();
    link.remove();
    setTimeout(() => URL.revokeObjectURL(url), 2000);
  }

  // ---------------------------------------------------------------- adding nodes

  // The titles nodes start with: a kind's, numbered past those taken.
  function freeTitle(base) {
    const taken = new Set((ui.model?.doc.nodes || []).map((n) => (n.title || '').toLowerCase()));
    if (!taken.has(base.toLowerCase())) return base;
    for (let n = 2; ; n++) if (!taken.has(`${base} ${n}`.toLowerCase())) return `${base} ${n}`;
  }

  // addItems is the + Add menu: what can be put on the canvas, at a point
  // of the world or in the middle; wired to a port, only what fits it.
  function addItems(at, connect) {
    const m = ui.model;
    if (!m || m.readOnly || m.deleted) return [];
    const takesInput = !!connect?.from;
    const givesOutput = !!connect?.to;
    const items = [];
    const group = (list) => {
      if (!list.length) return;
      if (items.length) items.push({ separator: true });
      items.push(...list);
    };
    group(kinds.harnesses().map((def) => ({
      icon: def.icon || '▣', label: def.title || def.id,
      detail: def.installed === false ? `${def.program || def.id} is not installed here` : def.description || '',
      run: () => addNode({ kind: 'terminal', preset: def.id, plugin: def.plugin || '' }, at, { connect }),
    })));
    group([{
      icon: '◆', label: 'kou agent', detail: 'An agent of this cockpit, with a session of its own',
      run: () => addNode({ kind: 'agent' }, at, { connect }),
    }]);
    if (!takesInput) {
      group(kinds.sources().map((def) => ({
        icon: def.icon || '⚡', label: def.title || def.id,
        detail: [def.description, def.plugin ? `from ${def.plugin}` : ''].filter(Boolean).join(' · '),
        run: () => addNode({ kind: 'source', preset: def.id, plugin: def.plugin || '' }, at, { connect }),
      })));
    }
    if (!takesInput && !givesOutput) {
      group([{ icon: '¶', label: 'Note', detail: 'Markdown on the board', run: () => addNode({ kind: 'note' }, at) }]);
    }
    if (!connect) {
      const summary = { ws: m.ws, id: m.id, title: m.doc.title, nodes: m.doc.nodes.length };
      group(cockpit.contributions('canvas.add', { unique: 'id' })
        .filter((item) => !item.shown || cockpit.safely(() => item.shown(summary)))
        .map((item) => ({ icon: item.icon || '◇', label: item.title || item.id, detail: item.group || item.plugin || '', run: () => item.create?.(at) })));
    }
    return items;
  }

  function openAddMenu(anchor, at = null, options = {}) {
    const items = addItems(at, options.connect || null);
    if (items.length) service('menu')?.open?.(anchor, items, COMPACT);
  }

  // addNode puts a node on the canvas, asking first for the settings its
  // preset cannot do without.
  async function addNode(spec, at = null, { connect = null } = {}) {
    const m = ui.model;
    const s = ui.surface;
    if (!m || !s || m.readOnly) return null;
    const node = { ...spec };
    let schema = null;
    let label = '';
    if (node.kind === 'terminal') {
      const def = kinds.harness(node);
      schema = def?.config || null;
      label = (def?.title || node.preset || 'Terminal').replace(/…$/, '');
      if (def?.installed === false) cockpit.toast(`${def.program || def.title} is not installed here: the node waits for it`, 'error', 'canvas-missing');
    } else if (node.kind === 'source') {
      const def = kinds.source(node);
      schema = def?.config || null;
      label = def?.title || node.preset || 'Source';
    } else if (node.kind === 'agent') {
      label = node.preset === 'foreman' ? 'Foreman' : 'Agent';
    } else if (node.kind === 'note') {
      label = 'Note';
      node.config ??= { text: '' };
    }
    if (schema && missing(schema, node.config || {}).length) {
      const values = await ask({
        title: `${label}: settings`, submit: 'Add',
        note: 'It needs these before it can run; the inspector changes them later.',
        schema, values: node.config || {},
      });
      if (!values) return null;
      node.config = { ...(node.config || {}), ...values };
      if (node.kind === 'terminal' && node.preset === 'command' && values.command) label = String(values.command).split(/\s+/)[0].split('/').pop() || label;
    }
    node.title ||= freeTitle(label);
    try {
      ui.galleryClosed = true;
      const id = await s.add(node, at, { connect, label: `Add «${node.title}»` });
      scheduleDraw();
      return id;
    } catch (error) {
      cockpit.toast(`It was not added: ${error.message}`, 'error');
      return null;
    }
  }

  // ---------------------------------------------------------------- the dialog that asks

  const askTitle = h('span', { class: 'label' });
  const askBody = h('div', { class: 'settings-body' });
  const askFoot = h('footer');
  const askBox = h('form', {
    class: 'settings cv-ask ticks', role: 'dialog', 'aria-modal': 'true', novalidate: true,
    onsubmit: (event) => {
      event.preventDefault();
      askSubmit?.();
    },
  }, h('header', null, askTitle, h('button', { class: 'icon', type: 'button', 'aria-label': 'Close', onclick: () => closeAsk(null) }, '×')), askBody, askFoot);
  const askOverlay = h('div', { class: 'overlay', hidden: true, onmousedown: (event) => { if (event.target === askOverlay) closeAsk(null); } }, askBox);
  let askResolve = null;
  let askSubmit = null;
  let askFocus = null;

  // ask shows a form made from a schema; it comes to the values given, or
  // null if it is closed.
  function ask({ title, note = '', schema, values = {}, submit = 'OK' }) {
    closeAsk(null);
    askFocus = document.activeElement;
    return new Promise((resolve) => {
      askResolve = resolve;
      const f = form(h, schema, values, { idPrefix: 'cv-ask' });
      const status = h('p', { class: 'settings-status', data: { kind: 'error' } });
      askTitle.textContent = title;
      askBody.replaceChildren(...[note ? h('p', { class: 'settings-note', text: note }) : null, f.node, status].filter(Boolean));
      askFoot.replaceChildren(h('span', { class: 'settings-path' }),
        h('button', { class: 'act', type: 'button', onclick: () => closeAsk(null) }, 'Cancel'),
        h('button', { class: 'primary', type: 'submit' }, submit));
      askSubmit = () => {
        if (!f.valid()) {
          status.textContent = 'Fill in what is marked *, as it asks.';
          for (const field of f.node.querySelectorAll('.cv-field')) {
            const input = field.querySelector('input, select, textarea');
            if (input) input.dispatchEvent(new Event('change'));
          }
          return;
        }
        const done = askResolve;
        askResolve = null;
        hideAsk();
        done?.(f.values());
      };
      askOverlay.hidden = false;
      requestAnimationFrame(() => f.focus());
    });
  }

  function hideAsk() {
    askOverlay.hidden = true;
    askSubmit = null;
    const back = askFocus;
    askFocus = null;
    if (back?.isConnected && back !== document.body) back.focus?.({ preventScroll: true });
    else if (shown()) ui.surface?.focus();
  }

  function closeAsk(value) {
    if (askOverlay.hidden && !askResolve) return;
    const done = askResolve;
    askResolve = null;
    hideAsk();
    done?.(value);
  }

  // ---------------------------------------------------------------- live canvases, in the rail's foot

  const liveText = h('span');
  const liveBadge = h('button', {
    class: 'cv-live-badge', type: 'button', hidden: true, 'aria-haspopup': 'menu', 'aria-expanded': 'false',
    onclick: (event) => service('menu')?.toggle?.(event.currentTarget, () => liveItems(), COMPACT),
  }, h('i', { 'aria-hidden': 'true' }), liveText);
  let liveNow = { live: 0, canvases: [] };

  async function pollLive() {
    if (!enabled() || document.visibilityState !== 'visible') return;
    try {
      liveNow = (await cockpit.api('/api/canvas/live')) || { live: 0, canvases: [] };
    } catch {
      return;
    }
    drawLive();
  }

  function drawLive() {
    const n = enabled() ? liveNow.live || 0 : 0;
    liveBadge.hidden = !n;
    if (!n) return;
    const busy = (liveNow.canvases || []).reduce((sum, c) => sum + (c.busy || 0), 0);
    const waiting = (liveNow.canvases || []).reduce((sum, c) => sum + (c.waiting || 0), 0);
    liveText.textContent = `◧ ${n}`;
    liveBadge.dataset.busy = busy ? 'true' : '';
    liveBadge.title = `${n} canvas${n === 1 ? '' : 'es'} live${busy ? ` · ${busy} busy` : ''}${waiting ? ` · ${waiting} waiting` : ''}: their sources run`;
    liveBadge.setAttribute('aria-label', liveBadge.title);
  }

  function liveItems() {
    const names = new Map((state().workspaces || []).map((w) => [w.id, w.name]));
    const items = (liveNow.canvases || []).map((c) => ({
      icon: '◧', label: c.title || 'Canvas',
      detail: [names.get(c.workspace) || '', c.busy ? `${c.busy} busy` : '', c.waiting ? `${c.waiting} waiting` : ''].filter(Boolean).join(' · '),
      run: () => go(c.workspace, c.id),
    }));
    if (items.length) items.push({ separator: true });
    items.push({ icon: '⏸', label: 'Pause all canvases', detail: 'Their sources stop, their messages wait', run: () => pauseAll() });
    return items;
  }

  async function pauseAll() {
    try {
      const result = await cockpit.api('/api/canvas/pause', { method: 'POST', body: {} });
      const n = result?.paused ?? 0;
      cockpit.toast(n ? `${n} canvas${n === 1 ? '' : 'es'} paused` : 'No canvas was live');
    } catch (error) {
      cockpit.toast(error.message, 'error');
    }
    pollLive();
  }
  cockpit.interval(pollLive, 15000);
  cockpit.listen(document, 'visibilitychange', () => {
    if (document.visibilityState === 'visible') pollLive();
  });

  // ---------------------------------------------------------------- an agent's session leads back

  const crumbLink = h('a', {
    class: 'cv-crumb', hidden: true,
    onclick: (event) => {
      if (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
      event.preventDefault();
      const v = session.view?.();
      const link = v && links.get(v);
      if (!link) return;
      ui.revealAfter = link.node || '';
      go(v.ws, link.canvas);
      if (ui.model && !ui.model.loading) revealPending();
    },
  });
  const links = new WeakMap(); // a session's view → { canvas, node }
  cockpit.on('session:loaded', (v, data) => {
    if (v && data?.canvas) links.set(v, { canvas: data.canvas, node: data.node || '' });
    drawCrumb();
    if (v && data?.canvas && !lists.has(v.ws)) refreshList(v.ws);
  });
  function drawCrumb() {
    const v = session.view?.();
    const link = enabled() && v && !v.fresh ? links.get(v) : null;
    crumbLink.hidden = !link;
    if (!link) return;
    const c = canvasesOf(v.ws).find((x) => x.id === link.canvas);
    const title = c?.title || 'Canvas';
    crumbLink.textContent = `◧ ${title}`;
    crumbLink.href = canvasHash(v.ws, link.canvas);
    crumbLink.title = `This session is the agent of a node on the canvas «${title}»: back to it`;
  }

  // ---------------------------------------------------------------- what is on only while the server runs canvases

  const viewItem = {
    id: 'canvas', title: 'Canvas', order: 15, rail: rail.element, page,
    tabTitle: 'Canvases: terminals, agents and events, wired together',
    select: () => openLast(),
    shown: () => {
      ui.surface?.shown();
      drawAll();
      refreshList();
      pollLive();
    },
    hidden: () => {
      ui.surface?.hidden();
      closeAsk(null);
    },
  };
  const rowsItem = {
    id: 'canvas', order: 10,
    rows: (ws) => canvasesOf(ws).map((c) => ({
      id: c.id, glyph: '◧', title: c.title || 'Canvas', meta: metaOf(c), at: c.updated_at,
      href: canvasHash(ws, c.id), current: shown() && ui.ws === ws && ui.id === c.id, running: c.busy > 0,
      open: () => go(ws, c.id),
      menu: () => canvasMenu(ws, c),
    })),
  };
  let gated = [];
  let on = false;
  function setEnabled(next) {
    if (next === on) return;
    on = next;
    for (const off of gated) off();
    gated = [];
    if (on) {
      gated = [
        cockpit.contribute('layout.view', viewItem),
        cockpit.contribute('session-list.rows', rowsItem),
      ];
      refreshList(state().ws, { force: true });
      pollLive();
    } else {
      if (shown()) backToSessions();
      close();
      lists.clear();
    }
    drawLive();
    drawCrumb();
    cockpit.render();
  }

  cockpit.on('session:config', () => setEnabled(enabled()));
  cockpit.on('session:workspace', (ws) => {
    if (ui.model && ui.ws !== ws) close();
    refreshList(ws);
  });
  let sessionsAt = 0;
  cockpit.on('session:sessions', () => {
    // The sessions' list is read again now and then: the canvases' too.
    if (Date.now() - sessionsAt < 5000) return;
    sessionsAt = Date.now();
    refreshList(state().ws);
  });
  cockpit.on('session:view', drawCrumb);
  cockpit.on('point:canvas.template', () => {
    gallery.dataset.key = '';
    scheduleDraw();
  });
  cockpit.on('plugins', () => {
    // Plugins came or went: their presets, sources and templates with them.
    kindsByWs.clear();
    templatesByWs.clear();
    if (!ui.ws) return;
    const ws = ui.ws;
    loadKinds(ws, { force: true }).then(() => {
      if (ui.ws !== ws) return;
      ui.surface?.sync();
      refreshInspector();
    });
    loadTemplates(ws, { force: true });
  });
  cockpit.on('render', () => {
    const v = session.view?.();
    const hide = !(on && shown() === false && cockpit.store.get('view') === 'sessions' && v?.fresh && state().ws);
    if (modeSwitch.hidden !== hide) modeSwitch.hidden = hide;
  });

  cockpit.ui.mount('bar.crumbs', { id: 'session-mode', order: 5, node: modeSwitch });
  cockpit.ui.mount('bar.crumbs', { id: 'canvas-crumb', order: 6, node: crumbLink });
  cockpit.ui.mount('rail.foot', { id: 'canvas-live', order: 20, node: liveBadge });
  cockpit.ui.mount('overlays', { id: 'canvas-ask', order: 35, node: askOverlay });
  cockpit.contribute('overlay', { id: 'canvas-ask', order: 35, modal: true, isOpen: () => !askOverlay.hidden, close: () => closeAsk(null) });
  cockpit.contribute('overlay', { id: 'canvas-inspector', order: 50, modal: false, isOpen: () => shown() && inspector.isOpen(), close: () => inspector.hide() });

  cockpit.routes.register({ id: 'canvas', priority: 10, match: (hash) => ROUTE.exec(hash), enter: (match) => routeCanvas(match[1], match[2]) });

  // ---------------------------------------------------------------- keys

  const surface = () => (shown() ? ui.surface : null);
  // The board has the keys: the focus is on it, or nowhere.
  const boardKeys = () => {
    const active = document.activeElement;
    return !!surface() && (!active || active === document.body || active === ui.surface.element);
  };
  const keys = [
    { key: 'Mod+0', run: () => surface()?.fit() },
    { key: 'Mod+1', run: () => surface()?.zoom100() },
    { key: ['Mod+=', 'Mod++'], run: () => surface()?.zoomBy(1) },
    { key: ['Mod+-', 'Mod+_'], run: () => surface()?.zoomBy(-1) },
    {
      key: 'Mod+z',
      when: () => !!surface() && !page.querySelector('.cv-rename, .rename'),
      run: (event) => (event.shiftKey ? redo() : undo()),
    },
    { key: 'Mod+y', when: () => !!surface(), run: () => redo() },
    {
      key: ['a', 'ф'], when: boardKeys, repeat: false,
      run: () => {
        const s = surface();
        if (!ui.model || ui.model.readOnly) return false;
        const p = s.pointerWorld();
        openAddMenu(s.placeAnchor(p.client.x, p.client.y), p.at);
        return undefined;
      },
    },
    { key: ['Delete', 'Backspace'], when: boardKeys, run: () => (surface().selected().length || surface().selectedEdge() ? surface().removeSelected() : false) },
    { key: ['Mod+d', 'Mod+в'], when: boardKeys, repeat: false, run: () => (surface().selected().length ? surface().duplicateSelected() : false) },
    { key: 'Enter', when: boardKeys, run: () => (surface().selected().length === 1 ? surface().enter() : false) },
    { key: 'Tab', when: boardKeys, run: () => surface().next(1) },
    { key: 'Shift+Tab', when: boardKeys, run: () => surface().next(-1) },
    { key: ['f', 'а'], when: boardKeys, run: () => (surface().selected().length === 1 ? surface().focusMode(surface().selected()[0]) : false) },
    {
      key: 'F2', when: () => !!surface(),
      run: () => {
        const s = surface();
        if (s.selected().length === 1) s.rename(s.selected()[0]);
        else renameCanvas();
      },
    },
    {
      key: 'Escape', when: boardKeys,
      run: () => {
        const s = surface();
        if (s.selected().length || s.selectedEdge()) {
          s.clear();
          return undefined;
        }
        return false;
      },
    },
  ];
  for (const spec of keys) cockpit.keys.register({ views: ['canvas'], priority: 70, ...spec });
  // ⌘Esc takes the keys back from a node — from a shell too, whose keys
  // are its own.
  cockpit.keys.register({
    key: 'Mod+Escape', global: true, own: true, priority: 70,
    when: () => !!surface() && (!!ui.surface.entered() || ui.surface.contains(document.activeElement)),
    run: () => ui.surface.exit(),
  });
  const helpKeys = [
    [['Space', 'drag'], 'Canvas: pan the board (or the wheel; ⌘ and the wheel zoom)'],
    [[mac ? '⌘' : 'Ctrl', '0'], 'Canvas: fit everything · ⌘1 the selection at 100% · ⌘= ⌘− zoom'],
    [['A'], 'Canvas: add a node where the pointer is (or double-click the board)'],
    [['⏎'], 'Canvas: enter the node selected · ⌘Esc gives the keys back'],
    [['Tab'], 'Canvas: the next node · ⇧Tab the one before'],
    [['F'], 'Canvas: the node selected at 100%'],
    [[mac ? '⌘' : 'Ctrl', 'D'], 'Canvas: duplicate what is selected'],
    [['⌫'], 'Canvas: delete what is selected (it can be undone)'],
    [[mac ? '⌘' : 'Ctrl', 'Z'], 'Canvas: undo · ⌘⇧Z do it again'],
  ];
  helpKeys.forEach(([list, text], n) => cockpit.contribute('help.keys', { keys: list, text, order: 300 + n }));

  // ---------------------------------------------------------------- palette, command

  cockpit.contribute('palette.provider', {
    id: 'canvas', order: 170,
    items: () => {
      if (!on || !state().ws) return [];
      const ws = state().ws;
      const items = [{ group: 'Canvas', icon: '◧', label: 'New canvas', detail: 'Terminals, agents and events, wired together', order: 170, run: () => createCanvas() }];
      for (const c of canvasesOf(ws).slice(0, 20)) {
        if (shown() && ui.id === c.id) continue;
        items.push({ group: 'Canvas', icon: '◧', label: `Canvas: ${c.title || 'Canvas'}`, detail: metaOf(c), order: 171, run: () => go(ws, c.id) });
      }
      const m = shown() ? ui.model : null;
      if (m && !m.deleted) {
        items.push({ group: 'Canvas', icon: '+', label: 'Add a node…', hint: 'A', order: 172, run: () => addButton.click() });
        items.push({ group: 'Canvas', icon: '◈', label: 'Ask the foreman…', order: 172.5, run: () => foreman.focus() });
        items.push({ group: 'Canvas', icon: '⤢', label: 'Fit everything', hint: `${mod}0`, order: 173, run: () => ui.surface?.fit() });
        if (agents.count()) {
          items.push({
            group: 'Canvas', icon: '✻', label: agents.shown() ? 'Hide the agents\' window' : 'Show the agents\' window',
            detail: `${agents.count()} agent${agents.count() === 1 ? '' : 's'} on the canvas`, order: 173.5, run: () => agents.toggle(),
          });
        }
        if (m.exists && !m.readOnly) {
          items.push(m.doc.live !== false
            ? { group: 'Canvas', icon: '⏸', label: 'Pause this canvas', detail: 'Its sources stop, its messages wait', order: 174, run: () => setLive(false) }
            : { group: 'Canvas', icon: '▶', label: 'Make this canvas live', order: 174, run: () => setLive(true) });
          items.push({ group: 'Canvas', icon: '◧', label: 'Save the canvas as a template…', order: 175, run: () => saveTemplate() });
        }
        items.push({ group: 'Canvas', icon: '⚙', label: 'Canvas settings', order: 176, run: () => inspect({ canvas: true }) });
      }
      if (liveNow.live) items.push({ group: 'Canvas', icon: '⏸', label: 'Pause all canvases', detail: `${liveNow.live} live`, order: 179, run: () => pauseAll() });
      return items;
    },
  });

  cockpit.commands.register({
    name: 'canvas', args: '[name]', order: 165,
    help: 'Open a canvas of the workspace by name, or a new one',
    shown: () => on,
    complete: () => canvasesOf(state().ws).map((c) => ({ value: c.title || c.id, label: c.title || c.id, detail: metaOf(c) })),
    run: (arg) => {
      if (!on) return cockpit.toast('This server runs no canvases (kou-conveyor-web -canvas=off).', 'error', 'canvas-off');
      const ws = state().ws;
      const name = String(arg || '').trim();
      if (!name) return createCanvas();
      const list = canvasesOf(ws);
      const lower = name.toLowerCase();
      const exact = list.find((c) => c.id === name || (c.title || '').toLowerCase() === lower);
      const found = exact ? [exact] : list.filter((c) => (c.title || '').toLowerCase().includes(lower) || c.id.startsWith(name));
      if (found.length === 1) return go(ws, found[0].id);
      if (found.length > 1) return cockpit.toast(`${found.length} canvases match "${name}": pick one in the rail`, 'error');
      return createCanvas({ title: name });
    },
  });

  // ---------------------------------------------------------------- what other plugins use

  cockpit.provide('canvas', {
    open: (id, ws = state().ws) => go(ws, String(id)),
    create: (options = {}) => createCanvas(options),
    current: () => (ui.model ? {
      ws: ui.ws, id: ui.id, title: ui.model.doc.title, exists: ui.model.exists, live: ui.model.doc.live !== false,
      nodes: ui.model.doc.nodes.map((n) => ({ id: n.id, kind: n.kind, preset: n.preset || '', plugin: n.plugin || '', title: n.title })),
      selected: ui.surface?.selected() || [],
    } : null),
    addNode: (spec, at = null) => addNode({ ...spec }, at),
    select: (ids) => ui.surface?.select(Array.isArray(ids) ? ids : [ids]),
    focusNode: (id) => ui.surface?.focusMode(id),
    fit: () => ui.surface?.fit(),
  });

  // ---------------------------------------------------------------- start

  setEnabled(enabled());
  drawAll();
  // A version loaded anew picks the canvas in view up again.
  const match = ROUTE.exec(location.hash);
  if (match && state().configured && on && (hot.current || cockpit.store.get('view') === 'canvas')) routeCanvas(match[1], match[2]);
}
