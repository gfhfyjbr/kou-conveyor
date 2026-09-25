// accounts: the Accounts view (layout.view "accounts", #/accounts): the
// gateway's connection, accounts and endpoints (gateway.js) in the stage,
// its filters in the rail, the count on its tab, and its dialogs to sign in
// and to add an endpoint. It provides the accounts service: show(),
// showConnection(), addAccount(provider), addEndpoint(kind), refresh(),
// providers().
import { createAccounts } from './gateway.js';

const MENU = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2 4h12M2 8h12M2 12h12" stroke="currentColor" stroke-width="1.4"/></svg>';
const REFRESH = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M13 8a5 5 0 1 1-1.5-3.55M13 2.5v3h-3" fill="none" stroke="currentColor" stroke-width="1.4"/></svg>';

// The sign-ins the gateway offers, and the kinds of endpoint it takes, for
// /accounts.
const PROVIDERS = [
  ['claude', 'Claude'], ['codex', 'Codex'], ['antigravity', 'Antigravity'], ['xai', 'Grok'],
  ['kimi', 'Kimi'], ['kimi-ai', 'Kimi.ai'], ['devin', 'Devin'], ['meta', 'Meta'],
];
const KINDS = [
  ['openai-compatible', 'an OpenAI-compatible endpoint'], ['anthropic', 'an Anthropic API key'],
  ['openai', 'an OpenAI API key'], ['gemini', 'a Gemini API key'], ['xai', 'an xAI API key'],
];

