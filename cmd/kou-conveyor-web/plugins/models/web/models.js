// models: each prompt runs with a model of its own, any the connection
// reaches, of any provider — through the gateway, every model of every
// account and endpoint. The composer shows the model the next prompt of the
// session takes (composer.row): the one chosen for the session, else the
// one its last prompt ran with, else the connection's. It provides the
// models service: next(v), default(), catalog(), load({ fresh }), set(v, id),
// command(arg), toggle(), openPicker(options), closePicker(), find(id),
// provider(id), title(id, extra), contextLabel(tokens), info().
import { closeModelPicker, contextLabel, findModel, modelPickerOpen, modelTitle, openModelPicker, providerOf } from './picker.js';

const STALE = 60_000;

export default function activate(cockpit) {
  const { h, prefs } = cockpit;
  const session = cockpit.use('session');
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const view = () => session.view?.();
  const hot = cockpit.hot.data;
  const ui = hot.ui ??= { catalog: null, ws: null, at: 0, ticket: 0 };
  let loading = null;
  let picker = null;

  const state = () => session.state || {};
  const modelKey = (v) => `model.${v.ws || 'default'}.${v.id}`;
  const catalog = () => (ui.ws === state().ws ? ui.catalog : null);

  // next is the model the next prompt of v runs with; '' leaves it to the
  // connection.
  function next(v) {
    return v ? v.model || session.lastModel(v) : '';
  }

  // fallback is the connection's model, which prompts that choose none run
  // with.
  function fallback() {
    return (ui.ws === state().ws && ui.catalog?.default) || session.currentWorkspace?.()?.connection?.model || state().config?.model || '';
  }

  // load asks which models the workspace's prompts can run with. fresh asks
  // an endpoint again instead of the server's recent answer.
  function load({ fresh = false } = {}) {
    const ws = state().ws;
    const ticket = ++ui.ticket;
    loading = (async () => {
      let answer;
      try {
        answer = await cockpit.api(cockpit.wsPath(ws, `/models${fresh ? '?fresh=1' : ''}`));
      } catch (error) {
        answer = { source: '', models: [], error: error.status ? error.message : 'The server is unreachable' };
      }
      if (ticket !== ui.ticket) return ui.catalog;
      Object.assign(ui, { catalog: answer, ws, at: Date.now() });
      loading = null;
      picker?.update(answer);
      cockpit.emit('models', answer);
      cockpit.render();
      cockpit.use('timeline').schedule?.();
      return answer;
    })();
    return loading;
  }

  // want loads the models unless those loaded are recent.
  function want() {
    if (loading) return;
    if (ui.ws !== state().ws || Date.now() - ui.at > STALE) load();
  }

  // ---------------------------------------------------------------- the button

  const dot = h('i', { class: 'model-dot', id: 'model-dot', 'aria-hidden': 'true' });
  const name = h('b', { id: 'model-name', text: 'default' });
  const button = h('button', {
    class: 'model-pick', id: 'model-pick', type: 'button', 'aria-haspopup': 'dialog', 'aria-expanded': 'false', data: { source: 'default' },
    title: 'The model the next prompt runs with: any the connection reaches, of any provider (M)',
    onclick: () => toggle(),
  }, h('span', { class: 'label', text: 'Model' }), dot, name, h('span', { class: 'chev', 'aria-hidden': 'true' }));
  cockpit.ui.mount('composer.row', { id: 'model', order: 10, node: button });

  function render() {
    const v = view();
    const id = next(v) || fallback();
    const source = v?.model ? 'chosen' : session.lastModel?.(v) ? 'session' : 'default';
    const list = catalog();
    const m = findModel(list, id);
    const unlisted = !!id && !!list?.source && list.models.length > 0 && !m;
    const provider = providerOf(list, id);
    if (button.dataset.source !== source) button.dataset.source = source;
    if ((button.dataset.provider || '') !== provider) button.dataset.provider = provider;
    button.dataset.warn = unlisted || m?.cooling ? 'true' : 'false';
    const text = id ? m?.name || id : 'Runner default';
    if (name.textContent !== text) name.textContent = text;
    button.title = [
      id ? modelTitle(list, id) : 'The runner picks the model: its environment names none',
      { chosen: 'Chosen for this session', session: 'The model the last prompt of this session ran with', default: 'The connection’s model' }[source],
      unlisted ? 'The connection does not list it: runs try it anyway' : '',
      'Click or press M for another, of any provider',
    ].filter(Boolean).join('\n');
  }
  cockpit.on('render', render);

  // toggle opens the models above the composer, or closes them.
  function toggle() {
    if (modelPickerOpen()) {
      closeModelPicker();
      return;
    }
    const v = view();
    if (!v) return;
    if (cockpit.store.get('view') !== 'sessions') service('layout')?.show?.('sessions');
    const stale = ui.ws !== state().ws || Date.now() - ui.at > STALE / 2;
    picker = openModelPicker({
      anchor: button, placement: 'above', title: 'Model',
      note: v.run ? 'for the next prompts · the running agent keeps its own' : v.fresh ? 'for this new session' : 'for the next prompts of this session',
      catalog: catalog(), loading: stale || !!loading,
      current: next(v) || fallback(),
      onPick: (id) => {
        set(view(), id);
        service('composer')?.focus?.();
      },
      onRefresh: () => load({ fresh: true }),
      onConnection: () => service('connection')?.open?.(),
      onClose: () => {
        picker = null;
        button.setAttribute('aria-expanded', 'false');
      },
    });
    button.setAttribute('aria-expanded', 'true');
    if (stale && !loading) load();
  }

  // set chooses the model the session's next prompts run with.
  function set(v, id) {
    if (!v) return;
    id = String(id || '').trim();
    const before = next(v) || fallback();
    v.model = id;
    prefs.set(modelKey(v), id || null);
    cockpit.render();
    if (!id || id === before) return;
    const m = findModel(ui.catalog, id);
    const running = v.run ? ' — the running agent keeps its own model' : '';
    cockpit.toast(`Next prompts: ${m?.name || id}${m?.provider_name ? ` · ${m.provider_name}` : ''}${running}`, 'info', 'model');
  }

  // command is /model: with an ID, it chooses the model; without one, it
  // opens the models.
  function command(arg) {
    if (!arg) return toggle();
    if (/\s/.test(arg)) return cockpit.toast('A model ID has no spaces.', 'error');
    set(view(), arg);
    const list = ui.catalog;
    if (list?.source && list.models.length && !findModel(list, arg)) {
      cockpit.toast(`${arg} is not among the models the connection lists; runs try it anyway.`, 'warn', 'model');
    }
    return undefined;
  }

  const info = () => ({
    current: next(view()) || fallback(), default: fallback(),
    list: (ui.catalog?.models || []).filter((m) => !m.media).map((m) => ({
      id: m.id, name: m.name || m.id, provider: m.provider_name || '', context: m.context || 0, cooling: !!m.cooling,
    })),
  });

  cockpit.provide('models', {
    next, default: fallback, catalog, load, want, set, command, toggle, info,
    openPicker: openModelPicker, closePicker: closeModelPicker, pickerOpen: modelPickerOpen,
    find: (id) => findModel(ui.catalog, id), provider: (id) => providerOf(catalog(), id),
    title: (id, extra) => modelTitle(catalog(), id, extra), contextLabel,
    // titleIn describes a model of another list, such as the gateway's.
    titleIn: (list, id, extra) => modelTitle(list, id, extra),
    // stale has the list asked for again when it is next needed.
    stale: () => { ui.at = 0; },
  });

  cockpit.contribute('overlay', { id: 'model-picker', order: 10, modal: false, isOpen: modelPickerOpen, close: closeModelPicker });
  cockpit.onDispose(() => closeModelPicker());

  cockpit.commands.register({
    name: 'model', args: '[model]', help: 'The model the next prompts run with: any the connection reaches, of any provider; alone, the list',
    order: 140, run: (arg) => command(String(arg).trim()),
    complete: () => {
      const { current, default: byDefault, list } = info();
      return list.map((m) => ({
        value: m.id, label: m.id, current: m.id === current,
        detail: [m.provider, m.name !== m.id ? m.name : '', m.id === current ? 'current' : m.id === byDefault ? 'default' : '', m.cooling ? 'cooling down' : '']
          .filter(Boolean).join(' · '),
      }));
    },
  });
  cockpit.keys.register({ key: 'm', views: ['sessions'], run: () => toggle() });
  cockpit.contribute('help.keys', { keys: ['M'], text: 'Model of the next prompt: any the connection reaches, of any provider', order: 70 });
  cockpit.contribute('palette.provider', {
    id: 'models', order: 700,
    items: () => [
      { group: 'Model', icon: '◇', label: 'Choose the model…', hint: 'M', detail: 'Of the next prompt: any the connection reaches, of any provider', order: 700, run: () => toggle() },
      ...(ui.catalog?.models || []).filter((m) => !m.media).map((m) => ({
        group: 'Model', icon: m.id === (next(view()) || fallback()) ? '●' : '○', label: `Model: ${m.name || m.id}`,
        hint: m.provider_name || '', detail: [m.id, m.cooling ? 'cooling down' : ''].filter(Boolean).join(' · '), order: 701, run: () => set(view(), m.id),
      })),
    ],
  });

  // The list follows the workspace in view, and is looked at again when the
  // page comes back.
  cockpit.on('session:workspace', () => load());
  cockpit.on('session:config', () => load());
  cockpit.listen(document, 'visibilitychange', () => { if (document.visibilityState === 'visible') want(); });
  if (!ui.catalog && state().configured) load();
  cockpit.render();
}
