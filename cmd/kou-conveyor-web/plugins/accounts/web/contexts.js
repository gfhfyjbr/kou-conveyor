// Model settings (Models, on the Accounts tab, after the endpoints): the
// context window and prompt-cache TTL each model's runs take. A run compacts
// its conversation before it outgrows the context window: the one the runner
// knows for the models of the providers it knows, else 128k tokens. Here the
// user sets a model's window, typed or as OpenRouter lists it, and its explicit
// cache TTL (/api/context-windows, /api/cache-ttls).
import { h, sync } from './gateway.js';

// SHOWN is how many models the list shows before "more", beside those a
// window or TTL is set for.
const SHOWN = 12;

// parseTokens reads a number of tokens as typed: 200000, 200 000, 200,000,
// 200k, 1M, 1.5m; null when it is none.
export function parseTokens(text) {
  let s = String(text ?? '').trim().toLowerCase().replace(/[\s_']/g, '');
  const suffix = /[km]$/.test(s) ? s.slice(-1) : '';
  s = suffix ? s.slice(0, -1).replace(',', '.') : s.replace(/,/g, '');
  if (!/^\d+(\.\d+)?$/.test(s)) return null;
  const n = Math.round(Number(s) * (suffix === 'm' ? 1e6 : suffix === 'k' ? 1e3 : 1));
  return Number.isSafeInteger(n) && n > 0 ? n : null;
}

// windowLabel reads a window briefly: 128k, 131.1k, 1M, 1.05M.
export function windowLabel(n) {
  if (!(n > 0)) return '—';
  if (n >= 1e6) return `${+(n / 1e6).toFixed(n % 1e6 ? 2 : 0)}M`;
  if (n >= 1e3) return `${+(n / 1e3).toFixed(n % 1e3 ? 1 : 0)}k`;
  return String(n);
}

const exact = (n) => `${Number(n).toLocaleString('en-US')} tokens`;

export function createContexts(ctx) {
  const { api, toast, $ } = ctx;
  const ui = {
    open: false,
    data: null, // the latest /api/context-windows answer
    ticket: 0,
    more: false, // every model listed, not only the first
    dialog: null, // the dialog's state while it is open
  };

  // ---------------------------------------------------------------- data

  async function refresh() {
    if (!ui.open && !ui.dialog) return;
    const ticket = ++ui.ticket;
    let data = null;
    try {
      data = await api('/api/context-windows');
    } catch {
      // The gateway is off: the gateway's card says so.
    }
    if (ticket !== ui.ticket) return;
    ui.data = data;
    render();
    if (ui.dialog) renderStatus();
  }

  function open() {
    if (ui.open) return;
    ui.open = true;
    render();
    refresh();
  }

  function close() {
    ui.open = false;
  }

  // byId is the listed model of an ID, in any case.
  function byId(id) {
    const key = String(id || '').trim().toLowerCase();
    return key ? (ui.data?.models || []).find((m) => m.id.toLowerCase() === key) || null : null;
  }

  // ---------------------------------------------------------------- the list

  function render() {
    const box = $('models-list');
    if (!box || !ui.open) return;
    const models = [...(ui.data?.models || [])];
    if (!models.length) {
      box.replaceChildren();
      return;
    }
    // The models a setting is saved for first, then those the gateway serves.
    models.sort((a, b) => (b.window || b.ttl ? 1 : 0) - (a.window || a.ttl ? 1 : 0) || (b.served ? 1 : 0) - (a.served ? 1 : 0) || a.id.localeCompare(b.id));
    const limit = Math.max(SHOWN, models.filter((m) => m.window || m.ttl).length);
    const shown = ui.more ? models : models.slice(0, limit);
    const head = h('div', { class: 'list-head', data: { row: 'head' } },
      h('span', { class: 'label', text: `Models · ${models.length}` }),
      h('span', { class: 'list-range', text: 'Each model’s context window and prompt-cache TTL' }),
      h('button', { class: 'act strong', type: 'button', title: 'Set the context window of a model', onclick: () => openWindow('') }, '+ Window'));
    const card = h('section', { class: 'u-card ctx-card', data: { row: 'windows' } },
      ui.data.error ? h('p', { class: 'settings-status', data: { kind: 'error' }, text: ui.data.error }) : null,
      h('table', { class: 'u-table ctx-table' },
        h('thead', null, h('tr', null,
          h('th', { text: 'Model' }),
          h('th', { class: 'num', text: 'Runs take', title: 'The window the model’s runs take: yours where you set one, else the runner’s own' }),
          h('th', { class: 'num', text: 'Cache TTL', title: 'The prompt-cache TTL the model’s next runs ask for' }),
          h('th', { class: 'num', text: 'Gateway', title: 'The window the gateway’s model registry gives the model' }),
          h('th', { class: 'num', text: 'OpenRouter', title: 'The window OpenRouter lists for the model, as of its list fetched last' }),
          h('th'))),
        h('tbody', null, shown.map(row))),
      models.length > limit ? h('button', { class: 'lim-more u-more', type: 'button', onclick: () => { ui.more = !ui.more; render(); } },
        ui.more ? 'Fewer models' : `+ ${models.length - limit} more model${models.length - limit === 1 ? '' : 's'}`) : null,
      h('p', { class: 'u-note u-fine', text: 'A run compacts its conversation before it outgrows the model’s context window: yours where you set one, else the one the runner knows for the model, else 128k. Cache TTL applies to providers with configurable prompt caching; Default leaves their own TTL unchanged. OpenRouter lists a model’s largest window; an account of a subscription may serve less of it than an API key does.' }));
    sync(box, [head, card]);
  }

  function cacheLabel(ttl) {
    return ttl === '5m' ? '5 min' : ttl === '1h' ? '1 hour' : 'Default';
  }

  function row(m) {
    const runs = m.window
      ? [h('b', { text: windowLabel(m.window), title: exact(m.window) }), h('span', {
        class: 'u-src', data: { source: m.from ? 'openrouter' : 'custom' }, text: m.from ? 'OpenRouter' : 'yours',
        title: m.from ? `Yours, as OpenRouter lists ${m.from}` : 'Yours',
      })]
      : h('span', { class: 'ctx-own', text: 'runner’s', title: 'The window the runner knows for the model, else 128k tokens' });
    return h('tr', { data: { row: `w:${m.id}` } },
      h('td', { class: 'u-name' }, h('div', { class: 'u-name-col' },
        h('span', { class: 'u-model', text: m.id, title: m.name || m.id }),
        m.name && m.name !== m.id ? h('small', { text: m.name }) : null,
        m.served ? null : h('small', { text: 'not served now' }))),
      h('td', { class: 'num ctx-runs' }, runs),
      h('td', { class: 'num' }, h('button', {
        class: 'act ctx-cache', type: 'button', text: cacheLabel(m.ttl),
        title: `Set the prompt-cache TTL of ${m.id}`, onclick: () => openCache(m.id),
      })),
      h('td', { class: 'num', text: windowLabel(m.gateway), title: m.gateway ? exact(m.gateway) : 'The gateway gives no window for it' }),
      h('td', {
        class: 'num', text: windowLabel(m.openrouter),
        title: m.openrouter ? `${exact(m.openrouter)}, as OpenRouter lists ${m.openrouter_id}` : 'OpenRouter lists no window for it, or its list is not fetched yet',
      }),
      h('td', { class: 'u-edit' }, h('button', { class: 'act', type: 'button', title: `Set the context window of ${m.id}`, onclick: () => openWindow(m.id) }, m.window ? 'Edit' : 'Set')));
  }

  // ---------------------------------------------------------------- the dialog

  // openWindow opens the dialog for a model's context window.
  function openWindow(model = '') {
    const m = byId(model);
    ui.dialog = {
      kind: 'context',
      model: m?.id || String(model || ''), tokens: m?.window ? String(m.window) : '',
      // fetched is the window OpenRouter lists for the model: as got, or as
      // the window set was taken from it.
      fetched: m?.window && m.from ? { model: m.id, tokens: m.window, openrouter: m.from } : null,
      fetching: false, busy: false, error: '', note: '',
    };
    ctx.openDialog();
    renderDialog();
    if (!ui.data) refresh();
    setTimeout(() => $('context-dialog')?.querySelector(model ? '#context-tokens' : '#context-model')?.focus(), 0);
  }

  // openCache opens the prompt-cache TTL editor for a model.
  function openCache(model = '') {
    const m = byId(model);
    ui.dialog = { kind: 'cache', model: m?.id || String(model || ''), ttl: m?.ttl || '', busy: false, error: '' };
    ctx.openDialog();
    renderCacheDialog();
    if (!ui.data) refresh();
    setTimeout(() => $('cache-ttl')?.focus(), 0);
  }

  function closeDialog() {
    if (!ui.dialog) return false;
    ui.dialog = null;
    ctx.closeDialog();
    return true;
  }

  function renderDialog() {
    const d = ui.dialog;
    const title = $('context-title');
    const body = $('context-body');
    const foot = $('context-foot');
    if (!d || !body) return;
    if (d.kind === 'cache') return renderCacheDialog();
    title.textContent = d.model ? `Context window of ${d.model}` : 'A model’s context window';
    const enter = (event) => {
      if (event.key !== 'Enter') return;
      event.preventDefault();
      save();
    };
    body.replaceChildren(
      h('label', { class: 'field' },
        h('span', { class: 'label', text: 'Model' }),
        h('input', {
          id: 'context-model', type: 'text', spellcheck: 'false', autocomplete: 'off', value: d.model, placeholder: 'claude-sonnet-4-5', list: 'context-models',
          oninput: (event) => { d.model = event.target.value.trim(); d.error = ''; d.note = ''; renderStatus(); },
          onkeydown: enter,
        }),
        h('datalist', { id: 'context-models' }, (ui.data?.models || []).map((m) => h('option', { value: m.id })))),
      h('label', { class: 'field' },
        h('span', { class: 'label', text: 'Context window · tokens' }),
        h('input', {
          id: 'context-tokens', type: 'text', inputmode: 'numeric', spellcheck: 'false', autocomplete: 'off', value: d.tokens, placeholder: '200000, 200k or 1M',
          oninput: (event) => { d.tokens = event.target.value; d.error = ''; renderStatus(); },
          onkeydown: enter,
        })),
      h('div', { class: 'ctx-fill' },
        h('button', { class: 'act', type: 'button', id: 'context-get', onclick: fetchWindow }, d.fetching ? 'Asking OpenRouter…' : 'Get from OpenRouter'),
        h('span', { id: 'context-gateway' })),
      h('p', { class: 'u-hint', id: 'context-hint' }),
      h('p', { class: 'settings-status', id: 'context-status', role: 'status' }));
    foot.replaceChildren(
      h('span', { class: 'settings-path', id: 'context-foot-note' }),
      h('button', { class: 'act', type: 'button', id: 'context-remove', onclick: () => remove(d.model, true) }, 'Remove mine'),
      h('button', { class: 'act', type: 'button', onclick: closeDialog }, 'Cancel'),
      h('button', { class: 'primary', type: 'button', id: 'context-save', onclick: save }, d.busy ? 'Saving…' : 'Save'));
    renderStatus();
  }

  function renderCacheDialog() {
    const d = ui.dialog;
    const body = $('context-body');
    const foot = $('context-foot');
    if (!d || d.kind !== 'cache' || !body) return;
    $('context-title').textContent = d.model ? `Prompt cache of ${d.model}` : 'A model’s prompt cache';
    const enter = (event) => {
      if (event.key !== 'Enter') return;
      event.preventDefault();
      saveCache();
    };
    body.replaceChildren(
      h('label', { class: 'field' },
        h('span', { class: 'label', text: 'Model' }),
        h('input', {
          id: 'cache-model', type: 'text', spellcheck: 'false', autocomplete: 'off', value: d.model, list: 'cache-models',
          oninput: (event) => { d.model = event.target.value.trim(); d.error = ''; renderCacheStatus(); },
          onkeydown: enter,
        }),
        h('datalist', { id: 'cache-models' }, (ui.data?.models || []).map((m) => h('option', { value: m.id })))),
      h('label', { class: 'field' },
        h('span', { class: 'label', text: 'Prompt-cache TTL' }),
        h('select', {
          id: 'cache-ttl', onchange: (event) => { d.ttl = event.target.value; d.error = ''; renderCacheStatus(); }, onkeydown: enter,
        },
        h('option', { value: '', selected: d.ttl === '' }, 'Default · provider decides'),
        h('option', { value: '5m', selected: d.ttl === '5m' }, '5 minutes · explicit 5m'),
        h('option', { value: '1h', selected: d.ttl === '1h' }, '1 hour · explicit 1h'))),
      h('p', { class: 'u-hint', id: 'cache-hint' }),
      h('p', { class: 'settings-status', id: 'cache-status', role: 'status' }));
    foot.replaceChildren(
      h('span', { class: 'settings-path', id: 'cache-foot-note' }),
      h('button', { class: 'act', type: 'button', onclick: closeDialog }, 'Cancel'),
      h('button', { class: 'primary', type: 'button', id: 'cache-save', onclick: saveCache }, d.busy ? 'Saving…' : 'Save'));
    renderCacheStatus();
  }

  function renderCacheStatus() {
    const d = ui.dialog;
    if (!d || d.kind !== 'cache') return;
    const m = byId(d.model);
    const hint = $('cache-hint');
    if (hint) {
      const says = ['An explicit TTL is sent with prompt-cache breakpoints. Five minutes costs less to write; one hour keeps the cache warm through longer gaps.'];
      if (!d.model) says.push('Name a model.');
      else if (!m?.served) says.push('The gateway does not serve this model now; the setting waits for it.');
      hint.textContent = says.join(' ');
    }
    const note = $('cache-foot-note');
    if (note) note.textContent = !d.ttl ? 'provider default' : m?.ttl && m.ttl !== d.ttl ? `${d.ttl} · replaces ${m.ttl}` : d.ttl;
    const status = $('cache-status');
    if (status) status.textContent = d.error;
    const save = $('cache-save');
    if (save) save.disabled = d.busy || !d.model;
  }

  // renderStatus updates what the dialog says as its fields change.
  function renderStatus() {
    const d = ui.dialog;
    if (!d) return;
    if (d.kind === 'cache') return renderCacheStatus();
    const m = byId(d.model);
    const tokens = parseTokens(d.tokens);
    const off = ui.data?.openrouter && !ui.data.openrouter.enabled;
    const get = $('context-get');
    if (get) {
      get.disabled = d.fetching || d.busy || off || !d.model;
      get.title = off ? 'Fetching from OpenRouter is off (KOU_CONVEYOR_OPENROUTER_PRICES=off)' : 'Get the context window OpenRouter lists for the model';
    }
    const gateway = $('context-gateway');
    if (gateway) {
      gateway.replaceChildren(m?.gateway ? h('button', {
        class: 'act', type: 'button', title: `${exact(m.gateway)}, as the gateway’s model registry gives it`,
        onclick: () => { d.tokens = String(m.gateway); d.error = ''; renderDialog(); },
      }, `Use the gateway’s ${windowLabel(m.gateway)}`) : '');
    }
    const hint = $('context-hint');
    if (hint) {
      const says = [];
      if (!d.model) says.push('Name a model the gateway serves.');
      else {
        says.push(m?.window ? `Its runs take ${windowLabel(m.window)}, yours${m.from ? `, as OpenRouter lists ${m.from}` : ''}.` : 'Its runs take the window the runner knows for it, else 128k.');
        if (!m?.served) says.push('The gateway does not serve it now; the window waits for it.');
      }
      if (d.note) says.push(d.note);
      const over = tokens && m?.gateway && tokens > m.gateway;
      hint.replaceChildren(says.join(' '), over ? h('span', {
        class: 'ctx-warn', text: ` More than the ${windowLabel(m.gateway)} the gateway gives it: requests may fail before the conversation is compacted.`,
      }) : '');
    }
    const remove = $('context-remove');
    if (remove) remove.hidden = !m?.window;
    const note = $('context-foot-note');
    if (note) note.textContent = [tokens ? exact(tokens) : '', m?.window ? 'replaces yours' : ''].filter(Boolean).join(' · ');
    const status = $('context-status');
    if (status) {
      status.textContent = d.error;
      status.dataset.kind = d.error ? 'error' : '';
    }
    const save = $('context-save');
    if (save) save.disabled = d.busy || !d.model || !tokens;
  }

  // fetchWindow fills in the window OpenRouter lists for the model.
  async function fetchWindow() {
    const d = ui.dialog;
    if (!d || d.fetching || !d.model) return;
    d.fetching = true;
    d.error = '';
    renderDialog();
    try {
      const got = await api(`/api/context-windows/openrouter?${new URLSearchParams({ model: d.model })}`);
      if (ui.dialog !== d) return;
      d.tokens = String(got.tokens);
      d.fetched = { model: d.model, tokens: got.tokens, openrouter: got.openrouter };
      d.note = `OpenRouter lists ${windowLabel(got.tokens)} as ${got.openrouter}.${got.note ? ` ${got.note}.` : ''}`;
      // The list's OpenRouter column follows the list fetched.
      refresh();
    } catch (error) {
      if (ui.dialog !== d) return;
      d.error = error.message;
    }
    d.fetching = false;
    renderDialog();
    $('context-dialog')?.querySelector('#context-tokens')?.focus();
  }

  async function save() {
    const d = ui.dialog;
    if (!d || d.busy) return;
    const tokens = parseTokens(d.tokens);
    if (!d.model || !tokens) {
      d.error = !d.model ? 'Name a model.' : 'A context window is a number of tokens: 200000, 200k or 1M.';
      renderStatus();
      return;
    }
    // The window keeps where it comes from while it is OpenRouter's.
    const same = d.fetched && d.fetched.model.toLowerCase() === d.model.toLowerCase() && d.fetched.tokens === tokens;
    d.busy = true;
    renderDialog();
    try {
      ui.data = await api('/api/context-windows', { method: 'POST', body: { model: d.model, tokens, openrouter: same ? d.fetched.openrouter : '' } });
      closeDialog();
      toast(`Context window saved · ${d.model} · ${windowLabel(tokens)}`);
      render();
      ctx.onChange?.();
    } catch (error) {
      if (ui.dialog !== d) return;
      d.busy = false;
      d.error = error.message;
      renderDialog();
    }
  }

  async function saveCache() {
    const d = ui.dialog;
    if (!d || d.kind !== 'cache' || d.busy) return;
    if (!d.model) {
      d.error = 'Name a model.';
      renderCacheStatus();
      return;
    }
    const previous = byId(d.model)?.ttl || '';
    d.busy = true;
    renderCacheDialog();
    try {
      if (d.ttl || previous) {
        ui.data = await api('/api/cache-ttls' + (d.ttl ? '' : `?${new URLSearchParams({ model: d.model })}`), {
          method: d.ttl ? 'POST' : 'DELETE',
          body: d.ttl ? { model: d.model, ttl: d.ttl } : undefined,
        });
      }
      closeDialog();
      toast(d.ttl ? `Prompt cache saved · ${d.model} · ${cacheLabel(d.ttl)}` : `Prompt cache of ${d.model} uses the provider’s default`);
      render();
      ctx.onChange?.();
    } catch (error) {
      if (ui.dialog !== d) return;
      d.busy = false;
      d.error = error.message;
      renderCacheDialog();
    }
  }

  async function remove(model, fromDialog = false) {
    try {
      ui.data = await api(`/api/context-windows?${new URLSearchParams({ model })}`, { method: 'DELETE' });
      if (fromDialog) closeDialog();
      toast(`Your context window of ${model} removed: its runs take the runner’s own`);
      ctx.onChange?.();
    } catch (error) {
      toast(error.message, 'error');
    }
    render();
  }

  function destroy() {
    ui.open = false;
    ui.dialog = null;
  }

  return {
    open, close, destroy, refresh, openWindow, openCache, closeDialog,
    get dialogOpen() { return !!ui.dialog; },
  };
}
