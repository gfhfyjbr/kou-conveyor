// header: the stage's bar — the workspace and the session's title, its pin
// and ID (bar.crumbs), and on the right (bar.end) the connection's and the
// stream's badges, the run's state with its clock, and the session's
// actions. It keeps the document's title too.
const DOTS = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3 8h1.5M7.25 8h1.5M11.5 8H13" stroke="currentColor" stroke-width="2"/></svg>';

export default function activate(cockpit) {
  const { h, svg, fmt } = cockpit;
  const session = cockpit.use('session');
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const view = () => session.view?.();

  const wsCrumb = h('button', {
    class: 'crumb-ws', id: 'crumb-ws', type: 'button', title: 'Switch workspace (W)',
    onclick: (event) => service('workspaces')?.openMenu?.(event.currentTarget),
  }, 'workspace');
  const title = h('strong', {
    id: 'crumb-title', title: 'Rename (F2)',
    onclick: () => { const v = view(); if (v && !v.fresh) session.beginRename(v.ws, v.id, 'crumb'); },
  }, 'New session');
  const pin = h('span', { class: 'tag pin-tag', id: 'crumb-pin', hidden: true, text: 'Pinned' });
  const id = h('code', { id: 'crumb-id' });
  const crumbs = h('div', { class: 'crumbs' }, wsCrumb, h('i', { text: '/' }), title, pin, id);
  cockpit.ui.mount('bar.crumbs', { id: 'crumbs', order: 0, node: crumbs });

  const offline = h('span', { class: 'badge offline', id: 'link-state', hidden: true, text: 'Offline' });
  const stream = h('span', { class: 'badge', id: 'stream-state', data: { state: 'idle' }, hidden: true });
  const word = h('b', { id: 'run-word', text: 'Idle' });
  const clock = h('time', { id: 'run-clock' });
  const chip = h('span', { class: 'run-state', id: 'run-state', data: { state: 'idle' }, 'aria-live': 'polite' }, h('i', { 'aria-hidden': 'true' }), word, clock);
  const actions = h('button', {
    class: 'icon', id: 'session-actions', type: 'button', title: 'Session actions', 'aria-label': 'Session actions', 'aria-haspopup': 'menu', 'aria-expanded': 'false',
    onclick: (event) => {
      const v = view();
      if (!v || v.fresh) return;
      service('menu')?.toggle?.(event.currentTarget, () => session.menuItems(v.ws, v.id));
    },
  }, svg(DOTS));
  cockpit.ui.mount('bar.end', { id: 'link-state', order: 10, node: offline });
  cockpit.ui.mount('bar.end', { id: 'stream-state', order: 20, node: stream });
  cockpit.ui.mount('bar.end', { id: 'run-state', order: 30, node: chip });
  cockpit.ui.mount('bar.end', { id: 'session-actions', order: 40, node: actions });

  let online = true;
  cockpit.on('online', (value) => {
    online = value;
    offline.hidden = online;
  });

  function render() {
    const v = view();
    if (!v) return;
    const state = session.state;
    const phase = session.runPhase(v);
    const text = v.title || session.firstPrompt(v) || (v.fresh ? 'New session' : 'Untitled session');
    const w = session.currentWorkspace();
    wsCrumb.textContent = w?.name || state.config?.workspace_name || 'workspace';
    title.textContent = text;
    title.title = v.fresh ? text : `${text} — click to rename (F2)`;
    title.dataset.editable = v.fresh ? 'false' : 'true';
    pin.hidden = !v.pinned;
    id.textContent = v.fresh ? 'unsaved' : v.id.slice(0, 8);
    actions.disabled = v.fresh || v.gone;

    chip.dataset.state = phase;
    chip.title = phase === 'external' ? 'Running in another window or terminal; this view follows it' : '';
    word.textContent = session.PHASE_LABEL[phase] || phase;
    clock.textContent = v.run ? fmt.timer(Date.now() - v.run.started) : '';

    stream.dataset.state = state.stream;
    stream.hidden = state.stream === 'idle' || state.stream === 'live';
    stream.textContent = state.stream === 'reconnecting' ? 'Reconnecting' : '';

    const busy = !!v.run;
    const current = cockpit.store.get('view');
    const other = current && current !== 'sessions' && cockpit.contributions('layout.view', { unique: 'id' }).find((x) => x.id === current);
    document.title = other ? `${other.title || other.id} · kou-conveyor`
      : `${busy ? '● ' : state.unseenOutcome ? `${state.unseenOutcome === 'failed' ? '✕' : '✓'} ` : ''}${text} · kou-conveyor`;
  }
  cockpit.on('render', render);

  cockpit.keys.register({
    key: 'F2', views: ['sessions'],
    run: () => { const v = view(); if (v && !v.fresh) session.beginRename(v.ws, v.id, 'crumb'); },
  });
  cockpit.contribute('help.keys', { keys: ['F2'], text: 'Rename the session', order: 110 });
  cockpit.render();
}
