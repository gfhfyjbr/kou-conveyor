// accounts: the Accounts view (layout.view "accounts", #/accounts): the
// gateway's connection, accounts and endpoints (gateway.js) in the stage,
// its filters in the rail, the count on its tab, and its dialogs to sign in
// and to add an endpoint. Its second tab, Usage (usage.js, #/accounts/usage,
// U), is what the gateway's requests used and what that comes to as API
// credits, with the prices tokens count at. It provides the accounts
// service: show(), showConnection(), showUsage(range), addAccount(provider),
// addEndpoint(kind), refresh(), providers().
import { createAccounts } from './gateway.js';
import { money } from './format.js';
import { USAGE_RANGES, createUsage } from './usage.js';

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
  const railAccounts = h('div', { class: 'rail-pane', data: { pane: 'accounts' } },
    h('div', { class: 'rail-tools' }, addButton),
    h('div', { class: 'rail-label' }, h('span', { text: 'Providers' }), h('span', { id: 'account-count', text: '0' })),
    h('nav', { class: 'filters', id: 'provider-filter', 'aria-label': 'Providers' }),
    h('div', { class: 'rail-label' }, h('span', { text: 'State' })),
    h('nav', { class: 'filters', id: 'state-filter', 'aria-label': 'States' }),
    h('div', { class: 'rail-label' }, h('span', { text: 'Endpoints' }), h('span', { id: 'endpoint-count', text: '0' })),
    h('nav', { class: 'filters', id: 'endpoint-filter', 'aria-label': 'Endpoints' }),
    h('p', { class: 'accounts-rail-note', id: 'accounts-rail-note' }));
  const railUsage = h('div', { class: 'rail-pane', data: { pane: 'usage' }, hidden: true },
    h('div', { class: 'rail-tools' }, h('button', { class: 'new', id: 'usage-price-add', type: 'button', title: 'Set a price for a model, or a pattern of models', onclick: () => usage.openPrice() },
      h('span', { text: 'Set a price' }), h('kbd', { text: 'P' }))),
    h('div', { class: 'rail-label' }, h('span', { text: 'Providers' }), h('span', { id: 'usage-total' })),
    h('nav', { class: 'filters', id: 'usage-provider-filter', 'aria-label': 'Providers' }),
    h('div', { class: 'rail-label', id: 'usage-account-label' }, h('span', { text: 'Accounts' })),
    h('nav', { class: 'filters', id: 'usage-account-filter', 'aria-label': 'Accounts' }),
    h('div', { class: 'rail-label', id: 'usage-model-label' }, h('span', { text: 'Models' })),
    h('nav', { class: 'filters', id: 'usage-model-filter', 'aria-label': 'Models' }),
    h('p', { class: 'accounts-rail-note', id: 'usage-rail-note' }));
  const rail = h('div', { class: 'accounts-rail', id: 'accounts-rail' }, railAccounts, railUsage);

  const range = h('span', { class: 'range', id: 'accounts-range', role: 'radiogroup', 'aria-label': 'Uptime over' },
    h('button', { type: 'button', role: 'radio', data: { range: '24h' }, 'aria-checked': 'true', title: 'Uptime over the last day (1)' }, '24h'),
    h('button', { type: 'button', role: 'radio', data: { range: '7d' }, 'aria-checked': 'false', title: 'Uptime over the last week (7)' }, '7d'));
  const usageRange = h('span', { class: 'range u-range', id: 'usage-range', role: 'radiogroup', 'aria-label': 'Usage over' },
    Object.entries(USAGE_RANGES).map(([id, r]) => h('button', {
      type: 'button', role: 'radio', data: { range: id }, 'aria-checked': 'false', title: `${r.label} (${id === '24h' ? 1 : id === '7d' ? 7 : id === '30d' ? 3 : 9})`,
    }, r.short)));
  const addTop = h('button', {
    class: 'act strong add-top', id: 'accounts-add-top', type: 'button', title: 'Add an account or an endpoint (+)', 'aria-haspopup': 'menu', 'aria-expanded': 'false',
    onclick: (event) => panel.addMenu(event.currentTarget),
  }, '+ Add');
  // The tabs of the view: the accounts, and what their requests used.
  const tabCost = h('span', { class: 'tab-count page-tab-cost', id: 'tab-usage-cost' });
  const tabButton = (id, label, badge, title) => h('button', {
    class: 'page-tab', id: `accounts-tab-${id}`, type: 'button', role: 'tab', 'aria-selected': 'false', 'aria-controls': `${id}-scroll`, data: { tab: id }, title,
    onclick: () => (id === 'usage' ? showUsage() : show()),
  }, label, badge);
  const pageTabs = h('nav', { class: 'page-tabs', role: 'tablist', 'aria-label': 'Accounts' },
    tabButton('accounts', 'Accounts', null, 'The connection, the gateway, its accounts and endpoints (A)'),
    tabButton('usage', 'Usage', tabCost, 'What the requests used, and what it comes to as API credits (U)'));
  const page = h('section', { class: 'accounts-page', id: 'accounts-page', 'aria-label': 'Accounts', data: { tab: 'accounts' } },
    h('header', { class: 'bar' },
      h('button', { class: 'icon rail-toggle', id: 'accounts-rail-toggle', type: 'button', 'aria-label': 'Menu', onclick: () => layout()?.rail?.() }, svg(MENU)),
      h('div', { class: 'crumbs' }, pageTabs, h('code', { id: 'gw-version', text: 'CLIProxyAPI' })),
      h('div', { class: 'bar-right' },
        h('span', { class: 'run-state gw-state', id: 'gw-state', data: { state: 'starting' } }, h('i', { 'aria-hidden': 'true' }), h('b', { id: 'gw-word', text: 'Starting' })),
        h('span', { class: 'bar-group', data: { for: 'accounts' } }, range),
        h('span', { class: 'bar-group', data: { for: 'usage' }, hidden: true }, usageRange),
        h('button', { class: 'icon', id: 'accounts-refresh', type: 'button', title: 'Refresh (R)', 'aria-label': 'Refresh', onclick: () => (tab === 'usage' ? usage.refresh() : panel.refresh()) }, svg(REFRESH)),
        h('span', { class: 'bar-group', data: { for: 'accounts' } }, addTop))),
    h('div', { class: 'accounts-scroll', id: 'accounts-scroll', role: 'tabpanel', 'aria-labelledby': 'accounts-tab-accounts' },
      h('div', { class: 'accounts-body' },
        h('div', { id: 'gw-card' }),
        h('div', { class: 'accounts-list', id: 'accounts-list' }),
        h('div', { class: 'accounts-list', id: 'endpoints-list' }))),
    h('div', { class: 'accounts-scroll usage-scroll', id: 'usage-scroll', role: 'tabpanel', 'aria-labelledby': 'accounts-tab-usage', hidden: true },
      h('div', { class: 'accounts-body usage-body', id: 'usage-body' })));

  const dialog = (id, prefix, cls, title) => h('div', { class: 'overlay', id, hidden: true },
    h('div', { class: `settings ${cls} ticks`, role: 'dialog', 'aria-modal': 'true', 'aria-labelledby': `${prefix}-title` },
      h('header', null, h('span', { class: 'label', id: `${prefix}-title`, text: title }),
        h('button', { class: 'icon', id: `${prefix}-close`, type: 'button', 'aria-label': 'Close' }, '×')),
      h('div', { class: 'settings-body', id: `${prefix}-body` }),
      h('footer', { id: `${prefix}-foot` })));
  const signin = dialog('signin', 'signin', 'signin', 'Add account');
  const endpoint = dialog('endpoint-dialog', 'endpoint', 'endpoint-form', 'Add endpoint');
  const price = dialog('price-dialog', 'price', 'price-form', 'Price');
  const files = h('input', { id: 'account-files', type: 'file', accept: '.json,application/json', multiple: true, hidden: true });
  const roots = [rail, page, signin, endpoint, price, files];

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

  for (const [id, node, order] of [['signin', signin, 30], ['endpoint-dialog', endpoint, 31], ['price-dialog', price, 33], ['account-files', files, 32]]) {
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
    showUsage: (r) => showUsage(r),
  });
  cockpit.onDispose(() => panel.destroy());

  const usage = createUsage({
    api: cockpit.api, toast: (text, kind, key) => cockpit.toast(text, kind, key), $, prefs,
    openDialog: () => {
      service('overlays')?.closeTop?.();
      price.hidden = false;
    },
    closeDialog: () => { price.hidden = true; },
    // Prices or the unit changed: the accounts' costs follow.
    onPrices: () => panel.refresh({ quiet: true }),
  });
  cockpit.onDispose(() => usage.destroy());

  // renderCount shows on the tab how many accounts there are, and whether
  // any needs a look.
  let lastUnit = null; // what amounts show in, as the latest listing said
  function renderCount(summary, unit = lastUnit) {
    lastUnit = unit || lastUnit;
    const on = summary && summary.state !== 'off';
    const cost = on ? summary.cost || 0 : 0;
    const costText = cost ? money(cost, unit) : '';
    if (tabCost.textContent !== costText) tabCost.textContent = costText;
    tabCost.parentNode.title = cost ? `Usage: the last day's requests would cost ${money(cost, unit)} as API credits (U)` : 'What the requests used, and what it comes to as API credits (U)';
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
  // tab is the view's tab in sight: "accounts" or "usage", kept.
  let tab = cockpit.hot.data.tab || (prefs.get('accounts-tab', 'accounts') === 'usage' ? 'usage' : 'accounts');

  // setTab shows a tab of the view, and has the one in sight go on.
  function setTab(next) {
    const changed = next !== tab;
    tab = next;
    cockpit.hot.data.tab = next;
    prefs.set('accounts-tab', next);
    page.dataset.tab = next;
    for (const button of pageTabs.querySelectorAll('.page-tab')) button.setAttribute('aria-selected', String(button.dataset.tab === next));
    railAccounts.hidden = next !== 'accounts';
    railUsage.hidden = next !== 'usage';
    for (const group of page.querySelectorAll('.bar-group')) group.hidden = group.dataset.for !== next;
    $('accounts-scroll').hidden = next !== 'accounts';
    $('usage-scroll').hidden = next !== 'usage';
    if (!shown()) return;
    if (next === 'usage') {
      panel.close();
      usage.open();
      // The bar's state of the gateway, and the cost on the tab.
      panel.refresh({ quiet: true });
    } else {
      usage.close();
      panel.open();
    }
    if (changed) cockpit.render();
  }

  // show goes to the Accounts view, on its accounts or its usage; the
  // sessions keep a view to come back to.
  function show(which = 'accounts') {
    const hash = which === 'usage' ? '#/accounts/usage' : '#/accounts';
    if (location.hash !== hash) history.pushState(null, '', hash);
    session.ensureView?.();
    setTab(which);
    layout()?.show?.('accounts');
  }

  // showUsage goes to the Usage tab, over a range if one is named.
  function showUsage(r = '') {
    service('overlays')?.closeTop?.();
    show('usage');
    if (USAGE_RANGES[r]) usage.setRange(r);
  }

  function back() {
    cockpit.contributions('layout.view', { unique: 'id' }).find((v) => v.id === 'sessions')?.select?.();
  }

  // showConnection shows where runs connect: the connection of this view,
  // which sends them through the gateway.
  function showConnection() {
    service('overlays')?.closeTop?.();
    show('accounts');
    panel.focusConnection();
  }

  cockpit.contribute('layout.view', {
    id: 'accounts', title: 'Accounts', order: 20, badge: count, rail, page,
    tabTitle: 'Accounts: the subscriptions runs can use (A)',
    select: () => show(tab),
    shown: () => setTab(tab),
    hidden: () => {
      panel.close();
      usage.close();
    },
  });
  cockpit.routes.register({ id: 'accounts', priority: 10, match: (hash) => hash.match(/^#\/accounts(?:\/(usage))?\/?$/), enter: (match) => {
    session.ensureView?.();
    setTab(match[1] ? 'usage' : 'accounts');
    layout()?.show?.('accounts');
  } });

  cockpit.listen(range, 'click', (event) => {
    const button = event.target.closest('button[data-range]');
    if (button) panel.setRange(button.dataset.range);
  });
  cockpit.listen(usageRange, 'click', (event) => {
    const button = event.target.closest('button[data-range]');
    if (button) usage.setRange(button.dataset.range);
  });
  cockpit.listen(price.querySelector('#price-close'), 'click', () => usage.closeDialog());
  cockpit.listen(price, 'mousedown', (event) => { if (event.target === price) usage.closeDialog(); });
  cockpit.contribute('overlay', { id: 'price', order: 33, modal: true, isOpen: () => !price.hidden, close: () => usage.closeDialog() });
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

  // The tab's count stays current while the view, or its accounts, are not
  // in sight.
  cockpit.interval(() => {
    if ((!shown() || tab !== 'accounts') && document.visibilityState === 'visible') panel.refresh({ quiet: true });
  }, 60_000);
  cockpit.listen(document, 'visibilitychange', () => {
    panel.visibility();
    usage.visibility();
  });
  cockpit.on('session:config', (config) => {
    renderCount(config?.accounts ? { state: config.accounts.state, ...config.accounts.summary } : null);
    if (!shown() || tab !== 'accounts') panel.refresh({ quiet: true });
  });
  setTab(tab);
  if (session.state?.configured || shown()) panel.refresh({ quiet: true });

  // ---------------------------------------------------------------- keys, commands, palette

  cockpit.keys.register({ key: 'a', run: () => (shown() && tab === 'accounts' ? back() : show('accounts')) });
  cockpit.keys.register({ key: 'u', run: () => (shown() && tab === 'usage' ? back() : showUsage()) });
  for (const key of ['+', 'r', '1', '7', '3', '9', 'p']) {
    cockpit.keys.register({ key, views: ['accounts'], run: (event) => ((tab === 'usage' ? usage.key(event) : panel.key(event)) ? undefined : false) });
  }
  cockpit.contribute('help.keys', { keys: ['A'], text: 'Accounts: the gateway\'s connection, accounts and endpoints · + add one', order: 140 });
  cockpit.contribute('help.keys', { keys: ['U'], text: 'Usage: tokens by model and account as API credits · 1 7 3 9 ranges · P set a price', order: 141 });

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
  cockpit.commands.register({
    name: 'usage', args: '[24h | 7d | 30d | 90d]', order: 161,
    help: 'What the gateway\'s requests used, by model and account, as API credits',
    complete: () => Object.entries(USAGE_RANGES).map(([id, r]) => ({ value: id, label: id, detail: r.label.toLowerCase() })),
    run: (arg) => {
      const r = String(arg || '').trim();
      if (r && !USAGE_RANGES[r]) return cockpit.toast('/usage takes 24h, 7d, 30d or 90d', 'error');
      return showUsage(r);
    },
  });
  cockpit.contribute('palette.provider', {
    id: 'accounts', order: 200,
    items: () => [
      shown() && tab === 'accounts'
        ? { group: 'Accounts', icon: '◧', label: 'Back to sessions', hint: 'A', order: 200, run: back }
        : { group: 'Accounts', icon: '◎', label: 'Accounts', hint: 'A', detail: 'Subscriptions runs can use: uptime, errors, limits', order: 200, run: () => show('accounts') },
      shown() && tab === 'usage'
        ? { group: 'Accounts', icon: '◧', label: 'Back to sessions', hint: 'U', order: 199, run: back }
        : { group: 'Accounts', icon: '$', label: 'Usage & API cost', hint: 'U', detail: 'Tokens by model and account, as API credits', order: 199, run: () => showUsage() },
      { group: 'Accounts', icon: '$', label: 'Set a model\'s price…', detail: 'What a million tokens of it cost as API credits', order: 205, run: () => { showUsage(); usage.openPrice(); } },
      { group: 'Accounts', icon: '+', label: 'Add account…', hint: '+', order: 201, run: () => addAccount() },
      ...panel.providers().map((p) => ({ group: 'Accounts', icon: '+', label: `Add account: ${p.name}`, detail: p.detail, order: 202, run: () => addAccount(p.id) })),
      { group: 'Accounts', icon: '↑', label: 'Import credential files…', order: 203, run: () => { show(); panel.pickFiles(); } },
      { group: 'Accounts', icon: '⌁', label: 'Add endpoint…', detail: 'An API key: Anthropic, OpenAI, Gemini, xAI, OpenAI-compatible', order: 204, run: () => addEndpoint() },
    ],
  });

  cockpit.provide('accounts', {
    show: () => show('accounts'), back, showConnection, showUsage, addAccount, addEndpoint,
    refresh: (options) => panel.refresh(options), providers: () => panel.providers(), pickFiles: () => panel.pickFiles(),
  });
}
