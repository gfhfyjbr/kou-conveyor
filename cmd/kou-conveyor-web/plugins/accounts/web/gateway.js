// The Accounts view: the cockpit's gateway — CLIProxyAPI, run inside the
// server — through which runs reach every model. It holds the connection
// (the model runs use through the gateway), the accounts signed in with
// OAuth and the endpoints added with an API key, and how each is doing: its
// state and cooldowns, its requests and errors over time, and what is left
// of an account's limits. Tokens and keys stay on the server.
import { fmt, h as element } from '/kernel/dom.js';

const POLL = 5000;
const QUOTA_STALE = 3 * 60 * 1000;
const QUOTA_PARALLEL = 3;

const STATE_LABEL = { ready: 'Ready', cooling: 'Cooling down', error: 'Failing', disabled: 'Disabled', refreshing: 'Refreshing', configured: 'config.yaml' };
const GATEWAY_LABEL = { starting: 'Starting', running: 'Running', failed: 'Failed', stopped: 'Stopped', off: 'Off' };
const REASON_LABEL = {
  quota: 'rate limit', credential_quota: 'quota used up', unauthorized: 'unauthorized', payment_required: 'payment required',
  invalid_grant: 'sign-in revoked', model_not_supported: 'model not supported', cloudflare_challenge: 'Cloudflare challenge',
  unknown: 'error',
};
const RANGES = { '24h': 'Last 24 hours', '7d': 'Last 7 days' };

// ---------------------------------------------------------------- rendering in place
//
// The tab renders again on every poll. Its nodes stay in place across renders
// (sync, morph): a node replaced, or taken out of the page and put back,
// replays its fade-in and loses its hover, focus and selected text, which
// reads as the tab flickering every few seconds.

// h is view.js's, with event handlers a render can hand over to the node it
// keeps: a node listens once per event type, calling the handler it holds now.
function h(tag, attrs, ...children) {
  const plain = {};
  const handlers = [];
  for (const [key, value] of Object.entries(attrs || {})) {
    if (key.startsWith('on') && typeof value === 'function') handlers.push([key.slice(2), value]);
    else plain[key] = value;
  }
  const el = element(tag, plain, ...children);
  for (const [type, fn] of handlers) listen(el, type, fn);
  return el;
}

function listen(el, type, fn) {
  el._on ||= {};
  if (!(type in el._on)) el.addEventListener(type, (event) => el._on[type]?.(event));
  el._on[type] = fn;
}

// morph makes old look like fresh and returns the node that stays: old,
// patched where the two differ, or fresh in its place where they are
// different kinds of node.
function morph(old, fresh) {
  if (old === fresh) return old;
  if (old.nodeType !== fresh.nodeType || old.nodeName !== fresh.nodeName) {
    old.replaceWith(fresh);
    return fresh;
  }
  if (old.nodeType !== Node.ELEMENT_NODE) {
    if (old.nodeValue !== fresh.nodeValue) old.nodeValue = fresh.nodeValue;
    return old;
  }
  for (const { name } of [...old.attributes]) {
    if (!fresh.hasAttribute(name)) old.removeAttribute(name);
  }
  for (const { name, value } of [...fresh.attributes]) {
    if (old.getAttribute(name) !== value) old.setAttribute(name, value);
  }
  const types = new Set([...Object.keys(old._on || {}), ...Object.keys(fresh._on || {})]);
  for (const type of types) {
    if (fresh._on?.[type]) listen(old, type, fresh._on[type]);
    else old._on[type] = null;
  }
  // What the user types stays while they type it.
  if ((old instanceof HTMLInputElement || old instanceof HTMLTextAreaElement) && old !== document.activeElement) {
    if (old.value !== fresh.value) old.value = fresh.value;
    if (old.checked !== fresh.checked) old.checked = fresh.checked;
  }
  const had = [...old.childNodes];
  const want = [...fresh.childNodes];
  want.forEach((node, i) => {
    if (i < had.length) morph(had[i], node);
    else old.append(node);
  });
  for (const node of had.slice(want.length)) node.remove();
  return old;
}

// sync makes box's children the nodes of a new render. Each pairs up with a
// child box holds, by its data-row or else by its position, and the child
// stays, patched to match; only new nodes enter the page, and only nodes
// that are gone leave it.
function sync(box, nodes) {
  nodes = nodes.filter(Boolean);
  const old = [...box.childNodes];
  const byRow = new Map();
  for (const node of old) if (node.dataset?.row) byRow.set(node.dataset.row, node);
  const taken = new Set(nodes.filter((node) => node.parentNode === box));
  const next = nodes.map((node, i) => {
    if (node.parentNode === box) return node;
    const row = node.dataset?.row;
    const match = row ? byRow.get(row) : old[i];
    if (!match || taken.has(match) || (match.dataset?.row || '') !== (row || '') || match.nodeName !== node.nodeName) return node;
    taken.add(match);
    return morph(match, node);
  });
  const keep = new Set(next);
  for (const node of old) if (!keep.has(node)) node.remove();
  // Nodes already in order stay where they are; moving one would replay it.
  let at = box.firstChild;
  for (const node of next) {
    if (node === at) at = at.nextSibling;
    else box.insertBefore(node, at);
  }
}

