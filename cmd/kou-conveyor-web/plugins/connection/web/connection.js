// connection: where runs connect. The foot of the rail says it (rail.foot)
// and has a button for it (rail.actions); "," and /settings open it: the
// Accounts view's connection when the accounts plugin is there, which sends
// runs through the gateway, else the settings of a connection straight to
// an endpoint — the dialog the Accounts view opens for one too. It provides
// the connection service: open(), openDirect(), describe(link), host(url),
// viaGateway(url, link).
const SLIDERS = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M1.5 4.5h6.5M12 4.5h2.5M1.5 11.5h2.5M8 11.5h6.5" stroke="currentColor" stroke-width="1.4"/><rect x="8.5" y="2.5" width="3" height="4" fill="none" stroke="currentColor" stroke-width="1.4"/><rect x="4.5" y="9.5" width="3" height="4" fill="none" stroke="currentColor" stroke-width="1.4"/></svg>';
const API_LABEL = { responses: 'Responses API', messages: 'Messages API' };

export default function activate(cockpit) {
  const { h, svg } = cockpit;
  const session = cockpit.use('session');
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const state = () => session.state || {};
  const toast = (text, kind = 'info') => cockpit.toast(text, kind);

  function host(url) {
    try {
      return new URL(url).host;
    } catch {
      return url;
    }
  }

  // viaGateway reports whether a base URL is the accounts gateway's.
  function viaGateway(url, link) {
    if (link?.source === 'gateway') return true;
    const gateway = state().config?.accounts;
    if (!gateway?.enabled || !gateway.url || !url) return false;
    try {
      const a = new URL(url);
      const b = new URL(gateway.url);
      const local = (name) => ['127.0.0.1', 'localhost', '[::1]', '::1'].includes(name);
      return a.port === b.port && (a.hostname === b.hostname || (local(a.hostname) && local(b.hostname)));
    } catch {
      return false;
    }
  }

  function describe(link) {
    if (link?.source === 'gateway') return ['gateway', link.model, API_LABEL[link.provider_type]].filter(Boolean).join(' · ');
    if (!link?.provider) return 'unknown';
    const parts = [link.provider];
    if (link.model) parts.push(link.model);
    if (link.base_url) parts.push(viaGateway(link.base_url) ? 'accounts gateway' : host(link.base_url));
    parts.push(link.key ? `key from ${link.key}` : 'no key');
    return parts.join(' · ');
  }

  // ---------------------------------------------------------------- the rail

  const summary = h('span', { id: 'conn-summary', text: '—' });
  const link = h('button', { class: 'link-state', id: 'conn-open', type: 'button', title: 'Connection settings (,)', onclick: () => open() },
    h('span', { class: 'label', text: 'Connection' }), summary);
  cockpit.ui.mount('rail.foot', { id: 'connection', order: 10, node: link });
  cockpit.ui.mount('rail.actions', {
    id: 'settings-open', order: 10,
    node: h('button', { class: 'icon', id: 'settings-open', type: 'button', title: 'Connection settings (,)', 'aria-label': 'Connection settings', onclick: () => open() }, svg(SLIDERS)),
  });
  cockpit.on('render', () => {
    const text = describe(session.currentWorkspace?.()?.connection || state().config?.connection);
    if (summary.textContent !== text) summary.textContent = text;
  });

  // open shows where runs connect: the Accounts view's connection, which
  // sends them through the gateway, or else the settings.
  function open() {
    service('overlays')?.closeTop?.();
    const accounts = service('accounts');
    if (accounts?.showConnection) return accounts.showConnection();
    return openDirect();
  }

  // ---------------------------------------------------------------- the settings

  const ui = { data: null, clearKey: false, ticket: 0, checks: 0 };
  const radios = [['', 'Environment', 'Runner defaults'], ['responses', 'Responses', 'OpenAI API'], ['messages', 'Messages', 'Anthropic API']]
    .map(([value, label, small]) => h('label', null, h('input', { type: 'radio', name: 'api', value }), h('span', null, h('b', { text: label }), h('small', { text: small }))));
  const note = h('p', { class: 'settings-note', id: 'settings-note' });
  const base = h('input', { id: 'settings-base', type: 'url', inputmode: 'url', spellcheck: 'false', autocomplete: 'off' });
  const keyClear = h('button', { class: 'act', id: 'settings-key-clear', type: 'button' }, 'Remove saved key');
  const key = h('input', { id: 'settings-key', class: 'secret', type: 'text', spellcheck: 'false', autocomplete: 'off', autocapitalize: 'off', 'data-1p-ignore': true, 'data-lpignore': 'true' });
  const models = h('datalist', { id: 'settings-models' });
  const model = h('input', { id: 'settings-model', type: 'text', list: 'settings-models', spellcheck: 'false', autocomplete: 'off' });
  const fields = h('div', { class: 'fields', id: 'settings-fields' },
    h('label', { class: 'field' }, h('span', { class: 'label', text: 'Base URL' }), base),
    h('label', { class: 'field' }, h('span', { class: 'label' }, 'API key ', keyClear), key),
    h('label', { class: 'field' }, h('span', { class: 'label', text: 'Model' }), model, models));
  const status = h('div', { class: 'settings-status', id: 'settings-status', role: 'status', 'aria-live': 'polite' });
  const where = h('span', { class: 'settings-path', id: 'settings-path' });
  const check = h('button', { class: 'act', id: 'settings-check', type: 'button' }, 'Check connection');
  const save = h('button', { class: 'primary', id: 'settings-save', type: 'submit' }, 'Save');
  const form = h('form', { class: 'settings ticks', id: 'settings-form', role: 'dialog', 'aria-modal': 'true', 'aria-labelledby': 'settings-title', novalidate: true },
    h('header', null, h('span', { class: 'label', id: 'settings-title', text: 'Connection' }),
      h('button', { class: 'icon', id: 'settings-close', type: 'button', 'aria-label': 'Close', onclick: () => closeDirect() }, '×')),
    h('div', { class: 'settings-body' },
      h('fieldset', { class: 'seg', id: 'settings-api' }, h('legend', { class: 'label', text: 'Provider type' }), radios),
      note, fields, status),
    h('footer', null, where, check, save));
  const overlay = h('div', { class: 'overlay', id: 'settings', hidden: true, onmousedown: (event) => { if (event.target === overlay) closeDirect(); } }, form);
  cockpit.ui.mount('overlays', { id: 'settings', order: 50, node: overlay });
  cockpit.contribute('overlay', { id: 'settings', order: 50, modal: true, isOpen: () => !overlay.hidden, close: () => closeDirect() });

  const selectedAPI = () => form.querySelector('input[name="api"]:checked')?.value ?? '';
  const sameURL = (a, b) => (a || '').trim().replace(/\/+$/, '').toLowerCase() === (b || '').trim().replace(/\/+$/, '').toLowerCase();
  const say = (text, kind = '') => {
    status.textContent = text;
    status.dataset.kind = kind;
  };
  const wsQuery = () => `ws=${encodeURIComponent(state().ws || '')}`;

  async function openDirect() {
    service('overlays')?.closeTop?.();
    const ticket = ++ui.ticket;
    overlay.hidden = false;
    say('Loading…');
    let data;
    try {
      data = await cockpit.api(`/api/settings?${wsQuery()}`);
    } catch (error) {
      if (ticket === ui.ticket) say(error.message, 'error');
      return;
    }
    if (ticket !== ui.ticket || overlay.hidden) return;
    fill(data);
    say(data.error ? `${data.error}. Saving replaces them.` : '', data.error ? 'error' : '');
    form.querySelector('input[name="api"]:checked')?.focus();
  }

  function closeDirect() {
    ui.ticket++;
    overlay.hidden = true;
    // A typed key never outlives the dialog.
    key.value = '';
  }

  function fill(data) {
    ui.data = data;
    ui.clearKey = false;
    for (const radio of form.querySelectorAll('input[name="api"]')) radio.checked = radio.value === data.provider_type;
    base.value = data.base_url || '';
    model.value = data.model || '';
    key.value = '';
    models.replaceChildren();
    renderForm();
  }

  function renderForm() {
    const data = ui.data;
    if (!data) return;
    const api = selectedAPI();
    const defaults = data.defaults?.[api] || {};
    const connection = data.connection || {};
    fields.hidden = api === '';
    base.placeholder = defaults.base_url || '';
    model.placeholder = defaults.model ? `${defaults.model} (default)` : '';

    // The saved key goes only to the endpoint it was saved for.
    const sameEndpoint = api === data.provider_type && sameURL(base.value || defaults.base_url, data.base_url || defaults.base_url);
    const keep = data.api_key_set && sameEndpoint && !ui.clearKey;
    if (keep) key.placeholder = `Saved key ${data.api_key_hint} — leave empty to keep it`;
    else if (data.api_key_set && !sameEndpoint) key.placeholder = 'Enter the key for this endpoint';
    else key.placeholder = defaults.key_variable ? `Paste a key, or leave empty to use ${defaults.key_variable}` : 'Paste a key';
    keyClear.hidden = !keep;

    let text;
    if (api === '') {
      text = `Runs use the server's environment: KOU_CONVEYOR_LLM_PROVIDER, _BASE_URL, _API_KEY and _MODEL, or the provider's own key variable. Now: ${describe(connection)}.`;
    } else if (api === 'responses') {
      text = 'OpenAI Responses API, or any endpoint that implements it. The base URL usually ends in /v1.';
    } else {
      text = 'Anthropic Messages API, or a gateway that implements it. Claude models get adaptive thinking and prompt caching.';
    }
    if (data.provider_flag) text += ` The server was started with -provider ${data.provider_flag}, which takes precedence until it restarts without it.`;
    if (data.gateway_in_use) text += ' These settings point runs at the accounts gateway; the Accounts tab (A) changes its model.';
    else if (data.gateway && api !== '') text += ` The accounts gateway at ${host(data.gateway)} serves signed-in subscriptions: Accounts tab (A) → Use for runs.`;
    note.textContent = text;
    where.textContent = data.available ? `Saved to ${data.path} · applies to the next run` : 'Settings need a user config directory; set KOU_CONVEYOR_CONFIG';
    save.disabled = !data.available;
  }

  function draft() {
    const body = { provider_type: selectedAPI(), base_url: base.value.trim(), model: model.value.trim() };
    const typed = key.value.trim();
    if (typed) body.api_key = typed;
    else if (ui.clearKey) body.api_key = '';
    return body;
  }

  // A check answers for the form as it was sent; editing the form, or
  // starting another check, makes its answer stale.
  async function checkSettings() {
    const ticket = ui.ticket;
    const number = ++ui.checks;
    const current = () => ticket === ui.ticket && number === ui.checks;
    say('Checking…');
    try {
      const result = await cockpit.api(`/api/settings/check?${wsQuery()}`, { method: 'POST', body: draft() });
      if (!current()) return;
      say(result.message, result.ok ? 'ok' : 'error');
      models.replaceChildren(...(result.models || []).map((id) => h('option', { value: id })));
    } catch (error) {
      if (current()) say(error.message, 'error');
    }
  }

  async function saveSettings() {
    const ticket = ui.ticket;
    say('Saving…');
    let data;
    try {
      data = await cockpit.api(`/api/settings?${wsQuery()}`, { method: 'PUT', body: draft() });
    } catch (error) {
      if (ticket !== ui.ticket) return;
      say(error.message, 'error');
      if (error.body?.field === 'api_key') key.focus();
      return;
    }
    if (ticket !== ui.ticket) return;
    closeDirect();
    toast(`Connection saved · ${describe(data.connection)}`);
    session.refreshConfig();
    service('accounts')?.refresh?.({ quiet: true });
  }

  cockpit.listen(form, 'submit', (event) => {
    event.preventDefault();
    saveSettings();
  });
  cockpit.listen(form, 'change', (event) => {
    if (event.target.name === 'api') {
      // A base URL and model belong to their API: the saved ones come back
      // with it, and another API starts from its defaults.
      ui.checks++; // a check still running answered for the old type
      const saved = ui.data?.provider_type === event.target.value;
      base.value = saved ? ui.data.base_url || '' : '';
      model.value = saved ? ui.data.model || '' : '';
      models.replaceChildren();
      say('');
    }
    renderForm();
  });
  cockpit.listen(form, 'input', (event) => {
    if (event.target.name === 'api') return; // handled on change
    if (/^Checking/.test(status.textContent)) say('');
    ui.checks++;
    if (event.target === base) renderForm();
  });
  cockpit.listen(check, 'click', checkSettings);
  cockpit.listen(keyClear, 'click', () => {
    ui.clearKey = true;
    key.value = '';
    renderForm();
    say('The saved key will be removed when you save', 'warn');
  });
  // Tab stays inside the dialog while it is open.
  cockpit.listen(form, 'keydown', (event) => {
    if (event.key !== 'Tab') return;
    const focusable = [...form.querySelectorAll('button:not(:disabled), input:not([type="radio"]):not(:disabled), input[type="radio"]:checked')]
      .filter((el) => el.offsetParent !== null);
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  });

  cockpit.provide('connection', { open, openDirect, closeDirect, describe, host, viaGateway });
  cockpit.commands.register({ name: 'settings', aliases: ['config', 'connection'], help: 'Connection: the model runs use through the accounts gateway', order: 150, run: open });
  cockpit.keys.register({ key: ',', run: open });
  cockpit.contribute('help.keys', { keys: [','], text: 'Connection: the model runs use through the gateway', order: 120 });
  cockpit.palette.register({ group: 'Actions', icon: '⚙', label: 'Connection', hint: ',', detail: 'The model runs use through the gateway', order: 180, run: open });
  cockpit.render();
}
