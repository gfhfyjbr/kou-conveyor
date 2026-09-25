// The model picker. A prompt runs with a model of its own choosing, from the
// models the connection reaches: through the gateway, every model of every
// account and endpoint, whichever provider serves it. The picker lists them
// grouped by provider, newest first, with a search field; the composer
// chooses the model of the next prompt with it, and the Accounts view the
// connection's default. Models that make media rather than answers are left
// out unless searched for, and an ID the list does not hold can be typed.
import { h } from '/kernel/dom.js';

// contextLabel says how much a context window holds: 200k, 1M.
export function contextLabel(tokens) {
  if (!tokens) return '';
  if (tokens >= 1e6) return `${Number((tokens / 1e6).toFixed(tokens % 1e6 ? 1 : 0))}M`;
  return `${Math.round(tokens / 1000)}k`;
}

export function findModel(catalog, id) {
  return (id && catalog?.models?.find((m) => m.id === id)) || null;
}

// FAMILIES tell who makes a model the list does not describe, by its name.
const FAMILIES = [
  [/^claude/, 'anthropic'], [/^(gpt|chatgpt|codex|o[134]\b|o[134]-)/, 'openai'], [/^(gemini|gemma)/, 'google'], [/^grok/, 'xai'],
  [/^(kimi|moonshot)/, 'moonshot'], [/^deepseek/, 'deepseek'], [/^(qwen|qwq)/, 'qwen'], [/^(llama|muse)/, 'meta'],
  [/^(mistral|codestral|devstral)/, 'mistral'], [/^glm/, 'zhipu'],
];

// providerOf is who makes a model: as the list says, else by its name.
export function providerOf(catalog, id) {
  const known = findModel(catalog, id);
  if (known?.provider) return known.provider;
  const name = String(id || '').toLowerCase();
  const slash = name.indexOf('/');
  if (slash > 0) return name.slice(0, slash);
  return FAMILIES.find(([re]) => re.test(name))?.[1] || '';
}

// modelTitle describes a model for a tooltip.
export function modelTitle(catalog, id, extra = '') {
  const m = findModel(catalog, id);
  if (!m) return [id, extra].filter(Boolean).join('\n');
  return [
    m.name && m.name !== m.id ? `${m.name} · ${m.id}` : m.id,
    [m.provider_name, m.context ? `${contextLabel(m.context)} context` : '', m.cooling ? 'cooling down' : ''].filter(Boolean).join(' · '),
    m.via?.length ? `Served by ${m.via.join(', ')}` : '',
    extra,
  ].filter(Boolean).join('\n');
}

let current = null; // the picker that is open

// closeModelPicker closes the picker, reporting whether one was open.
export function closeModelPicker() {
  if (!current) return false;
  current.close();
  return true;
}

export function modelPickerOpen() {
  return !!current;
}