export function createAccounts(ctx) {
  const { api, toast, copy, openMenu, $, query, prefs } = ctx;
  // The models plugin's picker and titles, when it is there.
  const modelTitle = (catalog, id, extra = '') => ctx.models()?.titleIn?.(catalog, id, extra) ?? [id, extra].filter(Boolean).join('\n');
  const openModelPicker = (options) => {
    const models = ctx.models();
    if (!models?.openPicker) {
      toast('Choosing among the models needs the models plugin, which is off.', 'error');
      return null;
    }
    return models.openPicker(options);
  };
  const ui = {
    open: false,
    data: null, // the latest /api/accounts answer
    error: '',
    loading: false,
    ticket: 0,
    poll: 0,
    range: prefs.get('accounts-range', '24h') === '7d' ? '7d' : '24h',
    filter: { provider: '', state: '' },
    expanded: new Set(), // accounts whose details are open
    quota: new Map(), // name → { loading, quota, error, at }
    queue: [],
    fetching: 0,
    conn: null, // the connection form's draft: { mode, model, protocol, busy, error, dirty }
    // modelsSig says which models the gateway served at the latest listing,
    // which lists them with the accounts.
    modelsSig: '',
    // watch looks again, soon, for accounts just added until the gateway
    // has taken them up with their models: { before, provider, until, timer }.
    watch: null,
    endpoint: null, // the endpoint dialog; see renderEndpointDialog
    signin: null, // the sign-in dialog; see renderSignIn
    sigs: new Map(), // row signatures, to keep unchanged rows in place
  };

  // ---------------------------------------------------------------- data

  async function refresh({ quiet = false } = {}) {
    const ticket = ++ui.ticket;
    if (!quiet) ui.loading = !ui.data;
    let data;
    try {
      data = await api(`/api/accounts?range=${ui.range}`);
    } catch (error) {
      if (ticket !== ui.ticket) return;
      ui.error = error.message;
      ui.loading = false;
      render();
      return;
    }
    if (ticket !== ui.ticket) return;
    ui.data = data;
    ui.error = data.error || '';
    ui.loading = false;
    if (!ui.conn?.dirty) ui.conn = null;
    for (const name of [...ui.expanded]) if (!data.accounts.some((a) => a.name === name)) ui.expanded.delete(name);
    render();
    ctx.onSummary(summary());
    if (ui.open) wantQuotas();
    // The composer's list follows the gateway's models.
    const sig = JSON.stringify((data.models || []).map((m) => [m.id, !!m.cooling]));
    if (sig !== ui.modelsSig) {
      ui.modelsSig = sig;
      ctx.onModels?.();
    }
    ui.signin?.onList?.(data.accounts);
    lookAgain(data);
  }

  // watchNew looks for the accounts added from now on, which the gateway
  // takes up — and registers the models of — a moment after they are saved.
  function watchNew(provider = '') {
    clearTimeout(ui.watch?.timer);
    ui.watch = { before: new Set((ui.data?.accounts || []).map((a) => a.name)), provider, until: Date.now() + 20_000, timer: 0 };
  }

  // lookAgain lists the accounts again shortly while an account just added
  // has not shown up, or shows without its models.
  function lookAgain(data) {
    const w = ui.watch;
    if (!w) return;
    const added = data.accounts.filter((a) => !w.before.has(a.name) && (!w.provider || a.provider === w.provider));
    const waiting = !added.length || added.some((a) => !a.models?.length && a.state !== 'disabled' && a.state !== 'error');
    if (!waiting || Date.now() > w.until) {
      ui.watch = null;
      return;
    }
    clearTimeout(w.timer);
    w.timer = setTimeout(() => { if (ui.watch === w) refresh({ quiet: true }); }, 700);
  }

  function schedule() {
    clearTimeout(ui.poll);
    if (!ui.open || document.visibilityState !== 'visible') return;
    ui.poll = setTimeout(() => refresh({ quiet: true }).finally(schedule), POLL);
  }

  function summary() {
    const g = ui.data?.gateway;
    return g ? { state: g.state, ...g.summary } : null;
  }

  // An account's quota: the latest the page fetched, else the list's.
  function quotaOf(a) {
    const own = ui.quota.get(a.name);
    const listed = a.quota || null;
    if (own?.quota && (!listed || Date.parse(own.quota.at) >= Date.parse(listed.at))) return own.quota;
    return listed;
  }

  function wantQuotas() {
    for (const a of ui.data?.accounts || []) {
      if (!a.quota_supported || a.state === 'disabled' || a.state === 'error') continue;
      const own = ui.quota.get(a.name);
      if (own?.loading || ui.queue.some((item) => item.name === a.name)) continue;
      if (own?.error && Date.now() - own.at < 60_000) continue;
      const q = quotaOf(a);
      if (q && q.source === 'live' && !q.error && Date.now() - Date.parse(q.at) < QUOTA_STALE) continue;
      ui.queue.push({ name: a.name, refresh: false });
    }
    pumpQuotas();
  }

  // askAll asks every provider for the limits left now, a few at a time.
  function askAll(list) {
    for (const a of list) {
      if (!a.quota_supported || a.state === 'disabled' || ui.quota.get(a.name)?.loading) continue;
      ui.queue = ui.queue.filter((item) => item.name !== a.name);
      ui.queue.push({ name: a.name, refresh: true });
      ui.quota.set(a.name, { ...(ui.quota.get(a.name) || {}), queued: true });
    }
    renderList();
    pumpQuotas();
  }

  function pumpQuotas() {
    while (ui.fetching < QUOTA_PARALLEL && ui.queue.length) {
      const item = ui.queue.shift();
      loadQuota(item.name, item.refresh);
    }
  }

  async function loadQuota(name, refreshNow) {
    const entry = { ...(ui.quota.get(name) || {}), loading: true, queued: false };
    ui.quota.set(name, entry);
    ui.fetching++;
    renderList();
    try {
      const quota = await api(`/api/accounts/${encodeURIComponent(name)}/quota${refreshNow ? '?refresh=1' : ''}`);
      ui.quota.set(name, { quota, error: quota.error || '', at: Date.now(), loading: false });
    } catch (error) {
      ui.quota.set(name, { ...entry, error: error.message, at: Date.now(), loading: false });
    } finally {
      ui.fetching--;
      renderList();
      pumpQuotas();
    }
  }

  // ---------------------------------------------------------------- tab

  function open() {
    if (ui.open) return;
    ui.open = true;
    render();
    refresh().finally(schedule);
  }

  function close() {
    ui.open = false;
    clearTimeout(ui.poll);
  }

  function setRange(range) {
    if (range === ui.range || !RANGES[range]) return;
    ui.range = range;
    prefs.set('accounts-range', range);
    ui.sigs.clear();
    refresh();
  }

  // ---------------------------------------------------------------- rendering

  function render() {
    if (!ui.open) return;
    renderHeader();
    renderRail();
    renderTop();
    renderList();
    renderEndpoints();
  }

  function renderHeader() {
    const g = ui.data?.gateway;
    const state = g?.state || 'starting';
    const chip = $('gw-state');
    if (chip.dataset.state !== state) chip.dataset.state = state;
    chip.title = g?.error || (state === 'running' ? `Up since ${fmt.stamp(g.since)}` : '');
    setText($('gw-word'), GATEWAY_LABEL[state] || state);
    setText($('gw-version'), g?.version ? `CLIProxyAPI ${g.version}` : 'CLIProxyAPI');
    for (const button of $('accounts-range').querySelectorAll('button')) {
      button.setAttribute('aria-checked', String(button.dataset.range === ui.range));
    }
    $('accounts-add-top').disabled = state !== 'running';
  }

  // addMenu offers what the gateway can take: an account, an endpoint, or
  // credentials another CLIProxyAPI signed in.
  function addMenu(anchor) {
    openMenu(anchor, [
      { icon: '+', label: 'Account', detail: 'Sign in with a subscription: Claude, Codex, Antigravity, Grok, Kimi…', run: () => addAccount() },
      { icon: '⌁', label: 'Endpoint', detail: 'An API key: Anthropic, OpenAI, Gemini, xAI or any OpenAI-compatible provider', run: () => addEndpoint() },
      { icon: '↑', label: 'Import credential files…', detail: 'JSON credentials of another CLIProxyAPI', run: pickFiles },
    ]);
  }

  function renderRail() {
    const list = ui.data?.accounts || [];
    const providers = new Map();
    for (const a of list) {
      const p = providers.get(a.provider) || { id: a.provider, name: a.provider_name, count: 0, failing: 0, cooling: 0 };
      p.count++;
      if (a.state === 'error') p.failing++;
      if (a.state === 'cooling') p.cooling++;
      providers.set(a.provider, p);
    }
    const item = (row, active, label, count, onclick, health) => h('button', {
      type: 'button', class: 'filter', 'aria-pressed': String(active), onclick, data: { row },
    }, h('span', { class: 'dot', data: { health: health || null } }), h('span', { class: 't', text: label }), h('span', { class: 'n', text: String(count) }));
    setText($('account-count'), String(list.length));
    sync($('provider-filter'), [
      item('all', !ui.filter.provider, 'All accounts', list.length, () => setFilter('provider', '')),
      ...[...providers.values()].map((p) => item(`p:${p.id}`, ui.filter.provider === p.id, p.name, p.count,
        () => setFilter('provider', ui.filter.provider === p.id ? '' : p.id), p.failing ? 'bad' : p.cooling ? 'warn' : 'ok')),
    ]);
    const s = ui.data?.gateway?.summary || {};
    sync($('state-filter'), [
      ['ready', 'Ready', s.ready, 'ok'], ['cooling', 'Cooling down', s.cooling, 'warn'],
      ['error', 'Failing', s.failing, 'bad'], ['disabled', 'Disabled', s.disabled, 'off'],
      s.keys ? ['configured', 'Keys in config.yaml', s.keys, 'off'] : null,
    ].filter(Boolean).map(([state, label, count, health]) => item(`s:${state}`, ui.filter.state === state, label, count || 0,
      () => setFilter('state', ui.filter.state === state ? '' : state), health)));
    const endpoints = ui.data?.endpoints || [];
    const kinds = new Map();
    for (const e of endpoints) {
      const k = kinds.get(e.kind) || { name: e.kind_name, count: 0, failing: 0 };
      k.count++;
      if (e.state === 'error') k.failing++;
      kinds.set(e.kind, k);
    }
    setText($('endpoint-count'), String(endpoints.length));
    const jump = () => $('endpoints-list').scrollIntoView({ block: 'start', behavior: 'smooth' });
    sync($('endpoint-filter'), [
      ...[...kinds.entries()].map(([id, k]) => item(`k:${id}`, false, k.name, k.count, jump, k.failing ? 'bad' : 'ok')),
      h('button', { type: 'button', class: 'filter add', onclick: () => addEndpoint(), data: { row: 'add' } }, h('span', { class: 'dot' }), h('span', { class: 't', text: '+ Add endpoint' }), h('span', { class: 'n' })),
    ]);
    const g = ui.data?.gateway;
    setText($('accounts-rail-note'), g?.dir ? `Credentials in ${home(g.dir)}/auths` : '');
  }

  function setFilter(kind, value) {
    ui.filter[kind] = value;
    ui.sigs.clear();
    render();
  }

  // renderTop shows the connection — what runs use — above the gateway it
  // goes through.
  function renderTop() {
    const box = $('gw-card');
    // A form being filled in stays as it is.
    if (box.contains(document.activeElement) && document.activeElement.matches('input, textarea')) return;
    const g = ui.data?.gateway;
    if (!g) {
      sync(box, [ui.error ? notice('Could not load the accounts', ui.error) : loadingLine('Loading accounts')]);
      return;
    }
    const c = ui.data.connection || { mode: 'environment' };
    ui.conn ||= { mode: c.mode, model: c.model || '', protocol: c.protocol || '', dirty: false };
    const running = g.enabled && g.state === 'running';
    let gateway;
    if (!g.enabled) {
      gateway = gatewayNote('The accounts gateway is off', g.error || 'Start the server with -accounts 127.0.0.1:8318 to run CLIProxyAPI inside it: sign in with Claude, Codex, Antigravity, Grok or Kimi subscriptions, add API keys, and send runs through it.');
    } else if (g.state === 'failed' || g.state === 'stopped') {
      gateway = gatewayNote(g.state === 'failed' ? 'The accounts gateway could not start' : 'The accounts gateway stopped', g.error || 'Restart the server to start it again.', g.config);
    } else {
      gateway = gatewaySection(g);
    }
    sync(box, [h('section', { class: 'gw ticks', data: { row: 'top' } }, connectionSection(g, c, running), gateway)]);
  }

  function connectionSection(g, c, running) {
    const draft = ui.conn;
    const modes = [
      ['gateway', 'Gateway', running ? 'accounts & endpoints' : g.enabled ? 'starting…' : 'off'],
      ['environment', 'Environment', 'runner defaults'],
      c.mode === 'direct' && ['direct', 'Direct', 'around the gateway'],
      c.mode === 'flag' && ['flag', '-provider', 'the server’s flag'],
    ].filter(Boolean);
    const seg = h('div', { class: 'conn-modes', role: 'radiogroup', 'aria-label': 'Runs connect to' },
      modes.map(([value, label, small]) => h('button', {
        type: 'button', role: 'radio', 'aria-checked': String(draft.mode === value),
        disabled: (value === 'gateway' && !g.enabled) || value === 'flag' || (value === 'direct' && c.mode !== 'direct'),
        onclick: () => { draft.mode = value; draft.dirty = value !== c.mode; renderTop(); },
      }, h('b', { text: label }), h('small', { text: small }))));
    const body = [];
    switch (draft.mode) {
      case 'gateway': {
        const chosen = draft.protocol || apiFor(draft.model);
        body.push(h('form', {
          class: 'conn-form', autocomplete: 'off',
          onsubmit: (event) => { event.preventDefault(); saveConnection(); },
        },
        h('div', { class: 'field conn-model' }, h('label', { class: 'label', for: 'conn-model', text: 'Model' }),
          h('span', { class: 'conn-model-row' },
            h('input', {
              id: 'conn-model', type: 'text', spellcheck: 'false', autocomplete: 'off', value: draft.model,
              placeholder: running ? 'Choose a model the gateway serves' : 'The gateway is not running',
              title: 'The model runs use unless a prompt picks another (↓ lists them)',
              oninput: (event) => { draft.model = event.target.value.trim(); draft.dirty = true; renderProtocols(); renderConnectionHint(); },
              onkeydown: (event) => {
                if (event.key === 'ArrowDown' && !event.altKey && !event.metaKey && !event.ctrlKey) {
                  event.preventDefault();
                  browseModels();
                }
              },
            }),
            h('button', {
              class: 'act', type: 'button', disabled: !running, 'aria-haspopup': 'dialog',
              title: 'Choose among the models the gateway serves, by provider (↓)', onclick: browseModels,
            }, 'Browse ▾'))),
        h('fieldset', { class: 'seg three conn-api', id: 'conn-api' },
          h('legend', { class: 'label', text: 'API' }),
          protocolChoices(draft, chosen)),
        h('button', { class: 'primary', type: 'submit', disabled: !!draft.busy || !draft.model || !running }, draft.busy ? 'Saving…' : c.mode === 'gateway' && !draft.dirty ? 'Saved' : 'Use')));
        const [hint, kind] = connectionHint();
        body.push(h('p', { class: 'conn-hint', id: 'conn-hint', data: { kind: kind || null }, text: hint }));
        break;
      }
      case 'environment':
        body.push(h('p', { class: 'conn-text', text: `Runs use the runner's environment and the workspace's .env: ${describe(c.resolved)}.` }));
        if (c.mode !== 'environment') {
          body.push(h('div', { class: 'conn-actions' }, h('button', { class: 'primary', type: 'button', disabled: !!draft.busy, onclick: () => saveConnection() }, 'Use the environment')));
        }
        break;
      case 'direct':
        body.push(h('p', { class: 'conn-text' }, c.via_gateway
          ? `Runs reach the gateway at ${hostOf(c.base_url)} as if it were any endpoint, with its key copied into the settings${c.model ? ` and ${c.model}` : ''}. Moved into the gateway, they follow it wherever it listens.`
          : `Runs connect directly to ${hostOf(c.base_url) || 'the provider'} over the ${c.api === 'messages' ? 'Messages' : 'Responses'} API${c.model ? `, with ${c.model}` : ''}${c.key_hint ? ` and the key ${c.key_hint}` : ''} — around the gateway, whose accounts, endpoints and uptime do not see them. Moved into the gateway, the endpoint joins the others there.`));
        body.push(h('div', { class: 'conn-actions' },
          h('button', { class: 'primary', type: 'button', disabled: !running || !!draft.busy, onclick: adopt, title: 'Add the endpoint to the gateway with its key, and send runs through the gateway to the same model' }, draft.busy ? 'Moving…' : 'Move into the gateway'),
          h('button', { class: 'act', type: 'button', onclick: () => ctx.openDirectSettings() }, 'Edit…')));
        break;
      case 'flag':
        body.push(h('p', { class: 'conn-text', text: `The server was started with -provider ${c.resolved?.provider || ''}, which decides runs' connection until it restarts without it.` }));
        break;
      default:
    }
    if (draft.error) body.push(h('p', { class: 'settings-status', data: { kind: 'error' }, text: draft.error }));
    if (c.error) body.push(h('p', { class: 'settings-status', data: { kind: 'error' }, text: `${c.error}; saving replaces the settings` }));
    return h('div', { class: 'conn' },
      h('header', { class: 'gw-head' }, h('span', { class: 'label', text: 'Connection' }), seg),
      body);
  }

  function protocolChoices(draft, chosen) {
    return [['', `Auto · ${chosen === 'messages' ? 'Messages' : 'Responses'}`, 'The API the model speaks natively'],
      ['messages', 'Messages', 'Anthropic Messages API'], ['responses', 'Responses', 'OpenAI Responses API']]
      .map(([value, label, title]) => h('label', { title },
        h('input', { type: 'radio', name: 'conn-api', value, checked: (draft.protocol || '') === value, onchange: () => { draft.protocol = value; draft.dirty = true; renderProtocols(); } }),
        h('span', null, h('b', { text: label }))));
  }

  // renderProtocols updates the API choices as the model changes.
  function renderProtocols() {
    const box = $('conn-api');
    if (!box || !ui.conn) return;
    const auto = box.querySelector('input[value=""] + span b');
    if (auto) auto.textContent = `Auto · ${apiFor(ui.conn.model) === 'messages' ? 'Messages' : 'Responses'}`;
    const button = box.closest('form')?.querySelector('.primary');
    if (button) {
      button.disabled = !!ui.conn.busy || !ui.conn.model || ui.data?.gateway?.state !== 'running';
      button.textContent = ui.conn.dirty ? 'Use' : 'Saved';
    }
  }

  // connectionHint says what the model is to the gateway: [text, kind].
  function connectionHint() {
    const models = ui.data?.models;
    const model = ui.conn?.model || '';
    const api = (ui.conn?.protocol || apiFor(model)) === 'messages' ? 'Messages' : 'Responses';
    if (!models) return [ui.data?.gateway?.state === 'running' ? 'Asking the gateway which models it serves…' : 'The gateway lists its models once it runs.', ''];
    const usable = models.filter((m) => !m.media);
    const providers = new Set(usable.map((m) => m.provider_name)).size;
    const all = `${usable.length} model${usable.length === 1 ? '' : 's'} of ${providers} provider${providers === 1 ? '' : 's'}`;
    if (!usable.length) return ['The gateway serves no models yet: sign in with an account or add an endpoint below.', 'warn'];
    const found = models.find((m) => m.id === model);
    if (model && !found) return [`The gateway serves no ${model}: its accounts and endpoints serve ${all}.`, 'warn'];
    if (found?.cooling) return [`${model} is cooling down: every account that serves it waits out a limit, and runs fail until one has. ${all} in all.`, 'warn'];
    if (model) {
      const by = found.via?.length ? found.via.join(', ') : found.provider_name;
      return [`Runs send ${model} to the gateway over the ${api} API${by ? `; ${by} serve${found.via?.length > 1 ? '' : 's'} it` : ''}. Any prompt can pick another of the ${all} in the composer.`, ''];
    }
    return [`Choose the model runs use by default, of the ${all} the gateway serves; any prompt can pick another in the composer.`, ''];
  }

  // renderConnectionHint updates the hint as the model is typed.
  function renderConnectionHint() {
    const el = $('conn-hint');
    if (!el || !ui.conn) return;
    const [text, kind] = connectionHint();
    setText(el, text);
    if ((el.dataset.kind || '') !== kind) el.dataset.kind = kind;
  }

  // gatewayCatalog is what the gateway serves, as the model picker lists it.
  function gatewayCatalog() {
    return { source: 'gateway', default: ui.data?.connection?.mode === 'gateway' ? ui.data.connection.model || '' : '', models: ui.data?.models || [], error: '' };
  }

  // browseModels opens the models the gateway serves under the connection's
  // model field.
  function browseModels() {
    const input = $('conn-model');
    if (!input || !ui.conn) return;
    openModelPicker({
      anchor: input.closest('.conn-model-row') || input, placement: 'below', title: 'Default model',
      note: 'of runs through the gateway; a prompt can pick another',
      catalog: gatewayCatalog(), loading: !ui.data?.models, current: ui.conn.model,
      onPick: (id) => {
        const field = $('conn-model');
        ui.conn.model = id;
        ui.conn.dirty = true;
        if (field) {
          field.value = id;
          field.focus();
        }
        renderProtocols();
        renderConnectionHint();
      },
      onRefresh: async () => {
        await refresh({ quiet: true });
        return gatewayCatalog();
      },
    });
  }

  // Claude models speak the Messages API natively; the gateway translates
  // the Responses API for the others.
  function apiFor(model) {
    return /(^|\/)claude/i.test(model || '') ? 'messages' : 'responses';
  }

  async function saveConnection() {
    const draft = ui.conn;
    draft.busy = true;
    draft.error = '';
    renderTop();
    try {
      const body = draft.mode === 'gateway' ? { mode: 'gateway', model: draft.model, protocol: draft.protocol || '' } : { mode: 'environment' };
      const answer = await api('/api/connection', { method: 'PUT', body });
      ui.data.connection = answer.connection;
      ui.data.gateway = answer.gateway;
      ui.conn = null;
      toast(body.mode === 'gateway' ? `Runs go through the gateway · ${body.model}` : 'Runs use the environment');
      ctx.onConnection();
    } catch (error) {
      draft.error = error.message;
      draft.busy = false;
    }
    renderTop();
  }

  // adopt moves a direct connection into the gateway.
  async function adopt() {
    const draft = ui.conn;
    draft.busy = true;
    draft.error = '';
    renderTop();
    try {
      const answer = await api('/api/connection/adopt', { method: 'POST', body: {} });
      ui.data.connection = answer.connection;
      ui.conn = null;
      toast(`Moved into the gateway · ${answer.connection.model}`);
      ctx.onConnection();
      refresh();
    } catch (error) {
      draft.error = error.message;
      draft.busy = false;
      renderTop();
    }
  }

  function gatewaySection(g) {
    const s = g.summary || {};
    const total = (s.ok || 0) + (s.failed || 0);
    const failing = (s.failing || 0) + (s.failing_endpoints || 0);
    return h('div', { class: 'gw-body' },
      h('header', { class: 'gw-head' }, h('span', { class: 'label', text: 'Gateway' }),
        h('span', { class: 'gw-sub', text: g.state === 'running' ? `up ${since(g.since)}` : GATEWAY_LABEL[g.state] || g.state })),
      h('div', { class: 'gw-kvs' },
        kv('Endpoint', g.url, null, h('button', { class: 'act', type: 'button', onclick: () => copy(g.url, 'Endpoint copied') }, 'Copy')),
        kv('API key', g.key_hint ? `${g.key_hint} · in config.yaml` : '—'),
        kv('Config', g.config ? home(g.config) : '—', null, h('button', { class: 'act', type: 'button', onclick: () => copy(g.config, 'Path copied') }, 'Copy'))),
      h('div', { class: 'gw-stats' },
        stat(s.accounts || 0, s.keys ? `Accounts + ${s.keys} keys` : 'Accounts'),
        stat(s.endpoints || 0, 'Endpoints'),
        stat(s.ready || 0, 'Ready', 'ok'), stat(s.cooling || 0, 'Cooling', 'warn'), stat(failing, 'Failing', 'bad'),
        stat(total ? fmt.tokens(total) : '0', 'Requests · 24h'),
        stat(total ? percent(s.ok, total) : '—', 'Success · 24h', total && s.failed ? (s.failed / total > 0.1 ? 'bad' : 'warn') : total ? 'ok' : null)));
  }

  function gatewayNote(title, text, path) {
    return h('div', { class: 'gw-body gw-off' },
      h('span', { class: 'label', text: 'Gateway' }),
      h('h2', { text: title }),
      h('p', { text }),
      path ? h('p', { class: 'gw-path', text: path }) : null);
  }

  function renderList() {
    if (!ui.open) return;
    const box = $('accounts-list');
    const g = ui.data?.gateway;
    if (!g || !g.enabled || g.state !== 'running') {
      box.replaceChildren();
      return;
    }
    const all = ui.data.accounts || [];
    if (!all.length) {
      // Once endpoints serve runs, a line invites accounts; before, a card.
      const empty = (ui.data.endpoints || []).length ? emptyLine() : emptyCard();
      sync(box, [ui.error ? h('div', { class: 'load-error', data: { row: 'error' } }, h('b', { text: 'Could not load the accounts' }), h('span', { text: ui.error })) : null, empty]);
      return;
    }
    const list = all.filter((a) => (!ui.filter.provider || a.provider === ui.filter.provider) && (!ui.filter.state || a.state === ui.filter.state));
    // A listing that failed keeps the accounts of the one before.
    const failed = ui.error ? h('div', { class: 'load-error', data: { row: 'error' } }, h('b', { text: 'Could not refresh the accounts' }), h('span', { text: ui.error })) : null;
    const head = h('div', { class: 'list-head', data: { row: 'head' } },
      h('span', { class: 'label', text: ui.filter.provider || ui.filter.state ? `Accounts · ${list.length} of ${all.length}` : `Accounts · ${all.length}` }),
      h('span', { class: 'list-range', text: `Uptime: ${RANGES[ui.range].toLowerCase()}` }),
      h('button', {
        class: 'act', type: 'button', title: 'Ask every provider for the limits left now',
        onclick: () => askAll(list),
      }, 'Refresh limits'));
    // A row that did not change is not even built again.
    const minute = Math.floor(Date.now() / 60000);
    const nodes = [failed, head];
    for (const a of list) {
      const row = `acc:${a.name}`;
      const q = ui.quota.get(a.name);
      const sig = JSON.stringify([a, q?.loading, q?.queued, q?.error, quotaOf(a), ui.expanded.has(a.name), minute]);
      const kept = [...box.children].find((node) => node.dataset.row === row);
      nodes.push(kept && ui.sigs.get(row) === sig ? kept : accountRow(a));
      ui.sigs.set(row, sig);
    }
    if (!list.length) nodes.push(h('p', { class: 'none list-none', data: { row: 'none' }, text: 'No account matches the filter.' }));
    sync(box, nodes);
  }

  function accountRow(a) {
    const open = ui.expanded.has(a.name);
    const u = a.uptime || {};
    const total = (u.ok || 0) + (u.failed || 0);
    const cooling = (a.cooldowns || []).filter((c) => c.model);
    const q = quotaOf(a);
    const own = ui.quota.get(a.name);
    const stateText = a.state === 'cooling' && a.retry_at ? `${STATE_LABEL.cooling} · ${until(a.retry_at)}` : STATE_LABEL[a.state] || a.state;
    const configured = a.state === 'configured';
    const lastError = (a.errors || [])[0];
    return h('article', { class: 'account', data: { row: `acc:${a.name}`, name: a.name, state: a.state, open: open ? 'true' : null } },
      h('header', { class: 'acc-head' },
        h('button', {
          class: 'acc-toggle', type: 'button', 'aria-expanded': String(open), data: { key: 'toggle' },
          title: open ? 'Hide details' : 'Show details', onclick: () => toggle(a.name),
        }, h('span', { class: 'acc-chev', 'aria-hidden': 'true' }),
        h('span', { class: 'ptag', data: { provider: a.provider }, text: a.provider_name }),
        h('span', { class: 'acc-label', text: a.label, title: a.name }),
        a.plan ? h('span', { class: 'plan', text: a.plan }) : null),
        h('span', { class: 'acc-state', data: { state: a.state }, title: a.message || '' }, h('i', { 'aria-hidden': 'true' }), h('b', { text: stateText })),
        h('span', { class: 'acc-rate', title: `${u.ok || 0} succeeded · ${u.failed || 0} failed · ${RANGES[ui.range].toLowerCase()}` },
          h('b', { text: total ? percent(u.ok, total) : '—' }), h('small', { text: total ? `${fmt.tokens(total)} req` : 'no requests' })),
        h('button', {
          class: 'more acc-more', type: 'button', title: 'Account actions', 'aria-label': `Actions for ${a.label}`, 'aria-haspopup': 'menu', 'aria-expanded': 'false',
          data: { key: 'more' }, onclick: (event) => openMenu(event.currentTarget, accountMenu(a)),
        }, h('span', { 'aria-hidden': 'true', text: '⋯' }))),
      h('div', { class: 'acc-body' },
        h('div', { class: 'acc-main' },
          uptimeBar(u),
          h('p', { class: 'acc-stats' }, total
            ? [`${u.ok || 0} ok`, u.failed ? `${u.failed} failed` : null,
              `${fmt.tokens(u.input_tokens || 0)} in · ${fmt.tokens(u.output_tokens || 0)} out`,
              u.avg_latency_ms ? `avg ${latency(u.avg_latency_ms)}` : null].filter(Boolean).join(' · ')
            : u.last_ok ? `No requests in this span · last one ${fmt.ago(u.last_ok)} ago` : 'No requests yet'),
          a.message && a.state !== 'ready' && !configured ? h('p', { class: 'acc-note', data: { state: a.state }, text: a.message }) : null,
          cooling.length ? h('p', { class: 'acc-note', data: { state: 'cooling' } },
            `${cooling.length === 1 ? cooling[0].model : `${cooling.length} models`} cooling down · ${REASON_LABEL[cooling[0].reason] || cooling[0].reason} · ${until(cooling[0].until)}`) : null,
          lastError ? h('p', { class: 'acc-error', title: lastError.message || '' },
            h('span', { class: 'err-code', text: lastError.status ? String(lastError.status) : 'ERR' }),
            h('span', { class: 'when', text: fmt.ago(lastError.at) }),
            h('span', { class: 'msg', text: lastError.message || 'request failed' })) : null,
          accountModels(a)),
        h('div', { class: 'acc-limits' }, limits(a, q, own))),
      open ? details(a, q) : null);
  }

  // accountModels says which models the gateway serves with an account: the
  // first few, and how many in all. Those cooling down on it are marked.
  function accountModels(a, all = false) {
    const ids = a.models || [];
    if (!ids.length) {
      if (a.state === 'configured') return null;
      const text = a.state === 'disabled' ? 'Disabled: serves no models'
        : a.state === 'error' ? 'Serves no models until it signs in again'
          : ui.watch && !ui.watch.before.has(a.name) ? 'Taking up its models…' : 'Serves no models the gateway knows';
      return h('p', { class: 'acc-models none', data: { pending: text.endsWith('…') ? 'true' : null } }, h('span', { class: 'ep-count', text }));
    }
    const cooling = new Set((a.cooldowns || []).filter((c) => c.model).map((c) => c.model));
    const catalog = { models: ui.data?.models || [] };
    // As the pickers list them: by provider, the newest first.
    const rank = new Map(catalog.models.map((m, i) => [m.id, i]));
    const sorted = [...ids].sort((x, y) => (rank.get(x) ?? Infinity) - (rank.get(y) ?? Infinity) || x.localeCompare(y));
    const shown = all ? sorted : sorted.slice(0, 6);
    return h('div', { class: 'acc-models' },
      h('span', { class: 'ep-count', text: `${ids.length} model${ids.length === 1 ? '' : 's'}` }),
      h('div', { class: 'chips' },
        shown.map((id) => h('code', { class: 'chip', data: { cooling: cooling.has(id) ? 'true' : null }, text: id, title: modelTitle(catalog, id, cooling.has(id) ? 'Cooling down on this account' : '') })),
        !all && ids.length > shown.length ? h('button', { class: 'lim-more', type: 'button', onclick: () => toggle(a.name) }, `+${ids.length - shown.length} more`) : null));
  }

  // toggle opens or closes the details of an account or an endpoint.
  function toggle(name) {
    if (ui.expanded.has(name)) ui.expanded.delete(name); else ui.expanded.add(name);
    renderList();
    renderEndpoints();
  }

  function uptimeBar(u) {
    const slots = u.slots || [];
    const step = (u.slot_seconds || 1800) * 1000;
    const from = Date.parse(u.from);
    const bar = h('div', { class: 'uptime', role: 'img', 'aria-label': `Requests over ${RANGES[ui.range].toLowerCase()}: ${u.ok || 0} succeeded, ${u.failed || 0} failed` },
      slots.map((slot, i) => {
        const n = slot.ok + slot.failed;
        const level = !n ? 'none' : !slot.failed ? 'ok' : slot.failed / n >= 0.5 ? 'bad' : 'warn';
        const start = from + i * step;
        const future = start > Date.now();
        return h('i', {
          data: { level: future ? 'future' : level },
          title: `${slotTime(start, step)} · ${n ? `${slot.ok} ok${slot.failed ? ` · ${slot.failed} failed` : ''}` : 'no requests'}`,
        });
      }));
    return h('div', { class: 'uptime-row' }, bar,
      h('div', { class: 'uptime-axis' }, h('span', { text: ui.range === '7d' ? '7d ago' : '24h ago' }), h('span', { text: 'now' })));
  }

  function limits(a, q, own) {
    if (a.state === 'configured') return h('p', { class: 'lim-none', text: 'A key of the gateway’s config.yaml, which the gateway does not list: it shows here from its requests. Change it in the file.' });
    if (!a.quota_supported && !q) return h('p', { class: 'lim-none', text: a.kind === 'api_key' ? 'API key: no subscription limits' : `${a.provider_name} does not report its limits` });
    const rows = [];
    const windows = (q?.windows || []);
    for (const w of windows.slice(0, 4)) rows.push(limitRow(w));
    if (windows.length > 4) rows.push(h('button', { class: 'lim-more', type: 'button', onclick: () => toggle(a.name) }, `+${windows.length - 4} more`));
    let status = '';
    if (own?.loading) status = 'Asking…';
    else if (own?.queued) status = 'Waiting to ask…';
    else if (q?.error) status = q.status === 401 || q.status === 403 ? `Could not ask: ${q.error}` : `Could not ask: ${q.error}`;
    else if (own?.error) status = own.error;
    else if (q) status = q.source === 'observed' ? `From its last response · ${fmt.ago(q.at)} ago` : `Live · ${fmt.ago(q.at) === 'now' ? 'just now' : `${fmt.ago(q.at)} ago`}`;
    else status = a.quota_supported ? 'Not asked yet' : '';
    if (!windows.length && !status) status = 'No limits reported';
    return [
      rows.length ? h('div', { class: 'lims' }, rows) : null,
      h('div', { class: 'lim-foot' },
        h('span', { class: 'lim-src', data: { kind: own?.loading ? 'busy' : q?.error || own?.error ? 'error' : q?.source || null }, text: status, title: q?.error || own?.error || '' }),
        a.quota_supported ? h('button', {
          class: 'act', type: 'button', data: { key: 'quota' }, disabled: !!own?.loading,
          title: 'Ask the provider now', onclick: () => loadQuota(a.name, true),
        }, 'Refresh') : null),
    ];
  }

  function limitRow(w) {
    const left = Math.max(0, 100 - (w.used || 0));
    const level = w.reached || left <= 5 ? 'bad' : left <= 25 ? 'warn' : 'ok';
    const fill = h('i');
    fill.style.width = `${left}%`;
    return h('div', { class: 'lim', data: { level } },
      h('span', { class: 'lim-label', text: w.label, title: w.label }),
      h('span', { class: 'lim-bar', title: `${fmtPct(w.used)} used${w.amount ? ` · ${w.amount}` : ''}` }, fill),
      h('span', { class: 'lim-left', text: w.reached ? 'limit hit' : `${fmtPct(left)} left` }),
      h('span', { class: 'lim-reset', text: w.resets_at ? resets(w.resets_at) : w.amount || '', title: w.resets_at ? fmt.stamp(w.resets_at) : '' }));
  }

  function details(a, q) {
    const u = a.uptime || {};
    const errs = a.errors || [];
    const windows = q?.windows || [];
    return h('div', { class: 'acc-details' },
      h('section', null,
        h('h3', { text: 'Account' }),
        kv('Provider', a.provider_name),
        kv('Signed in as', a.label),
        a.plan ? kv('Plan', a.plan) : null,
        kv(a.state === 'configured' ? 'Key' : 'Credential', a.name),
        kv('Added', a.created ? fmt.stamp(a.created) : '—'),
        kv('Token refreshed', a.refreshed ? `${fmt.ago(a.refreshed)} ago` : '—'),
        a.priority != null ? kv('Priority', String(a.priority)) : null,
        a.note ? kv('Note', a.note) : null),
      h('section', null,
        h('h3', { text: `Requests · ${RANGES[ui.range].toLowerCase()}` }),
        kv('Succeeded', String(u.ok || 0)),
        kv('Failed', String(u.failed || 0), u.failed ? 'bad' : null),
        kv('Tokens in / out', `${fmt.tokens(u.input_tokens || 0)} / ${fmt.tokens(u.output_tokens || 0)}`),
        kv('Average latency', u.avg_latency_ms ? latency(u.avg_latency_ms) : '—'),
        kv('Last success', u.last_ok ? `${fmt.ago(u.last_ok)} ago` : '—'),
        kv('Last failure', u.last_failure ? `${fmt.ago(u.last_failure)} ago` : '—', u.last_failure ? 'bad' : null)),
      windows.length || q?.notes?.length ? h('section', { class: 'wide' },
        h('h3', { text: 'Limits' }),
        h('div', { class: 'lims' }, windows.map(limitRow)),
        (q?.notes || []).map((note) => h('p', { class: 'lim-note', text: note }))) : null,
      (a.models || []).length ? h('section', { class: 'wide' },
        h('h3', { text: `Models · ${a.models.length}` }),
        accountModels(a, true)) : null,
      (a.cooldowns || []).length ? h('section', { class: 'wide' },
        h('h3', { text: 'Cooldowns' }),
        h('ol', { class: 'cooldowns' }, a.cooldowns.map((c) => h('li', null,
          h('span', { class: 'm', text: c.model || 'whole account' }),
          h('span', { class: 'r', text: `${REASON_LABEL[c.reason] || c.reason}${c.status ? ` · ${c.status}` : ''}` }),
          h('span', { class: 't', text: until(c.until), title: fmt.stamp(c.until) }))))) : null,
      h('section', { class: 'wide' },
        h('h3', { text: errs.length ? `Latest errors · ${errs.length}` : 'Latest errors' }),
        errs.length ? h('ol', { class: 'errs' }, errs.map((e) => h('li', null,
          h('span', { class: 'err-code', text: e.status ? String(e.status) : 'ERR' }),
          h('time', { text: fmt.stamp(e.at), datetime: e.at }),
          e.model ? h('span', { class: 'model', text: e.model }) : null,
          h('span', { class: 'msg', text: e.message || 'request failed' })))) : h('p', { class: 'none', text: 'None recorded.' })));
  }

  function accountMenu(a) {
    const disabled = a.state === 'disabled';
    return [
      a.quota_supported && { icon: '↻', label: 'Refresh limits', run: () => loadQuota(a.name, true) },
      !a.config && { icon: '⟳', label: 'Refresh token', detail: 'Get a new access token now', run: () => refreshToken(a) },
      !a.config && { icon: disabled ? '▶' : '⏸', label: disabled ? 'Enable' : 'Disable', detail: disabled ? 'Let the gateway use it again' : 'Keep the gateway from using it', run: () => setDisabled(a, !disabled) },
      { icon: '▾', label: ui.expanded.has(a.name) ? 'Hide details' : 'Details', run: () => toggle(a.name) },
      { icon: '#', label: 'Copy credential name', run: () => copy(a.name, 'Name copied') },
      !a.config && { separator: true },
      !a.config && { icon: '×', label: 'Remove', danger: true, confirm: 'Click again to remove', detail: 'Delete its credential and history', run: () => remove(a) },
    ].filter(Boolean);
  }

  async function setDisabled(a, disabled) {
    try {
      await api(`/api/accounts/${encodeURIComponent(a.name)}`, { method: 'PATCH', body: { disabled } });
      toast(`${a.label} ${disabled ? 'disabled' : 'enabled'}`);
    } catch (error) {
      toast(error.message, 'error');
    }
    refresh();
  }

  async function remove(a) {
    try {
      await api(`/api/accounts/${encodeURIComponent(a.name)}`, { method: 'DELETE' });
      ui.quota.delete(a.name);
      ui.expanded.delete(a.name);
      toast(`${a.label} removed`);
    } catch (error) {
      toast(error.message, 'error');
    }
    refresh();
  }

  async function refreshToken(a) {
    toast(`Refreshing ${a.label}…`, 'info', `refresh-${a.name}`);
    try {
      await api(`/api/accounts/${encodeURIComponent(a.name)}/refresh`, { method: 'POST', body: {} });
      toast(`${a.label}: token refreshed`, 'info', `refresh-${a.name}`);
    } catch (error) {
      toast(`${a.label}: ${error.message}`, 'error', `refresh-${a.name}`);
    }
    refresh();
  }

  function emptyLine() {
    return h('div', { class: 'list-head acc-none', data: { row: 'empty-line' } },
      h('span', { class: 'label', text: 'Accounts · 0' }),
      h('span', { class: 'list-range', text: 'Sign in with a subscription:' }),
      h('span', { class: 'chips' }, (ui.data?.providers || []).map((p) => h('button', {
        class: 'ptag', type: 'button', data: { provider: p.id }, title: p.detail,
        onclick: () => { ui.signin = { step: 'choose' }; $('signin').hidden = false; begin(p, window.open('about:blank', '_blank')); },
      }, p.name))),
      h('button', { class: 'act', type: 'button', onclick: pickFiles }, 'Import…'));
  }

  function emptyCard() {
    const providers = ui.data?.providers || [];
    return h('section', { class: 'acc-empty ticks', data: { row: 'empty' } },
      h('span', { class: 'label', text: 'No accounts yet' }),
      h('h2', { text: 'Sign in with a subscription' }),
      h('p', { text: 'The gateway serves runs from the subscriptions you sign in with and spreads requests over them. Tokens stay on this machine, in the gateway’s folder.' }),
      h('div', { class: 'provider-grid' }, providers.map((p) => providerCard(p))),
      h('div', { class: 'acc-empty-foot' },
        h('button', { class: 'act', type: 'button', onclick: pickFiles }, 'Import credential files…'),
        h('span', { text: 'JSON credentials of another CLIProxyAPI' })));
  }

  // ---------------------------------------------------------------- endpoints

  function renderEndpoints() {
    if (!ui.open) return;
    const box = $('endpoints-list');
    const g = ui.data?.gateway;
    if (!g || !g.enabled || g.state !== 'running') {
      box.replaceChildren();
      return;
    }
    const list = ui.data.endpoints || [];
    const head = h('div', { class: 'list-head', data: { row: 'head' } },
      h('span', { class: 'label', text: `Endpoints · ${list.length}` }),
      h('span', { class: 'list-range', text: 'API keys the gateway serves models with, beside the accounts' }),
      h('button', { class: 'act strong', type: 'button', onclick: () => addEndpoint() }, '+ Add endpoint'));
    if (!list.length) {
      sync(box, [head, h('section', { class: 'ep-empty', data: { row: 'empty' } },
        h('p', { text: 'An API key of Anthropic, OpenAI, Gemini or xAI, or of any provider that speaks the Chat Completions API — OpenRouter, DeepSeek, a local Ollama — adds its models to the gateway. Runs reach them the same way as the accounts’.' }),
        h('div', { class: 'provider-grid kinds' }, (ui.data.endpoint_kinds || []).map((k) => h('button', {
          class: 'provider', type: 'button', onclick: () => addEndpoint(k.id),
        }, h('span', { class: 'ptag', data: { kind: k.id }, text: k.name }), h('span', { class: 'pdetail', text: k.detail })))))]);
      return;
    }
    const minute = Math.floor(Date.now() / 60000);
    const nodes = [head];
    for (const e of list) {
      const row = `ep:${e.id}`;
      const sig = JSON.stringify([e, ui.expanded.has(e.id), minute]);
      const kept = [...box.children].find((node) => node.dataset.row === row);
      nodes.push(kept && ui.sigs.get(row) === sig ? kept : endpointRow(e));
      ui.sigs.set(row, sig);
    }
    sync(box, nodes);
  }

  function endpointRow(e) {
    const open = ui.expanded.has(e.id);
    const u = e.uptime || {};
    const total = (u.ok || 0) + (u.failed || 0);
    const lastError = (e.errors || [])[0];
    const label = e.name || e.host;
    const models = e.models || [];
    return h('article', { class: 'account endpoint', data: { row: `ep:${e.id}`, name: e.id, state: e.state, open: open ? 'true' : null } },
      h('header', { class: 'acc-head' },
        h('button', { class: 'acc-toggle', type: 'button', 'aria-expanded': String(open), onclick: () => toggle(e.id) },
          h('span', { class: 'acc-chev', 'aria-hidden': 'true' }),
          h('span', { class: 'ptag', data: { kind: e.kind }, text: e.kind_name }),
          h('span', { class: 'acc-label', text: label, title: e.base_url || e.host }),
          e.name ? h('span', { class: 'ep-host', text: e.host }) : null,
          e.key_hint ? h('span', { class: 'plan', text: `key ${e.key_hint}`, title: e.keys > 1 ? `${e.keys} keys` : '' }) : null),
        h('span', { class: 'acc-state', data: { state: e.state } }, h('i', { 'aria-hidden': 'true' }), h('b', { text: e.state === 'error' ? 'Failing' : STATE_LABEL[e.state] || e.state })),
        h('span', { class: 'acc-rate', title: `${u.ok || 0} succeeded · ${u.failed || 0} failed · ${RANGES[ui.range].toLowerCase()}` },
          h('b', { text: total ? percent(u.ok, total) : '—' }), h('small', { text: total ? `${fmt.tokens(total)} req` : 'no requests' })),
        h('button', {
          class: 'more acc-more', type: 'button', title: 'Endpoint actions', 'aria-label': `Actions for ${label}`, 'aria-haspopup': 'menu', 'aria-expanded': 'false',
          onclick: (event) => openMenu(event.currentTarget, endpointMenu(e)),
        }, h('span', { 'aria-hidden': 'true', text: '⋯' }))),
      h('div', { class: 'acc-body' },
        h('div', { class: 'acc-main' },
          uptimeBar(u),
          h('p', { class: 'acc-stats' }, total
            ? [`${u.ok || 0} ok`, u.failed ? `${u.failed} failed` : null, `${fmt.tokens(u.input_tokens || 0)} in · ${fmt.tokens(u.output_tokens || 0)} out`,
              u.avg_latency_ms ? `avg ${latency(u.avg_latency_ms)}` : null].filter(Boolean).join(' · ')
            : 'No requests in this span'),
          lastError ? h('p', { class: 'acc-error', title: lastError.message || '' },
            h('span', { class: 'err-code', text: lastError.status ? String(lastError.status) : 'ERR' }),
            h('span', { class: 'when', text: fmt.ago(lastError.at) }),
            h('span', { class: 'msg', text: lastError.message || 'request failed' })) : null),
        h('div', { class: 'acc-limits ep-models' },
          h('span', { class: 'ep-count', text: models.length ? `${models.length} model${models.length === 1 ? '' : 's'}` : 'The provider’s known models' }),
          models.length ? h('div', { class: 'chips' },
            models.slice(0, 8).map((m) => h('code', { class: 'chip', text: m.alias || m.name, title: m.alias ? `${m.alias} → ${m.name}` : m.name })),
            models.length > 8 ? h('button', { class: 'lim-more', type: 'button', onclick: () => toggle(e.id) }, `+${models.length - 8} more`) : null)
            : h('p', { class: 'lim-none', text: e.kind === 'anthropic' ? 'Every Claude model the gateway knows' : 'Every model the gateway knows for it' }))),
      open ? endpointDetails(e) : null);
  }

  function endpointDetails(e) {
    const u = e.uptime || {};
    const errs = e.errors || [];
    return h('div', { class: 'acc-details' },
      h('section', null,
        h('h3', { text: 'Endpoint' }),
        kv('Kind', e.kind_name),
        e.name ? kv('Name', e.name) : null,
        kv('Base URL', e.base_url || `${e.host} (the kind's usual)`),
        kv('Key', e.key_hint ? `${e.key_hint}${e.keys > 1 ? ` · ${e.keys} keys` : ''}` : 'none'),
        e.prefix ? kv('Prefix', e.prefix) : null),
      h('section', null,
        h('h3', { text: `Requests · ${RANGES[ui.range].toLowerCase()}` }),
        kv('Succeeded', String(u.ok || 0)),
        kv('Failed', String(u.failed || 0), u.failed ? 'bad' : null),
        kv('Tokens in / out', `${fmt.tokens(u.input_tokens || 0)} / ${fmt.tokens(u.output_tokens || 0)}`),
        kv('Average latency', u.avg_latency_ms ? latency(u.avg_latency_ms) : '—'),
        kv('Last success', u.last_ok ? `${fmt.ago(u.last_ok)} ago` : '—')),
      (e.models || []).length ? h('section', { class: 'wide' },
        h('h3', { text: 'Models' }),
        h('div', { class: 'chips' }, e.models.map((m) => h('code', { class: 'chip', text: m.alias && m.alias !== m.name ? `${m.alias} → ${m.name}` : m.name })))) : null,
      h('section', { class: 'wide' },
        h('h3', { text: errs.length ? `Latest errors · ${errs.length}` : 'Latest errors' }),
        errs.length ? h('ol', { class: 'errs' }, errs.map((x) => h('li', null,
          h('span', { class: 'err-code', text: x.status ? String(x.status) : 'ERR' }),
          h('time', { text: fmt.stamp(x.at), datetime: x.at }),
          x.model ? h('span', { class: 'model', text: x.model }) : null,
          h('span', { class: 'msg', text: x.message || 'request failed' })))) : h('p', { class: 'none', text: 'None recorded.' })));
  }

  function endpointMenu(e) {
    const disabled = e.state === 'disabled';
    return [
      { icon: '✎', label: 'Edit…', run: () => addEndpoint(e.kind, e) },
      { icon: '↻', label: 'Check', detail: 'Ask the endpoint which models it serves, with its key', run: () => checkEndpoint(e) },
      { icon: disabled ? '▶' : '⏸', label: disabled ? 'Enable' : 'Disable', detail: disabled ? 'Let the gateway use it again' : 'Keep the gateway from using it', run: () => switchEndpoint(e, !disabled) },
      { icon: '▾', label: ui.expanded.has(e.id) ? 'Hide details' : 'Details', run: () => toggle(e.id) },
      { separator: true },
      { icon: '×', label: 'Remove', danger: true, confirm: 'Click again to remove', detail: 'Take it out of the gateway, with its history', run: () => removeEndpoint(e) },
    ];
  }

  async function checkEndpoint(e) {
    toast(`Asking ${e.name || e.host}…`, 'info', `check-${e.id}`);
    try {
      const { models } = await api('/api/endpoints/probe', { method: 'POST', body: { id: e.id, kind: e.kind, base_url: e.base_url } });
      toast(`${e.name || e.host}: the key works · ${models.length} models`, 'info', `check-${e.id}`);
    } catch (error) {
      toast(`${e.name || e.host}: ${error.message}`, 'error', `check-${e.id}`);
    }
  }

  async function switchEndpoint(e, disabled) {
    try {
      await api(`/api/endpoints/${encodeURIComponent(e.id)}`, { method: 'PATCH', body: { disabled } });
      toast(`${e.name || e.host} ${disabled ? 'disabled' : 'enabled'}`);
    } catch (error) {
      toast(error.message, 'error');
    }
    refresh();
  }

  async function removeEndpoint(e) {
    try {
      await api(`/api/endpoints/${encodeURIComponent(e.id)}`, { method: 'DELETE' });
      ui.expanded.delete(e.id);
      toast(`${e.name || e.host} removed`);
    } catch (error) {
      toast(error.message, 'error');
    }
    refresh();
  }

  // ---------------------------------------------------------------- endpoint dialog

  // addEndpoint opens the endpoint dialog: for a kind, or for an endpoint
  // the gateway has.
  function addEndpoint(kind = '', existing = null) {
    ctx.closeOverlays();
    const kinds = ui.data?.endpoint_kinds || [];
    const e = existing;
    if (kinds.length && !kinds.some((k) => k.id === kind)) kind = '';
    ui.endpoint = {
      id: e?.id || '', kind: kind || e?.kind || kinds[0]?.id || 'openai-compatible',
      name: e?.name || '', base: e?.base_url || '', key: '', keyHint: e?.key_hint || '',
      models: (e?.models || []).map((m) => (m.alias && m.alias !== m.name ? `${m.name} = ${m.alias}` : m.name)).join('\n'),
      prefix: e?.prefix || '', fetched: null, fetching: false, busy: false, error: '', note: '',
    };
    $('endpoint-dialog').hidden = false;
    renderEndpointDialog();
    if (!ui.data) refresh().then(renderEndpointDialog);
    setTimeout(() => $('endpoint-dialog').querySelector(e ? '#ep-models' : '.kind-grid button[aria-checked="true"]')?.focus(), 0);
  }

  function closeEndpoint() {
    if (!ui.endpoint) return false;
    ui.endpoint = null;
    $('endpoint-dialog').hidden = true;
    return true;
  }

  function endpointKindOf(id) {
    return (ui.data?.endpoint_kinds || []).find((k) => k.id === id) || { id, name: id, detail: '' };
  }

  // parseModels reads the models field: a model a line, "name = alias" to
  // serve it under another name.
  function parseModels(text) {
    return text.split(/\n|,/).map((line) => line.replace(/#.*/, '').trim()).filter(Boolean).map((line) => {
      const [name, alias = ''] = line.split(/\s*(?:=>|=)\s*/);
      return { name: name.trim(), alias: alias.trim() };
    });
  }

  function renderEndpointDialog() {
    const d = ui.endpoint;
    if (!d) return;
    const kind = endpointKindOf(d.kind);
    const editing = !!d.id;
    $('endpoint-title').textContent = editing ? `Edit endpoint · ${kind.name}` : 'Add endpoint';
    const field = (label, input, extra) => h('label', { class: 'field' }, h('span', { class: 'label' }, label, extra || null), input);
    const input = (key, attrs) => h('input', { type: 'text', spellcheck: 'false', autocomplete: 'off', value: d[key], oninput: (event) => { d[key] = event.target.value; }, ...attrs });
    const chosen = new Set(parseModels(d.models).map((m) => m.name));
    const body = [
      editing ? null : h('div', { class: 'kind-grid', role: 'radiogroup', 'aria-label': 'Kind' }, (ui.data?.endpoint_kinds || []).map((k) => h('button', {
        type: 'button', role: 'radio', 'aria-checked': String(k.id === d.kind),
        onclick: () => { d.kind = k.id; d.fetched = null; d.error = ''; renderEndpointDialog(); },
      }, h('span', { class: 'ptag', data: { kind: k.id }, text: k.name })))),
      h('p', { class: 'settings-note', text: `${kind.detail}. ${kind.needs_models ? 'The gateway serves the models listed below, and no others.' : 'Without a list, the gateway serves the models it knows for it.'}` }),
      h('div', { class: 'fields' },
        kind.named ? field('Name', input('name', { placeholder: 'openrouter', maxlength: 40 })) : null,
        field('Base URL', input('base', { type: 'url', inputmode: 'url', placeholder: kind.base_url || 'https://openrouter.ai/api/v1' })),
        field('API key', input('key', {
          class: 'secret', autocapitalize: 'off', 'data-1p-ignore': 'true', 'data-lpignore': 'true',
          placeholder: editing ? `Saved key ${d.keyHint || ''} — leave empty to keep it` : 'Paste the key',
        })),
        field('Models', h('textarea', {
          id: 'ep-models', rows: '4', spellcheck: 'false',
          placeholder: kind.needs_models ? 'One model a line; "name = alias" serves it under another name' : 'Optional: one model a line, to serve only these',
          oninput: (event) => { d.models = event.target.value; syncPicked(); },
        }, d.models), h('button', {
          class: 'act', type: 'button', disabled: d.fetching, title: 'Ask the endpoint which models it serves', onclick: fetchEndpointModels,
        }, d.fetching ? 'Fetching…' : 'Fetch models')),
        d.fetched ? h('div', { class: 'picked' },
          h('div', { class: 'picked-head' },
            h('span', { text: `${d.fetched.length} models at the endpoint` }),
            h('button', { class: 'act', type: 'button', onclick: () => pickAll(true) }, 'All'),
            h('button', { class: 'act', type: 'button', onclick: () => pickAll(false) }, 'None')),
          h('ul', null, d.fetched.map((m) => h('li', null, h('label', null,
            h('input', { type: 'checkbox', value: m, checked: chosen.has(m), onchange: (event) => pickModel(m, event.target.checked) }),
            h('code', { text: m })))))) : null,
        field('Prefix', input('prefix', { placeholder: 'Optional: serves the models as prefix/model' }))),
      h('div', { class: 'settings-status', id: 'ep-status', data: { kind: d.error ? 'error' : d.note ? 'ok' : '' }, role: 'status', text: d.error || d.note }),
    ];
    $('endpoint-body').replaceChildren(...body.filter(Boolean));
    $('endpoint-foot').replaceChildren(
      h('span', { class: 'settings-path', text: 'The key stays on this machine, in the gateway’s config.yaml' }),
      h('button', { class: 'act', type: 'button', onclick: closeEndpoint }, 'Cancel'),
      h('button', { class: 'primary', type: 'button', disabled: d.busy, onclick: saveEndpoint }, d.busy ? 'Saving…' : editing ? 'Save' : 'Add'));
  }

  function syncPicked() {
    const d = ui.endpoint;
    const chosen = new Set(parseModels(d.models).map((m) => m.name));
    for (const box of $('endpoint-body').querySelectorAll('.picked input[type="checkbox"]')) box.checked = chosen.has(box.value);
  }

  function pickModel(model, on) {
    const d = ui.endpoint;
    const models = parseModels(d.models);
    const next = on ? (models.some((m) => m.name === model) ? models : [...models, { name: model, alias: '' }]) : models.filter((m) => m.name !== model);
    d.models = next.map((m) => (m.alias ? `${m.name} = ${m.alias}` : m.name)).join('\n');
    const area = $('ep-models');
    if (area) area.value = d.models;
  }

  function pickAll(on) {
    const d = ui.endpoint;
    if (!d.fetched) return;
    const kept = parseModels(d.models).filter((m) => !d.fetched.includes(m.name));
    const next = on ? [...kept, ...d.fetched.map((name) => ({ name, alias: '' }))] : kept;
    d.models = next.map((m) => (m.alias ? `${m.name} = ${m.alias}` : m.name)).join('\n');
    const area = $('ep-models');
    if (area) area.value = d.models;
    syncPicked();
  }

  function endpointBody(d) {
    const body = { kind: d.kind, name: d.name.trim(), base_url: d.base.trim(), models: parseModels(d.models), prefix: d.prefix.trim() };
    if (d.key.trim()) body.api_key = d.key.trim();
    return body;
  }

  async function fetchEndpointModels() {
    const d = ui.endpoint;
    d.fetching = true;
    d.error = d.note = '';
    renderEndpointDialog();
    try {
      const { models } = await api('/api/endpoints/probe', { method: 'POST', body: { ...endpointBody(d), id: d.id } });
      if (ui.endpoint !== d) return;
      d.fetched = models;
      d.note = models.length ? `The key works: the endpoint serves ${models.length} models; check those the gateway should serve.` : 'The key works, but the endpoint lists no models: type them.';
      // A new endpoint that needs a list starts with nothing picked, so
      // hundreds of models do not flood the gateway by accident.
    } catch (error) {
      if (ui.endpoint === d) d.error = error.message;
    }
    if (ui.endpoint !== d) return;
    d.fetching = false;
    renderEndpointDialog();
  }

  async function saveEndpoint() {
    const d = ui.endpoint;
    d.busy = true;
    d.error = d.note = '';
    renderEndpointDialog();
    try {
      const body = endpointBody(d);
      const saved = d.id
        ? await api(`/api/endpoints/${encodeURIComponent(d.id)}`, { method: 'PUT', body })
        : await api('/api/endpoints', { method: 'POST', body });
      closeEndpoint();
      toast(`${saved.name || saved.host} ${d.id ? 'saved' : 'added'} · ${saved.models?.length ? `${saved.models.length} models` : 'the provider’s known models'}`);
        refresh();
    } catch (error) {
      if (ui.endpoint !== d) return;
      d.busy = false;
      d.error = error.message;
      renderEndpointDialog();
    }
  }

  // ---------------------------------------------------------------- sign-in

  // The sign-in dialog steps through choose → waiting → done or failed.
  function addAccount(providerID = '') {
    const provider = (ui.data?.providers || []).find((p) => p.id === providerID);
    ctx.closeOverlays();
    ui.signin = { step: 'choose' };
    $('signin').hidden = false;
    if (provider) begin(provider, null);
    renderSignIn();
    if (!ui.data) refresh().then(() => { if (ui.signin?.step === 'choose') renderSignIn(); });
  }

  function providerCard(p, tab = true) {
    const count = (ui.data?.accounts || []).filter((a) => a.provider === p.id).length;
    return h('button', {
      class: 'provider', type: 'button', data: { provider: p.id },
      onclick: () => {
        if (!ui.signin) { ui.signin = { step: 'choose' }; $('signin').hidden = false; }
        // Opened now, while the click lets the page open a tab; the
        // sign-in page goes into it once the gateway has one.
        begin(p, tab ? window.open('about:blank', '_blank') : null);
      },
    },
    h('span', { class: 'ptag', data: { provider: p.id }, text: p.name }),
    h('span', { class: 'pdetail', text: p.detail }),
    h('span', { class: 'pmeta', text: [p.flow === 'device' ? 'code' : 'browser', count ? `${count} signed in` : ''].filter(Boolean).join(' · ') }));
  }

  async function begin(provider, tab) {
    const current = { step: 'starting', provider, tab, before: new Set((ui.data?.accounts || []).map((a) => a.name)) };
    ui.signin = current;
    renderSignIn();
    let login;
    try {
      login = await api('/api/sign-ins', { method: 'POST', body: { provider: provider.id } });
    } catch (error) {
      tab?.close();
      if (ui.signin === current) Object.assign(current, { step: 'failed', error: error.message });
      renderSignIn();
      return;
    }
    if (ui.signin !== current) {
      api(`/api/sign-ins/${encodeURIComponent(login.state)}`, { method: 'DELETE' }).catch(() => {});
      return;
    }
    if (!/^https?:\/\//i.test(login.url || '')) {
      tab?.close();
      Object.assign(current, { step: 'failed', error: 'The gateway gave no sign-in page' });
      renderSignIn();
      return;
    }
    let opened = false;
    try {
      if (tab && !tab.closed) {
        tab.location.href = login.url;
        opened = true;
      }
    } catch { /* the tab went elsewhere */ }
    Object.assign(current, { step: 'waiting', login, opened, polls: 0 });
    renderSignIn();
    poll(current);
  }

  async function poll(current) {
    if (ui.signin !== current || current.step !== 'waiting') return;
    let state;
    try {
      state = await api(`/api/sign-ins/${encodeURIComponent(current.login.state)}`);
    } catch (error) {
      if (ui.signin !== current) return;
      if (error.status === 404) {
        Object.assign(current, { step: 'failed', error: 'The sign-in expired. Start it again.' });
        renderSignIn();
        return;
      }
      state = { status: 'wait' };
    }
    if (ui.signin !== current) return;
    if (state.status === 'ok') {
      Object.assign(current, { step: 'done' });
      watchNew(current.provider.id);
      ui.watch.before = current.before;
      current.onList = (list) => {
        const added = list.filter((a) => !current.before.has(a.name) && a.provider === current.provider.id);
        if (added.length) current.added = added;
        if (ui.signin === current) renderSignIn();
      };
      renderSignIn();
      refresh();
      return;
    }
    if (state.status === 'error') {
      Object.assign(current, { step: 'failed', error: state.error || 'The sign-in failed' });
      renderSignIn();
      return;
    }
    current.polls++;
    if (current.login.expires_at && Date.parse(current.login.expires_at) < Date.now()) {
      Object.assign(current, { step: 'failed', error: 'The code expired before it was entered. Start again.' });
      renderSignIn();
      return;
    }
    renderCountdown(current);
    current.timer = setTimeout(() => poll(current), 1500);
  }

  function renderCountdown(current) {
    const el = $('signin-expires');
    if (el && current.login?.expires_at) el.textContent = `Code expires in ${until(current.login.expires_at).replace(/^in /, '')}`;
  }

  async function submitCallback(current) {
    const input = $('signin-callback');
    const url = input.value.trim();
    if (!url) {
      input.focus();
      return;
    }
    current.callbackError = '';
    current.submitting = true;
    renderSignIn();
    try {
      await api(`/api/sign-ins/${encodeURIComponent(current.login.state)}/callback`, { method: 'POST', body: { url } });
      current.submitted = true;
    } catch (error) {
      current.callbackError = error.message;
    }
    current.submitting = false;
    if (ui.signin === current) renderSignIn();
  }

  function closeSignIn() {
    const current = ui.signin;
    if (!current) return false;
    clearTimeout(current.timer);
    if (current.step === 'waiting' && current.login) {
      api(`/api/sign-ins/${encodeURIComponent(current.login.state)}`, { method: 'DELETE' }).catch(() => {});
    }
    ui.signin = null;
    $('signin').hidden = true;
    return true;
  }

  // dismissSignIn closes the dialog unless a sign-in is under way in it.
  function dismissSignIn() {
    if (ui.signin?.step === 'waiting' || ui.signin?.step === 'starting') return false;
    return closeSignIn();
  }

  function renderSignIn() {
    const current = ui.signin;
    if (!current) return;
    const body = $('signin-body');
    const foot = $('signin-foot');
    const p = current.provider;
    $('signin-title').textContent = p ? `Add account · ${p.name}` : 'Add account';
    switch (current.step) {
      case 'choose': {
        const providers = ui.data?.providers || [];
        body.replaceChildren(
          h('p', { class: 'settings-note', text: 'Sign in with a subscription. The gateway keeps its token in its folder on this machine and uses the account for runs pointed at it.' }),
          providers.length ? h('div', { class: 'provider-grid' }, providers.map((provider) => providerCard(provider))) : loadingLine('Loading providers'));
        foot.replaceChildren(
          h('span', { class: 'settings-path', text: 'Or bring credentials another CLIProxyAPI signed in' }),
          h('button', { class: 'act', type: 'button', onclick: pickFiles }, 'Import files…'));
        break;
      }
      case 'starting':
        body.replaceChildren(loadingLine(`Starting the ${p.name} sign-in`));
        foot.replaceChildren(h('span', { class: 'settings-path' }), h('button', { class: 'act', type: 'button', onclick: closeSignIn }, 'Cancel'));
        break;
      case 'waiting': {
        const login = current.login;
        const device = login.flow === 'device';
        const link = h('a', { class: 'act strong open-link', href: login.url, target: '_blank', rel: 'noopener noreferrer' }, device ? 'Open the page ↗' : 'Open the sign-in page ↗');
        const parts = [];
        if (device) {
          parts.push(h('p', { class: 'settings-note', text: `Enter this code on the ${p.name} page${current.opened ? ' that opened in a new tab' : ''}, and confirm the sign-in there.` }));
          if (login.user_code) {
            parts.push(h('div', { class: 'user-code' },
              h('code', { text: login.user_code }),
              h('button', { class: 'act', type: 'button', onclick: () => copy(login.user_code, 'Code copied') }, 'Copy')));
          }
        } else {
          parts.push(h('p', { class: 'settings-note', text: current.opened
            ? `Sign in to ${p.name} in the tab that just opened. When it says you can close it, this dialog finishes on its own.`
            : `Open the ${p.name} sign-in page and sign in. When it says you can close it, this dialog finishes on its own.` }));
        }
        parts.push(h('div', { class: 'signin-actions' }, link,
          h('button', { class: 'act', type: 'button', onclick: () => copy(login.url, 'Link copied') }, 'Copy link')));
        parts.push(h('div', { class: 'state-line waiting' },
          h('span', { class: 'meter', 'aria-hidden': 'true' }, Array.from({ length: 8 }, () => h('i'))),
          current.submitted ? 'Finishing the sign-in' : `Waiting for ${p.name}`,
          device && login.expires_at ? h('span', { class: 'expires', id: 'signin-expires', text: `Code expires in ${until(login.expires_at).replace(/^in /, '')}` }) : null));
        if (!device) {
          const manual = h('details', { class: 'manual', open: login.manual || !!current.callbackError },
            h('summary', null, login.manual ? 'After signing in' : 'Browser shows a page that cannot be reached?'),
            h('p', { class: 'settings-note', text: login.manual
              ? `The ${p.name} page sends the browser to an address on this computer that nothing answers here. Copy that address from the browser's address bar and paste it below.`
              : 'The provider sends the browser back to localhost. If that page does not load — a browser on another machine, or the port was taken — copy its address and paste it here.' }),
            h('div', { class: 'callback-row' },
              h('input', { id: 'signin-callback', type: 'text', spellcheck: 'false', placeholder: 'http://localhost:…/callback?code=…&state=…', onkeydown: (event) => { if (event.key === 'Enter') { event.preventDefault(); submitCallback(current); } } }),
              h('button', { class: 'act strong', type: 'button', disabled: !!current.submitting, onclick: () => submitCallback(current) }, current.submitting ? 'Sending…' : 'Finish')),
            current.callbackError ? h('p', { class: 'settings-status', data: { kind: 'error' }, text: current.callbackError }) : null);
          parts.push(manual);
        }
        body.replaceChildren(...parts);
        foot.replaceChildren(h('span', { class: 'settings-path', text: login.manual ? 'No callback listens on this machine for this sign-in' : '' }),
          h('button', { class: 'act', type: 'button', onclick: closeSignIn }, 'Cancel'));
        break;
      }
      case 'done': {
        const added = current.added || [];
        body.replaceChildren(h('div', { class: 'signin-done' },
          h('span', { class: 'done-mark', 'aria-hidden': 'true', text: '✓' }),
          h('div', null,
            h('b', { text: added.length ? `Signed in as ${added.map((a) => a.label).join(', ')}` : `Signed in to ${p.name}` }),
            h('p', { class: 'settings-note', text: signedInNote(added) }),
            added.some((a) => a.models?.length) ? accountModels({ ...added[0], models: [...new Set(added.flatMap((a) => a.models || []))] }) : null)));
        foot.replaceChildren(h('span', { class: 'settings-path' }),
          h('button', { class: 'act', type: 'button', onclick: () => { ui.signin = { step: 'choose' }; renderSignIn(); } }, 'Add another'),
          h('button', { class: 'primary', type: 'button', onclick: closeSignIn }, 'Done'));
        foot.querySelector('.primary')?.focus();
        break;
      }
      case 'failed':
        body.replaceChildren(h('p', { class: 'settings-status', data: { kind: 'error' }, text: current.error }));
        foot.replaceChildren(h('span', { class: 'settings-path' }),
          h('button', { class: 'act', type: 'button', onclick: () => { ui.signin = { step: 'choose' }; renderSignIn(); } }, 'Back'),
          p ? h('button', { class: 'primary', type: 'button', onclick: () => begin(p, window.open('about:blank', '_blank')) }, 'Try again') : null);
        break;
      default:
    }
  }

  // ---------------------------------------------------------------- import

  function pickFiles() {
    $('account-files').value = '';
    $('account-files').click();
  }

  async function importFiles(files) {
    let added = 0;
    watchNew();
    for (const file of files) {
      if (file.size > 60_000) {
        toast(`${file.name} is too large for a credential`, 'error');
        continue;
      }
      try {
        await api('/api/accounts/import', { method: 'POST', body: { name: file.name, content: await file.text() } });
        added++;
      } catch (error) {
        toast(`${file.name}: ${error.message}`, 'error');
      }
    }
    if (added) {
      toast(added === 1 ? 'Account imported' : `${added} accounts imported`);
      if (ui.signin?.step === 'choose') closeSignIn();
    } else {
      clearTimeout(ui.watch?.timer);
      ui.watch = null;
    }
    refresh();
  }

  // signedInNote says whether the new account serves models yet.
  function signedInNote(added) {
    const models = new Set(added.flatMap((a) => a.models || []));
    if (models.size) return `The account is ready: runs through the gateway can use its ${models.size} model${models.size === 1 ? '' : 's'} now, and any prompt can pick one in the composer.`;
    if (ui.watch) return 'The gateway is taking the account up; its models show in a moment.';
    return 'The account is signed in. The gateway lists its models as soon as it has taken it up.';
  }

  // ---------------------------------------------------------------- keys

  // key handles a key pressed on the tab; it reports whether it did.
  function key(event) {
    if (!ui.open) return false;
    const actions = {
      '+': () => addMenu($('accounts-add-top')),
      r: () => refresh(),
      1: () => setRange('24h'),
      7: () => setRange('7d'),
    };
    const action = actions[event.key];
    if (!action) return false;
    event.preventDefault();
    action();
    return true;
  }

  // ---------------------------------------------------------------- helpers

  function kv(k, value, level, action) {
    return h('div', { class: `kv${level ? ` lv-${level}` : ''}` },
      h('span', { class: 'k', text: k }), h('span', { class: 'dots' }), h('span', { class: 'v', text: value, title: value }), action || null);
  }

  function stat(value, label, level) {
    const zero = value === 0 || value === '0';
    return h('div', { data: { level: zero ? null : level || null, zero: zero ? 'true' : null } }, h('b', { text: String(value) }), h('span', { text: label }));
  }

  function notice(title, text, g) {
    return h('section', { class: 'gw gw-off ticks', data: { row: 'notice' } },
      h('span', { class: 'label', text: 'Gateway' }),
      h('h2', { text: title }),
      h('p', { text }),
      g?.config ? h('p', { class: 'gw-path', text: g.config }) : null);
  }

  function loadingLine(text) {
    return h('div', { class: 'state-line', data: { row: 'loading' } }, h('span', { class: 'meter', 'aria-hidden': 'true' }, Array.from({ length: 8 }, () => h('i'))), text);
  }

  // destroy stops what the view has going, for a version loaded anew.
  function destroy() {
    ui.open = false;
    clearTimeout(ui.poll);
    clearTimeout(ui.watch?.timer);
    clearTimeout(ui.signin?.timer);
  }

  return {
    open, close, destroy, refresh, addAccount, addEndpoint, closeEndpoint, closeSignIn, dismissSignIn, key, summary,
    setRange, importFiles, pickFiles, addMenu,
    // focusConnection brings the connection's model into view.
    focusConnection: () => {
      $('accounts-scroll').scrollTo({ top: 0, behavior: 'smooth' });
      const focus = () => ($('conn-model') || query('.conn-modes [aria-checked="true"]'))?.focus();
      if (ui.data) setTimeout(focus, 50); else refresh().then(() => setTimeout(focus, 50));
    },
    get signingIn() { return !!ui.signin; },
    get endpointOpen() { return !!ui.endpoint; },
    providers: () => ui.data?.providers || [],
    visibility: () => { if (ui.open && document.visibilityState === 'visible') refresh({ quiet: true }).finally(schedule); },
  };
}