export default function activate(cockpit) {
  const { h, svg, prefs } = cockpit;
  const session = cockpit.use('session');
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const layout = () => service('layout');

  // ---------------------------------------------------------------- its elements

  const count = h('span', { class: 'tab-count', id: 'tab-accounts-count', data: { health: '' } });
  const addButton = h('button', { class: 'new', id: 'account-add', type: 'button', onclick: () => panel.addAccount() }, h('span', { text: 'Add account' }), h('kbd', { text: '+' }));
  const rail = h('div', { class: 'accounts-rail', id: 'accounts-rail' },
    h('div', { class: 'rail-tools' }, addButton),
    h('div', { class: 'rail-label' }, h('span', { text: 'Providers' }), h('span', { id: 'account-count', text: '0' })),
    h('nav', { class: 'filters', id: 'provider-filter', 'aria-label': 'Providers' }),
    h('div', { class: 'rail-label' }, h('span', { text: 'State' })),
    h('nav', { class: 'filters', id: 'state-filter', 'aria-label': 'States' }),
    h('div', { class: 'rail-label' }, h('span', { text: 'Endpoints' }), h('span', { id: 'endpoint-count', text: '0' })),
    h('nav', { class: 'filters', id: 'endpoint-filter', 'aria-label': 'Endpoints' }),
    h('p', { class: 'accounts-rail-note', id: 'accounts-rail-note' }));

  const range = h('span', { class: 'range', id: 'accounts-range', role: 'radiogroup', 'aria-label': 'Uptime over' },
    h('button', { type: 'button', role: 'radio', data: { range: '24h' }, 'aria-checked': 'true', title: 'Uptime over the last day (1)' }, '24h'),
    h('button', { type: 'button', role: 'radio', data: { range: '7d' }, 'aria-checked': 'false', title: 'Uptime over the last week (7)' }, '7d'));
  const addTop = h('button', {
    class: 'act strong add-top', id: 'accounts-add-top', type: 'button', title: 'Add an account or an endpoint (+)', 'aria-haspopup': 'menu', 'aria-expanded': 'false',
    onclick: (event) => panel.addMenu(event.currentTarget),
  }, '+ Add');
  const page = h('section', { class: 'accounts-page', id: 'accounts-page', 'aria-label': 'Accounts' },
    h('header', { class: 'bar' },
      h('button', { class: 'icon rail-toggle', id: 'accounts-rail-toggle', type: 'button', 'aria-label': 'Menu', onclick: () => layout()?.rail?.() }, svg(MENU)),
      h('div', { class: 'crumbs' }, h('strong', { text: 'Accounts' }), h('code', { id: 'gw-version', text: 'CLIProxyAPI' })),
      h('div', { class: 'bar-right' },
        h('span', { class: 'run-state gw-state', id: 'gw-state', data: { state: 'starting' } }, h('i', { 'aria-hidden': 'true' }), h('b', { id: 'gw-word', text: 'Starting' })),
        range,
        h('button', { class: 'icon', id: 'accounts-refresh', type: 'button', title: 'Refresh (R)', 'aria-label': 'Refresh', onclick: () => panel.refresh() }, svg(REFRESH)),
        addTop)),
    h('div', { class: 'accounts-scroll', id: 'accounts-scroll' },
      h('div', { class: 'accounts-body' },
        h('div', { id: 'gw-card' }),
        h('div', { class: 'accounts-list', id: 'accounts-list' }),
        h('div', { class: 'accounts-list', id: 'endpoints-list' }))));

  const dialog = (id, cls, title) => h('div', { class: 'overlay', id, hidden: true },
    h('div', { class: `settings ${cls} ticks`, role: 'dialog', 'aria-modal': 'true', 'aria-labelledby': `${id === 'signin' ? 'signin' : 'endpoint'}-title` },
      h('header', null, h('span', { class: 'label', id: `${id === 'signin' ? 'signin' : 'endpoint'}-title`, text: title }),
        h('button', { class: 'icon', id: `${id === 'signin' ? 'signin' : 'endpoint'}-close`, type: 'button', 'aria-label': 'Close' }, '×')),
      h('div', { class: 'settings-body', id: `${id === 'signin' ? 'signin' : 'endpoint'}-body` }),
      h('footer', { id: `${id === 'signin' ? 'signin' : 'endpoint'}-foot` })));
  const signin = dialog('signin', 'signin', 'Add account');
  const endpoint = dialog('endpoint-dialog', 'endpoint-form', 'Add endpoint');
  const files = h('input', { id: 'account-files', type: 'file', accept: '.json,application/json', multiple: true, hidden: true });
  const roots = [rail, page, signin, endpoint, files];

  // $ finds the view's own elements, in the page or not yet.
  const $ = (id) => {
    for (const root of roots) {
      if (root.id === id) return root;
      const found = root.querySelector(`#${CSS.escape(id)}`);
      if (found) return found;
    }
    return null;
  };
  const query = (selector) => {
    for (const root of roots) {
      const found = root.matches(selector) ? root : root.querySelector(selector);
      if (found) return found;
    }
    return null;
  };

  for (const [id, node, order] of [['signin', signin, 30], ['endpoint-dialog', endpoint, 31], ['account-files', files, 32]]) {
    cockpit.ui.mount('overlays', { id, order, node });
  }

  // ---------------------------------------------------------------- the view

  const panel = createAccounts({
    api: cockpit.api, toast: (text, kind, key) => cockpit.toast(text, kind, key), copy: (text, label) => cockpit.copy(text, label),
    openMenu: (anchor, items) => service('menu')?.open?.(anchor, items), $, query, prefs,
    closeOverlays: () => service('overlays')?.closeTop?.(),
    models: () => service('models'),
    // The dialog for a connection around the gateway, straight to an endpoint.
    openDirectSettings: () => service('connection')?.openDirect?.(),
    onSummary: renderCount,
    // Runs were pointed at the gateway: the connection changed.
    onConnection: () => {
      session.refreshConfig();
      service('models')?.load?.();
    },
    // The gateway serves other models: an account or endpoint came or went.
    onModels: () => service('models')?.stale?.(),
  });
  cockpit.onDispose(() => panel.destroy());

  // renderCount shows on the tab how many accounts there are, and whether
  // any needs a look.
  function renderCount(summary) {
    const on = summary && summary.state !== 'off';
    const total = on ? (summary.accounts || 0) + (summary.endpoints || 0) : 0;
    const failing = on ? (summary.failing || 0) + (summary.failing_endpoints || 0) : 0;
    if (count.textContent !== (total ? String(total) : '')) count.textContent = total ? String(total) : '';
    count.dataset.health = !on ? '' : summary.state === 'failed' || failing ? 'bad' : summary.cooling ? 'warn' : '';
    const tab = count.closest('.rail-tab');
    if (tab) {
      tab.title = !on ? 'Accounts: the gateway is off'
        : `Accounts: ${summary.accounts || 0} accounts, ${summary.endpoints || 0} endpoints · ${summary.ready || 0} ready${summary.cooling ? ` · ${summary.cooling} cooling down` : ''}${failing ? ` · ${failing} failing` : ''} (A)`;
    }
  }

  const shown = () => cockpit.store.get('view') === 'accounts';

  // show goes to the Accounts view; the sessions keep a view to come back to.
  function show() {
    if (location.hash !== '#/accounts') history.pushState(null, '', '#/accounts');
    session.ensureView?.();
    layout()?.show?.('accounts');
  }

  function back() {
    cockpit.contributions('layout.view', { unique: 'id' }).find((v) => v.id === 'sessions')?.select?.();
  }

  // showConnection shows where runs connect: the connection of this view,
  // which sends them through the gateway.
  function showConnection() {
    service('overlays')?.closeTop?.();
    show();
    panel.focusConnection();
  }

  cockpit.contribute('layout.view', {
    id: 'accounts', title: 'Accounts', order: 20, badge: count, rail, page,
    tabTitle: 'Accounts: the subscriptions runs can use (A)',
    select: show,
    shown: () => panel.open(),
    hidden: () => panel.close(),
  });
  cockpit.routes.register({ id: 'accounts', priority: 10, match: (hash) => /^#\/accounts\/?$/.test(hash), enter: () => {
    session.ensureView?.();
    layout()?.show?.('accounts');
  } });

  cockpit.listen(range, 'click', (event) => {
    const button = event.target.closest('button[data-range]');
    if (button) panel.setRange(button.dataset.range);
  });
  cockpit.listen(signin.querySelector('#signin-close'), 'click', () => panel.closeSignIn());
  cockpit.listen(endpoint.querySelector('#endpoint-close'), 'click', () => panel.closeEndpoint());
  cockpit.listen(endpoint, 'mousedown', (event) => { if (event.target === endpoint) panel.closeEndpoint(); });
  // A click beside the dialog closes it, unless a sign-in is under way there.
  cockpit.listen(signin, 'mousedown', (event) => { if (event.target === signin) panel.dismissSignIn(); });
  cockpit.listen(files, 'change', (event) => {
    const list = [...event.target.files];
    if (list.length) panel.importFiles(list);
  });
  cockpit.contribute('overlay', { id: 'signin', order: 30, modal: true, isOpen: () => !signin.hidden, close: () => panel.closeSignIn() });
  cockpit.contribute('overlay', { id: 'endpoint', order: 31, modal: true, isOpen: () => !endpoint.hidden, close: () => panel.closeEndpoint() });

  // The tab's count stays current while the view is not in sight.
  cockpit.interval(() => {
    if (!shown() && document.visibilityState === 'visible') panel.refresh({ quiet: true });
  }, 60_000);
  cockpit.listen(document, 'visibilitychange', () => panel.visibility());
  cockpit.on('session:config', (config) => {
    renderCount(config?.accounts ? { state: config.accounts.state, ...config.accounts.summary } : null);
    if (!shown()) panel.refresh({ quiet: true });
  });
  if (session.state?.configured) panel.refresh({ quiet: true });
  if (shown()) panel.open();

  // ---------------------------------------------------------------- keys, commands, palette

  cockpit.keys.register({ key: 'a', run: () => (shown() ? back() : show()) });
  for (const key of ['+', 'r', '1', '7']) cockpit.keys.register({ key, views: ['accounts'], run: (event) => (panel.key(event) ? undefined : false) });
  cockpit.contribute('help.keys', { keys: ['A'], text: 'Accounts: the gateway\'s connection, accounts and endpoints · + add one', order: 140 });

  const addAccount = (provider = '') => { show(); panel.addAccount(String(provider)); };
  const addEndpoint = (kind = '') => { show(); panel.addEndpoint(String(kind)); };
  cockpit.commands.register({
    name: 'accounts', args: '[add [provider] | endpoint [kind]]', order: 160,
    help: 'The accounts gateway: its connection, accounts and endpoints, with uptime, errors and limits',
    complete: () => [
      { value: 'add', label: 'add', detail: 'sign in with a subscription' },
      ...PROVIDERS.map(([id, name]) => ({ value: `add ${id}`, label: `add ${id}`, detail: `sign in with ${name}` })),
      { value: 'endpoint', label: 'endpoint', detail: 'add an API key: Anthropic, OpenAI, Gemini, xAI, OpenAI-compatible' },
      ...KINDS.map(([id, name]) => ({ value: `endpoint ${id}`, label: `endpoint ${id}`, detail: `add ${name}` })),
    ],
    run: (arg) => {
      const [verb, which = ''] = String(arg).split(/\s+/);
      if (!verb) return show();
      if (verb === 'add') return addAccount(which);
      if (verb === 'endpoint') return addEndpoint(which);
      return cockpit.toast('/accounts takes add or endpoint', 'error');
    },
  });
  cockpit.contribute('palette.provider', {
    id: 'accounts', order: 200,
    items: () => [
      shown()
        ? { group: 'Accounts', icon: '◧', label: 'Back to sessions', hint: 'A', order: 200, run: back }
        : { group: 'Accounts', icon: '◎', label: 'Accounts', hint: 'A', detail: 'Subscriptions runs can use: uptime, errors, limits', order: 200, run: show },
      { group: 'Accounts', icon: '+', label: 'Add account…', hint: '+', order: 201, run: () => addAccount() },
      ...panel.providers().map((p) => ({ group: 'Accounts', icon: '+', label: `Add account: ${p.name}`, detail: p.detail, order: 202, run: () => addAccount(p.id) })),
      { group: 'Accounts', icon: '↑', label: 'Import credential files…', order: 203, run: () => { show(); panel.pickFiles(); } },
      { group: 'Accounts', icon: '⌁', label: 'Add endpoint…', detail: 'An API key: Anthropic, OpenAI, Gemini, xAI, OpenAI-compatible', order: 204, run: () => addEndpoint() },
    ],
  });

  cockpit.provide('accounts', {
    show, back, showConnection, addAccount, addEndpoint,
    refresh: (options) => panel.refresh(options), providers: () => panel.providers(), pickFiles: () => panel.pickFiles(),
  });
}