// openModelPicker shows the picker by anchor, above it or below. It opens on
// the providers — each with its models' count, the newest of them, and
// whether it holds the model in use — and a provider opens on its models,
// sliding in; ← or Esc goes back. Typing searches every provider's models
// at once, or the models of the provider open.
// options: catalog ({ source, default, models, error }), loading, current
// (the ID chosen now), title, onPick(id), onRefresh() → Promise of a
// catalog, onConnection() to edit the connection, placement.
export function openModelPicker(options) {
  closeModelPicker();
  const o = { placement: 'above', ...options };
  let catalog = o.catalog;
  let loading = !!o.loading;
  let cursor = 0;
  let rows = []; // what ↑↓ go through: { kind: provider | model | free, id, key, el }
  let open = null; // the provider whose models show, by name; null for the providers
  const still = () => matchMedia('(prefers-reduced-motion: reduce)').matches;

  const search = h('input', {
    class: 'mp-search', type: 'text', spellcheck: 'false', autocomplete: 'off',
    placeholder: 'Search models, providers, accounts… or type a model ID',
    role: 'combobox', 'aria-expanded': 'true', 'aria-controls': 'mp-list', 'aria-label': 'Search models',
  });
  const list = h('ul', { class: 'mp-list', id: 'mp-list', role: 'listbox', 'aria-label': 'Models' });
  const stage = h('div', { class: 'mp-stage' }, list);
  const keys = h('span', { class: 'mp-keys' });
  const count = h('span', { class: 'mp-count' });
  const refresh = o.onRefresh && h('button', { class: 'act', type: 'button', title: 'Ask the connection again which models it has' }, 'Refresh');
  const connection = o.onConnection && h('button', { class: 'act', type: 'button', title: 'Where runs connect, and their default model' }, 'Connection…');
  const box = h('div', { class: 'model-picker', role: 'dialog', 'aria-label': o.title || 'Choose a model', data: { placement: o.placement } },
    h('header', { class: 'mp-head' }, h('span', { class: 'label', text: o.title || 'Model' }), h('span', { class: 'mp-note', text: o.note || '' }), keys),
    search, stage,
    h('footer', { class: 'mp-foot' }, count, refresh, connection));

  function place() {
    const r = o.anchor.getBoundingClientRect();
    const width = Math.min(520, window.innerWidth - 16);
    box.style.width = `${width}px`;
    box.style.left = `${Math.max(8, Math.min(r.left, window.innerWidth - width - 8))}px`;
    if (o.placement === 'above') {
      box.style.top = '';
      box.style.bottom = `${window.innerHeight - r.top + 6}px`;
      box.style.maxHeight = `${Math.max(220, r.top - 16)}px`;
    } else {
      box.style.bottom = '';
      box.style.top = `${r.bottom + 6}px`;
      box.style.maxHeight = `${Math.max(220, window.innerHeight - r.bottom - 16)}px`;
    }
  }

  // matches reports whether a model has every word of the query, in its ID,
  // name, provider or the accounts that serve it.
  function matches(m, words) {
    const hay = [m.id, m.name, m.provider_name, m.provider, ...(m.via || [])].join(' ').toLowerCase();
    return words.every((w) => hay.includes(w));
  }

  // groups are the providers, as the list orders them, with their models;
  // those that make media rather than answers are apart.
  function groups() {
    const out = new Map();
    for (const m of catalog?.models || []) {
      const key = m.provider_name || 'Other';
      if (!out.has(key)) out.set(key, { key, provider: m.provider || '', models: [], media: [] });
      out.get(key)[m.media ? 'media' : 'models'].push(m);
    }
    return [...out.values()];
  }

  const words = () => search.value.trim().toLowerCase().split(/\s+/).filter(Boolean);

  // render shows the pane in order: a provider's models, what a search
  // finds, or the providers. direction slides the new pane in: 1 forward,
  // -1 back, 0 in place.
  function render(direction = 0) {
    const typed = search.value.trim();
    const query = words();
    const all = catalog?.models || [];
    const byProvider = groups();
    if (open && !byProvider.some((g) => g.key === open)) open = null;
    const nodes = [];
    rows = [];
    if (loading && !all.length) nodes.push(h('li', { class: 'mp-note-row', text: 'Asking the connection which models it has…' }));
    if (catalog?.error) nodes.push(h('li', { class: 'mp-note-row', data: { kind: 'error' }, text: catalog.error }));
    else if (catalog && !catalog.source && !all.length) {
      nodes.push(h('li', { class: 'mp-note-row', text: 'This connection lists no models. Type the ID of the one to use.' }));
    }
    if (open) {
      // A provider's models, under the way back.
      const g = byProvider.find((x) => x.key === open);
      const shown = [...g.models, ...(query.length ? g.media : [])].filter((m) => matches(m, query));
      nodes.push(h('li', { class: 'mp-back', role: 'presentation', data: { provider: g.provider || null } },
        h('button', {
          type: 'button', title: 'All providers (←)',
          onmousedown: (event) => { event.preventDefault(); back(); },
        }, 'Providers'),
        h('i', { class: 'mp-dot', 'aria-hidden': 'true' }), h('b', { text: g.key }),
        h('small', { text: `${g.models.length} model${g.models.length === 1 ? '' : 's'}` })));
      for (const m of shown) nodes.push(row({ model: m }));
      if (!shown.length && !typed) nodes.push(h('li', { class: 'mp-note-row', text: 'Only models that make media: search for one by name.' }));
    } else if (query.length) {
      // What a search finds, among every provider's models.
      let group = null;
      const shown = all.filter((m) => matches(m, query));
      for (const m of shown) {
        const name = m.provider_name || 'Other';
        if (name !== group) {
          group = name;
          const n = shown.filter((x) => (x.provider_name || 'Other') === name).length;
          nodes.push(h('li', { class: 'mp-group', role: 'presentation', data: { provider: m.provider || null } }, h('span', { text: name }), h('small', { text: String(n) })));
        }
        nodes.push(row({ model: m }));
      }
    } else {
      // The providers.
      const visible = byProvider.filter((g) => g.models.length);
      for (const g of visible) nodes.push(providerRow(g));
      if (!visible.length && all.length) nodes.push(h('li', { class: 'mp-note-row', text: 'Only models that make media: search for one by name.' }));
    }
    // An ID the list does not hold can still be used.
    if (typed && !/\s/.test(typed) && !all.some((m) => m.id === typed)) nodes.push(row({ free: typed }));
    if (!nodes.length) nodes.push(h('li', { class: 'mp-note-row', text: 'No model matches.' }));
    swap(nodes, direction);
    cursor = Math.max(0, Math.min(cursor, rows.length - 1));
    select(cursor, direction !== 0);
    keys.replaceChildren(...(open
      ? [h('kbd', { text: '←' }), ' back · ', h('kbd', { text: '↑' }), h('kbd', { text: '↓' }), ' ', h('kbd', { text: '↵' })]
      : [h('kbd', { text: '↑' }), h('kbd', { text: '↓' }), ' ', h('kbd', { text: '↵' }), ' open · ', h('kbd', { text: 'Esc' })]));
    const providers = new Set(all.filter((m) => !m.media).map((m) => m.provider_name));
    const usable = all.filter((m) => !m.media).length;
    count.textContent = loading ? 'Loading…' : all.length
      ? `${usable} model${usable === 1 ? '' : 's'} · ${providers.size} provider${providers.size === 1 ? '' : 's'}${catalog.source === 'gateway' ? ' · gateway' : ''}`
      : '';
  }

  // swap puts the new pane in the old one's place: the old slides out and
  // fades, the new slides in, the box grows or shrinks to it, and the rows
  // come in one after another.
  function swap(nodes, direction) {
    if (!direction || still() || !box.isConnected) {
      list.replaceChildren(...nodes);
      return;
    }
    const before = box.offsetHeight;
    const scrolled = list.scrollTop;
    const ghost = list.cloneNode(true);
    ghost.classList.add('mp-ghost');
    ghost.removeAttribute('id');
    ghost.setAttribute('aria-hidden', 'true');
    for (const el of ghost.querySelectorAll('[id]')) el.removeAttribute('id');
    stage.append(ghost);
    ghost.scrollTop = scrolled;
    list.replaceChildren(...nodes);
    list.scrollTop = 0;
    const after = box.offsetHeight;
    const ease = 'cubic-bezier(0.22, 1, 0.36, 1)';
    const shift = 44 * direction;
    ghost.animate([{ transform: 'none', opacity: 1 }, { transform: `translateX(${-shift}px)`, opacity: 0 }], { duration: 190, easing: ease, fill: 'forwards' })
      .finished.then(() => ghost.remove(), () => ghost.remove());
    list.animate([{ transform: `translateX(${shift}px)`, opacity: 0 }, { transform: 'none', opacity: 1 }], { duration: 280, easing: ease });
    if (Math.abs(before - after) > 1) box.animate([{ height: `${before}px` }, { height: `${after}px` }], { duration: 260, easing: ease });
    [...list.children].slice(0, 16).forEach((el, i) => {
      el.animate([{ opacity: 0, transform: 'translateY(6px)' }, { opacity: 1, transform: 'none' }], { duration: 240, delay: 50 + i * 22, easing: ease, fill: 'backwards' });
    });
  }

  function providerRow(g) {
    const n = rows.length;
    const holds = (id) => g.models.some((m) => m.id === id) || g.media.some((m) => m.id === id);
    const cooling = g.models.filter((m) => m.cooling).length;
    const flags = [];
    if (holds(o.current)) flags.push(h('span', { class: 'mp-flag current', text: 'current' }));
    else if (holds(catalog?.default)) flags.push(h('span', { class: 'mp-flag', text: 'default' }));
    if (cooling) flags.push(h('span', { class: 'mp-flag cooling', text: `${cooling} cooling`, title: 'Models every account that serves them waits out a limit for' }));
    const newest = g.models.slice(0, 3).map((m) => m.name || m.id);
    const li = h('li', {
      class: 'mp-row mp-prov', role: 'option', id: `mp-${n}`, 'aria-selected': 'false', 'aria-haspopup': 'listbox',
      data: { provider: g.provider || null, current: holds(o.current) ? 'true' : null },
      title: `${g.key}: ${g.models.length} model${g.models.length === 1 ? '' : 's'}`,
      onmousemove: () => { if (cursor !== n) select(n, false); },
      onmousedown: (event) => {
        event.preventDefault();
        enter(g.key);
      },
    },
    h('i', { class: 'mp-dot', 'aria-hidden': 'true' }),
    h('span', { class: 'mp-prov-name' }, h('b', { text: g.key }),
      h('small', { text: newest.join(' · ') + (g.models.length > newest.length ? ' …' : '') })),
    h('span', { class: 'mp-flags' }, flags),
    h('span', { class: 'mp-prov-count', text: String(g.models.length) }),
    h('span', { class: 'mp-arrow', 'aria-hidden': 'true' }));
    rows.push({ kind: 'provider', key: g.key, el: li });
    return li;
  }

  function row(item) {
    const n = rows.length;
    const m = item.model;
    const id = m ? m.id : item.free;
    const flags = [];
    if (id === o.current) flags.push(h('span', { class: 'mp-flag current', text: 'current' }));
    if (m && id === catalog?.default) flags.push(h('span', { class: 'mp-flag', text: 'default' }));
    if (m?.cooling) flags.push(h('span', { class: 'mp-flag cooling', text: 'cooling', title: 'Every account that serves it waits out a limit; the gateway takes it again once one has' }));
    if (m?.media) flags.push(h('span', { class: 'mp-flag', text: 'media' }));
    const li = h('li', {
      class: 'mp-row', role: 'option', id: `mp-${n}`, 'aria-selected': 'false',
      data: { provider: m?.provider || null, free: item.free ? 'true' : null, current: id === o.current ? 'true' : null },
      title: m ? modelTitle(catalog, id) : `Use ${id}, which the connection does not list`,
      onmousemove: () => { if (cursor !== n) select(n, false); },
      onmousedown: (event) => {
        event.preventDefault();
        pick(n);
      },
    },
    h('i', { class: 'mp-dot', 'aria-hidden': 'true' }),
    m
      ? h('span', { class: 'mp-name' }, h('b', { text: m.name || m.id }), m.name && m.name !== m.id ? h('code', { text: m.id }) : null)
      : h('span', { class: 'mp-name' }, h('b', { text: `Use “${id}”` }), h('code', { text: 'not listed' })),
    h('span', { class: 'mp-ctx', text: m?.context ? contextLabel(m.context) : '' }),
    h('span', { class: 'mp-flags' }, flags));
    rows.push({ kind: item.free ? 'free' : 'model', id, el: li });
    return li;
  }

  function select(n, scroll = true) {
    rows[cursor]?.el.setAttribute('aria-selected', 'false');
    cursor = n;
    const r = rows[n];
    if (!r) {
      search.removeAttribute('aria-activedescendant');
      return;
    }
    r.el.setAttribute('aria-selected', 'true');
    search.setAttribute('aria-activedescendant', r.el.id);
    if (scroll) r.el.scrollIntoView({ block: 'nearest' });
  }

  // enter opens a provider's models; back goes to the providers, on the
  // one that was open.
  function enter(key) {
    open = key;
    cursor = 0;
    search.value = '';
    render(1);
    const at = rows.findIndex((r) => r.id === o.current);
    if (at >= 0) select(at);
    search.focus();
  }

  function back() {
    if (!open) return;
    const from = open;
    open = null;
    search.value = '';
    render(-1);
    const at = rows.findIndex((r) => r.key === from);
    if (at >= 0) select(at);
    search.focus();
  }

  function pick(n) {
    const r = rows[n];
    if (!r) return;
    if (r.kind === 'provider') {
      enter(r.key);
      return;
    }
    close();
    o.onPick?.(r.id);
  }

  search.addEventListener('input', () => {
    cursor = 0;
    render();
  });
  search.addEventListener('keydown', (event) => {
    if (event.isComposing) return;
    const empty = !search.value;
    switch (event.key) {
      case 'ArrowDown':
      case 'ArrowUp': {
        event.preventDefault();
        if (rows.length) select((cursor + (event.key === 'ArrowDown' ? 1 : rows.length - 1)) % rows.length);
        break;
      }
      case 'PageDown':
      case 'PageUp':
        event.preventDefault();
        if (rows.length) select(Math.max(0, Math.min(rows.length - 1, cursor + (event.key === 'PageDown' ? 8 : -8))));
        break;
      case 'ArrowRight':
        if (empty && rows[cursor]?.kind === 'provider') {
          event.preventDefault();
          enter(rows[cursor].key);
        }
        break;
      case 'ArrowLeft':
      case 'Backspace':
        if (empty && open) {
          event.preventDefault();
          back();
        }
        break;
      case 'Enter':
        event.preventDefault();
        pick(cursor);
        break;
      case 'Escape':
        event.preventDefault();
        event.stopPropagation();
        // Esc steps back: out of a search, out of a provider, then away.
        if (!empty) {
          search.value = '';
          cursor = 0;
          render();
        } else if (open) {
          back();
        } else {
          close();
          o.anchor?.focus?.();
        }
        break;
      case 'Tab':
        event.preventDefault();
        break;
      default:
    }
  });
  refresh?.addEventListener('click', async () => {
    loading = true;
    render();
    try {
      const next = await o.onRefresh();
      if (next) catalog = next;
    } finally {
      loading = false;
      if (current === handle) render();
    }
  });
  connection?.addEventListener('click', () => {
    close();
    o.onConnection();
  });

  const outside = (event) => {
    if (!box.contains(event.target) && !o.anchor.contains(event.target)) close();
  };
  const onResize = () => place();
  // It stays by its anchor as the page scrolls; its own list scrolls too.
  const onScroll = (event) => { if (!box.contains(event.target)) place(); };

  function close() {
    if (current !== handle) return;
    current = null;
    document.removeEventListener('mousedown', outside, true);
    window.removeEventListener('resize', onResize);
    window.removeEventListener('scroll', onScroll, true);
    box.remove();
    o.onClose?.();
  }

  const handle = {
    close,
    // update shows a catalog that arrived while the picker is open.
    update(next, stillLoading = false) {
      catalog = next;
      loading = stillLoading;
      if (!open && !search.value && groups().filter((g) => g.models.length).length === 1) open = groups().find((g) => g.models.length).key;
      render();
    },
  };
  current = handle;
  document.body.append(box);
  place();
  // With one provider there is nothing to choose among: its models show.
  const single = groups().filter((g) => g.models.length);
  if (single.length === 1) open = single[0].key;
  render();
  // Open on the model in use, or its provider.
  const inUse = (catalog?.models || []).find((m) => m.id === o.current);
  const at = rows.findIndex((r) => r.id === o.current || (inUse && r.kind === 'provider' && r.key === (inUse.provider_name || 'Other')));
  if (at >= 0) select(at);
  document.addEventListener('mousedown', outside, true);
  window.addEventListener('resize', onResize);
  window.addEventListener('scroll', onScroll, true);
  search.focus();
  return handle;
}