// ---------------------------------------------------------------- formats

// setText changes an element's text only when it differs, so text that stays
// keeps its selection.
function setText(el, text) {
  if (el.textContent !== text) el.textContent = text;
}

// describe says where runs of a resolved connection go.
function describe(link) {
  if (!link?.provider) return 'unknown';
  const parts = [link.provider];
  if (link.model) parts.push(link.model);
  if (link.base_url) parts.push(hostOf(link.base_url));
  parts.push(link.key ? `key from ${link.key}` : 'no key');
  return parts.join(' · ');
}

function hostOf(value) {
  try {
    return new URL(value).host;
  } catch {
    return value || '';
  }
}

// home shortens a path in the home directory with ~.
function home(path) {
  return String(path || '').replace(/^\/(Users|home)\/[^/]+/, '~');
}

function percent(ok, total) {
  if (!total) return '—';
  const value = (ok / total) * 100;
  return value === 100 ? '100%' : `${value >= 99.95 ? '99.9' : value.toFixed(value >= 10 ? 1 : 0)}%`;
}

// latency reads a duration in milliseconds.
function latency(ms) {
  return ms < 1000 ? `${Math.round(ms)}ms` : fmt.duration(ms);
}

function fmtPct(value) {
  const v = Math.max(0, Math.min(100, value || 0));
  return `${v >= 10 || v === 0 ? Math.round(v) : v.toFixed(1)}%`;
}

// until says how long until a time, as "in 2h 14m".
function until(value) {
  const ms = Date.parse(value) - Date.now();
  if (!Number.isFinite(ms)) return '';
  if (ms <= 0) return 'now';
  const m = Math.round(ms / 60000);
  if (m < 1) return `in ${Math.max(1, Math.round(ms / 1000))}s`;
  if (m < 60) return `in ${m}m`;
  const hrs = Math.floor(m / 60);
  if (hrs < 48) return `in ${hrs}h ${String(m % 60).padStart(2, '0')}m`;
  return `in ${Math.round(hrs / 24)}d`;
}

// resets says when a limit resets: soon ones in hours, later ones by day.
function resets(value) {
  const d = new Date(value);
  const ms = d.getTime() - Date.now();
  if (!Number.isFinite(ms)) return '';
  if (ms <= 0) return 'resetting';
  if (ms < 24 * 3600 * 1000) return `resets ${until(value)}`;
  return `resets ${d.toLocaleDateString(undefined, { weekday: 'short' })} ${fmt.short(value)}`;
}

function since(value) {
  const s = (Date.now() - Date.parse(value)) / 1000;
  if (!Number.isFinite(s)) return '';
  if (s < 60) return 'just now';
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ${Math.floor(s / 60) % 60}m`;
  return `${Math.floor(s / 86400)}d`;
}

function slotTime(start, step) {
  const a = new Date(start);
  const b = new Date(start + step);
  const day = step >= 3600 * 1000 * 2 || a.toDateString() !== new Date().toDateString()
    ? `${a.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' })} ` : '';
  return `${day}${fmt.short(a)}–${fmt.short(b)}`;
}
