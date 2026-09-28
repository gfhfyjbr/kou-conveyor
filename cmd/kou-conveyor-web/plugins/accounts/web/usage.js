// The Usage tab of the Accounts view: what the gateway's requests used —
// tokens by model, account and kind, over a day, a week, a month or a
// quarter — and what they come to as API credits, at the providers' list
// prices or the user's own; and the prices tokens are counted at, which the
// user sets for the models the cockpit does not know.
import { fmt } from '/kernel/dom.js';
import { h, sync } from './gateway.js';
import { DOLLARS, count, inUnit, isDollars, money, moneyShort, rate, tokens as tokenCount, unitName } from './format.js';

const fmtTokens = (n) => tokenCount(n, fmt.tokens);

const POLL = 30_000;
export const USAGE_RANGES = {
  '24h': { label: 'Last 24 hours', short: '24h', days: 1 },
  '7d': { label: 'Last 7 days', short: '7d', days: 7 },
  '30d': { label: 'Last 30 days', short: '30d', days: 30 },
  '90d': { label: 'Last 90 days', short: '90d', days: 90 },
};
const RANGE_KEYS = { 1: '24h', 7: '7d', 3: '30d', 9: '90d' };
const METRICS = [['cost', 'Cost'], ['tokens', 'Tokens'], ['requests', 'Requests']];
const SPLITS = [['model', 'By model'], ['kind', 'By kind']];
// The kinds of token API prices tell apart.
const KINDS = [
  ['input', 'Input', 'Fresh input: neither read from a cache nor written to one'],
  ['cache_read', 'Cache read', 'Input read from the provider’s cache, at a fraction of the input price'],
  ['cache_write', 'Cache write', 'Input written to the provider’s cache, at a premium for Claude'],
  ['output', 'Output', 'What the model wrote, its thinking included'],
];
const SOURCES = {
  list: ['list', 'The provider’s list price'],
  estimate: ['≈ est.', 'Estimated: the price of another model of the family'],
  custom: ['yours', 'A price you set'],
  openrouter: ['openrouter', 'The price OpenRouter lists, fetched twice a day'],
  none: ['no price', 'No price known: its tokens are left out of the cost'],
};
// Models with a colour of their own; the rest share "other".
const SERIES = 6;
const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];

const zero = () => ({ input: 0, cache_read: 0, cache_write: 0, output: 0, reasoning: 0, other: 0 });
const tokensOf = (t) => (t ? t.input + t.cache_read + t.cache_write + t.output + (t.other || 0) : 0);
const costOf = (c) => (c ? c.input + c.cache_read + c.cache_write + c.output : 0);
const addTo = (into, from) => {
  for (const key of Object.keys(into)) into[key] += from?.[key] || 0;
  return into;
};

// glob matches a model's ID with a pattern where * stands for anything, as
// the server does.
export function glob(pattern, id) {
  const parts = String(pattern).toLowerCase().split('*');
  id = String(id).toLowerCase();
  if (parts.length === 1) return parts[0] === id;
  if (!id.startsWith(parts[0])) return false;
  let rest = id.slice(parts[0].length);
  const last = parts[parts.length - 1];
  for (const part of parts.slice(1, -1)) {
    const at = rest.indexOf(part);
    if (at < 0) return false;
    rest = rest.slice(at + part.length);
  }
  return rest.endsWith(last) && rest.length >= last.length;
}

// families are the patterns a model's price could be set for: the model,
// then ever wider families of it ("claude-opus-5-5", "claude-opus-5*",
// "claude-opus-*", "claude-*").
export function families(id) {
  const parts = String(id).split('-');
  const out = [String(id)];
  for (let n = parts.length - 1; n >= 1; n--) {
    const stem = parts.slice(0, n).join('-');
    out.push(/\d$/.test(stem) ? `${stem}*` : `${stem}-*`);
  }
  return [...new Set(out)].slice(0, 4);
}

export function createUsage(ctx) {
  const { api, toast, $, prefs } = ctx;
  const range = prefs.get('usage-range', '7d');
  const ui = {
    open: false,
    range: USAGE_RANGES[range] ? range : '7d',
    metric: METRICS.some(([m]) => m === prefs.get('usage-metric', 'cost')) ? prefs.get('usage-metric', 'cost') : 'cost',
    split: prefs.get('usage-split', 'model') === 'kind' ? 'kind' : 'model',
    filter: { provider: '', account: '', model: '' },
    data: null, // the latest /api/usage answer
    error: '',
    loading: false,
    ticket: 0,
    poll: 0,
    prices: null, // the latest /api/usage/prices answer
    pricesError: '',
    moreModels: false, // the price table lists the models only served too
    dialog: null, // the price or unit dialog's draft; see renderDialog
    hover: -1, // the slot under the pointer
    fetching: false, // OpenRouter's prices are being fetched
  };
  const tip = h('div', { class: 'u-tip', role: 'tooltip', hidden: true });

  // ---------------------------------------------------------------- data

  function zone() {
    try {
      return Intl.DateTimeFormat().resolvedOptions().timeZone || '';
    } catch {
      return '';
    }
  }

  async function refresh({ quiet = false } = {}) {
    const ticket = ++ui.ticket;
    if (!quiet) ui.loading = !ui.data;
    const query = new URLSearchParams({ range: ui.range, tz: zone(), offset: String(-new Date().getTimezoneOffset()) });
    for (const [key, value] of Object.entries(ui.filter)) if (value) query.set(key, value);
    let data;
    try {
      data = await api(`/api/usage?${query}`);
    } catch (error) {
      if (ticket !== ui.ticket) return;
      ui.error = error.message;
      ui.loading = false;
      render();
      return;
    }
    if (ticket !== ui.ticket) return;
    ui.data = data;
    ui.error = '';
    ui.loading = false;
    render();
  }

  async function loadPrices() {
    try {
      ui.prices = await api('/api/usage/prices');
      ui.pricesError = '';
    } catch (error) {
      ui.pricesError = error.message;
    }
    render();
    if (ui.dialog) renderDialog();
  }

  function schedule() {
    clearTimeout(ui.poll);
    if (!ui.open || document.visibilityState !== 'visible') return;
    ui.poll = setTimeout(() => refresh({ quiet: true }).finally(schedule), POLL);
  }

  // ---------------------------------------------------------------- the tab

  function open() {
    if (ui.open) return;
    ui.open = true;
    render();
    refresh().finally(schedule);
    loadPrices();
  }

  function close() {
    ui.open = false;
    clearTimeout(ui.poll);
    hideTip();
  }

  function setRange(next) {
    if (!USAGE_RANGES[next] || next === ui.range) return;
    ui.range = next;
    prefs.set('usage-range', next);
    ui.data = ui.data && { ...ui.data, stale: true };
    render();
    refresh();
  }

  function setFilter(kind, value) {
    ui.filter[kind] = ui.filter[kind] === value ? '' : value;
    render();
    refresh();
  }

  function clearFilter() {
    ui.filter = { provider: '', account: '', model: '' };
    render();
    refresh();
  }

  function setMetric(metric) {
    ui.metric = metric;
    prefs.set('usage-metric', metric);
    render();
  }

  function setSplit(split) {
    ui.split = split;
    prefs.set('usage-split', split);
    render();
  }

  const unit = () => ui.data?.unit || ui.prices?.unit || DOLLARS;
  const filtered = () => Object.values(ui.filter).some(Boolean);

  // ---------------------------------------------------------------- sums

  const zeroCost = () => ({ input: 0, cache_read: 0, cache_write: 0, output: 0 });

  // sums gathers a report's rows by slot, model and account.
  function sums(d) {
    const slots = d.starts.map(() => ({ requests: 0, failed: 0, tokens: zero(), costs: zeroCost(), cost: 0, models: new Map() }));
    const models = new Map();
    const accounts = new Map();
    for (const r of d.rows || []) {
      const slot = slots[r.s];
      if (!slot) continue;
      const cost = costOf(r.c);
      slot.requests += r.n;
      slot.failed += r.f || 0;
      addTo(slot.tokens, r.t);
      addTo(slot.costs, r.c);
      slot.cost += cost;
      const m = slot.models.get(r.m || '') || { requests: 0, tokens: 0, cost: 0 };
      m.requests += r.n;
      m.tokens += tokensOf(r.t);
      m.cost += cost;
      slot.models.set(r.m || '', m);
      for (const [map, key] of [[models, r.m || ''], [accounts, r.a || '']]) {
        const e = map.get(key) || { id: key, requests: 0, failed: 0, tokens: zero(), costs: zeroCost(), cost: 0 };
        e.requests += r.n;
        e.failed += r.f || 0;
        addTo(e.tokens, r.t);
        addTo(e.costs, r.c);
        e.cost += cost;
        map.set(key, e);
      }
    }
    return { slots, models, accounts };
  }

  // colours gives the costliest models of the range, whatever the filter, a
  // colour of their own, the same in the rail, the chart and the tables.
  function colours(d) {
    const map = new Map();
    (d.facets?.models || []).slice(0, SERIES).forEach((f, i) => map.set(f.id, String(i)));
    return map;
  }

  const accountOf = (id) => ui.data?.accounts?.find((a) => a.id === id) || { id, label: id || 'No account' };
  const modelOf = (id) => ui.data?.models?.find((m) => m.id === id) || { id, price: { source: 'none' } };
  const providerName = (id) => ui.data?.accounts?.find((a) => a.provider === id && a.provider_name)?.provider_name
    || (id ? id.charAt(0).toUpperCase() + id.slice(1) : 'No account');
  const valueOf = (metric, v, short = false) => (metric === 'cost' ? (short ? moneyShort(v, unit()) : money(v, unit()))
    : metric === 'tokens' ? fmtTokens(Math.round(v)) : count(Math.round(v)));
  const pct = (part, whole) => (whole > 0 ? `${(part / whole * 100).toFixed(part / whole >= 0.995 || part / whole < 0.1 ? 0 : 1)}%` : '—');

  // ---------------------------------------------------------------- rendering

  function render() {
    if (!ui.open) return;
    renderBar();
    renderRail();
    renderBody();
  }

  function renderBar() {
    for (const button of $('usage-range')?.querySelectorAll('button') || []) {
      button.setAttribute('aria-checked', String(button.dataset.range === ui.range));
    }
  }

  function renderRail() {
    const d = ui.data;
    const item = (row, active, dot, label, value, onclick, title = '') => h('button', {
      type: 'button', class: 'filter u-filter', 'aria-pressed': String(active), onclick, title: title || label, data: { row },
    }, h('span', { class: 'dot', data: dot }), h('span', { class: 't', text: label }), h('span', { class: 'n', text: value }));
    const worth = (f) => (f.cost > 0 ? moneyShort(f.cost, unit()) : f.tokens ? fmtTokens(f.tokens) : String(f.requests));
    const facets = d?.facets || { providers: [], accounts: [], models: [] };
    const total = facets.providers.reduce((sum, f) => sum + f.cost, 0);
    const colourMap = d ? colours(d) : new Map();
    setText($('usage-total'), d ? moneyShort(total, unit()) : '');
    sync($('usage-provider-filter'), [
      item('all', !ui.filter.provider, {}, 'All providers', d ? moneyShort(total, unit()) : '', () => { if (ui.filter.provider) setFilter('provider', ui.filter.provider); }),
      ...facets.providers.map((f) => item(`p:${f.id}`, ui.filter.provider === f.id, { health: f.unpriced_tokens && !f.cost ? 'off' : 'ok' },
        providerName(f.id), worth(f), () => setFilter('provider', f.id))),
    ]);
    sync($('usage-account-filter'), facets.accounts.map((f) => {
      const a = accountOf(f.id);
      return item(`a:${f.id}`, ui.filter.account === f.id, { health: a.gone ? 'off' : null }, a.label,
        worth(f), () => setFilter('account', f.id), `${a.provider_name ? `${a.provider_name} · ` : ''}${a.label}${a.gone ? ' · no longer in the gateway' : ''}`);
    }));
    sync($('usage-model-filter'), facets.models.map((f) => item(`m:${f.id}`, ui.filter.model === f.id, { s: colourMap.get(f.id) ?? 'o' },
      f.id || 'Unknown model', worth(f), () => setFilter('model', f.id))));
    for (const [id, list] of [['usage-account-label', facets.accounts], ['usage-model-label', facets.models]]) {
      const el = $(id);
      if (el) el.hidden = !list.length;
    }
    const note = $('usage-rail-note');
    if (note) {
      const since = d?.since ? new Date(d.since) : null;
      setText(note, since ? `Recorded since ${since.toLocaleDateString(undefined, { day: 'numeric', month: 'short' })}, ${fmt.short(since)} · kept for 92 days, in usage.json beside the accounts` : 'Recording starts with the first request through the gateway');
    }
  }

  function renderBody() {
    const box = $('usage-body');
    if (!box) return;
    const d = ui.data;
    if (!d) {
      sync(box, [ui.error ? failure('Could not load the usage', ui.error) : loadingLine('Adding up the usage')]);
      return;
    }
    const agg = sums(d);
    const colourMap = colours(d);
    const empty = !(d.rows || []).length && !d.totals.requests;
    const nodes = [
      ui.error ? failure('Could not refresh the usage', ui.error) : null,
      d.prices_error ? failure('Could not read your prices', `${d.prices_error}; saving a price replaces the file`) : null,
      filterLine(d),
      kpis(d),
    ];
    if (empty && !filtered()) {
      nodes.push(emptyCard(d));
    } else {
      nodes.push(chartCard(d, agg, colourMap),
        h('div', { class: 'u-split', data: { row: 'split' } }, modelsCard(d, agg, colourMap), accountsCard(d, agg)),
        h('div', { class: 'u-split u-split-even', data: { row: 'split2' } }, mixCard(d), heatCard(d)));
    }
    nodes.push(pricesCard(d));
    sync(box, nodes);
    box.dataset.stale = d.stale ? 'true' : '';
  }

  function failure(title, text) {
    return h('div', { class: 'load-error', data: { row: `error:${title}` } }, h('b', { text: title }), h('span', { text }));
  }

  function loadingLine(text) {
    return h('div', { class: 'state-line', data: { row: 'loading' } }, h('span', { class: 'meter', 'aria-hidden': 'true' }, Array.from({ length: 8 }, () => h('i'))), text);
  }

  // filterLine says what the tab is narrowed to, and undoes it.
  function filterLine(d) {
    if (!filtered()) return null;
    const parts = [];
    if (ui.filter.provider) parts.push(['provider', providerName(ui.filter.provider)]);
    if (ui.filter.account) parts.push(['account', accountOf(ui.filter.account).label]);
    if (ui.filter.model) parts.push(['model', ui.filter.model]);
    return h('div', { class: 'u-filters', data: { row: 'filters' } },
      h('span', { class: 'label', text: 'Only' }),
      parts.map(([kind, label]) => h('button', { type: 'button', class: 'chip u-chip', title: 'Show them all again', onclick: () => setFilter(kind, ui.filter[kind]) }, label, h('span', { 'aria-hidden': 'true', text: ' ×' }))),
      h('button', { type: 'button', class: 'lim-more', onclick: clearFilter }, 'Show everything'),
      h('span', { class: 'list-range', text: `${d.totals.requests} of the range's requests` }));
  }

  // ---------------------------------------------------------------- figures

  function kpis(d) {
    const t = d.totals;
    const p = d.previous || {};
    const cost = costOf(t.cost);
    const before = costOf(p.cost);
    const tokens = tokensOf(t.tokens);
    const input = t.tokens.input + t.tokens.cache_read + t.tokens.cache_write + (t.tokens.other || 0);
    const from = Date.parse(d.from);
    const to = Date.parse(d.to);
    const since = d.since ? Date.parse(d.since) : NaN;
    // The span before counts only once the ledger covered all of it.
    const comparable = Number.isFinite(since) && since <= from - (to - from) + 3600e3 && (before > 0 || cost > 0);
    let delta = null;
    if (comparable) {
      const change = before > 0 ? (cost - before) / before : null;
      delta = h('span', { class: 'u-delta', data: { trend: change == null ? 'new' : change > 0.005 ? 'up' : change < -0.005 ? 'down' : 'flat' } },
        change == null ? `new: nothing the ${USAGE_RANGES[d.range].short} before` : `${change > 0 ? '▲' : change < 0 ? '▼' : '='} ${Math.abs(change * 100).toFixed(Math.abs(change) < 0.1 ? 1 : 0)}% on the ${USAGE_RANGES[d.range].short} before (${money(before, unit())})`);
    }
    // A pace: the cost of the time recorded, stretched to thirty days.
    const recorded = Math.min(Date.now(), to) - Math.max(from, Number.isFinite(since) ? since : from);
    const pace = recorded >= 3 * 3600e3 && cost > 0 ? cost / recorded * 30 * 86400e3 : 0;
    const unpriced = t.unpriced_tokens || 0;
    const ok = t.requests - t.failed;
    return h('section', { class: 'u-kpis ticks', data: { row: 'kpis' } },
      h('div', { class: 'u-kpi u-kpi-cost' },
        h('span', { class: 'label', text: `API cost · ${USAGE_RANGES[d.range].short}` }),
        h('b', { class: 'u-big', text: money(cost, unit()), title: `${money(cost)} at the providers' API prices` }),
        delta,
        h('span', { class: 'u-sub', text: pace ? `≈ ${money(pace, unit())} a month at this pace` : 'what the tokens would cost as API credits' }),
        unpriced ? h('button', {
          type: 'button', class: 'u-warn', title: 'Their models have no price: set one',
          onclick: () => $('usage-prices')?.scrollIntoView({ block: 'start', behavior: 'smooth' }),
        }, `+ ${fmtTokens(unpriced)} tokens without a price`) : null),
      h('div', { class: 'u-kpi' },
        h('span', { class: 'label', text: 'Tokens' }),
        h('b', { class: 'u-mid', text: fmtTokens(tokens) }),
        h('span', { class: 'u-sub', text: `${fmtTokens(t.tokens.input + (t.tokens.other || 0))} fresh in · ${fmtTokens(t.tokens.cache_read + t.tokens.cache_write)} cache · ${fmtTokens(t.tokens.output)} out` })),
      h('div', { class: 'u-kpi' },
        h('span', { class: 'label', text: 'Requests' }),
        h('b', { class: 'u-mid', text: count(t.requests) }),
        h('span', { class: 'u-sub', text: [t.requests ? `${pct(ok, t.requests)} ok` : 'none', t.avg_latency_ms ? `avg ${latency(t.avg_latency_ms)}` : null, ok && cost ? `${money(cost / ok, unit())} each` : null].filter(Boolean).join(' · ') })),
      h('div', { class: 'u-kpi' },
        h('span', { class: 'label', text: 'Cache' }),
        h('b', { class: 'u-mid', text: input ? pct(t.tokens.cache_read, input) : '—' }),
        h('span', { class: 'u-sub', text: input ? `of the input read from a cache${t.cache_saved ? ` · ${t.cache_saved > 0 ? 'saved' : 'cost'} ${money(Math.abs(t.cache_saved), unit())}` : ''}` : 'no input yet' })));
  }

  function emptyCard(d) {
    return h('section', { class: 'u-card u-empty', data: { row: 'empty' } },
      h('span', { class: 'label', text: d.since ? 'No requests in this range' : 'No usage recorded yet' }),
      h('h2', { text: d.since ? `Nothing went through the gateway in the ${USAGE_RANGES[d.range].label.toLowerCase()}` : 'Tokens turn into API credits here' }),
      h('p', { text: 'The gateway reports every request it sends upstream: the account and model that served it, and the tokens it used by kind — fresh input, cache reads, cache writes and output. Priced as the providers’ APIs price them, they are what the same work would have cost as API credits, hour by hour, model by model.' }),
      h('p', { class: 'u-note', text: d.since ? `Recorded since ${fmt.stamp(d.since)}.` : 'The first run through the gateway starts the record; it keeps 92 days.' }));
  }

  // chartCard draws the range slot by slot: stacked by model or by kind,
  // in cost, tokens or requests.
  function chartCard(d, agg, colourMap) {
    const series = seriesOf(d, agg, colourMap);
    const n = d.starts.length;
    const totals = Array.from({ length: n }, (_, i) => series.reduce((sum, s) => sum + s.values[i], 0));
    // The top reads well in the unit amounts show in.
    const peak = Math.max(0, ...totals);
    const u = unit();
    const top = ui.metric === 'cost' && !isDollars(u) ? niceMax(inUnit(peak, u)) * u.usd : niceMax(peak, ui.metric === 'requests');
    const since = d.since ? Date.parse(d.since) : Infinity;
    const end = (i) => Date.parse(i + 1 < n ? d.starts[i + 1] : d.to);
    const bars = d.starts.map((_, i) => {
      const segments = series.map((s) => {
        const v = s.values[i];
        if (!(v > 0)) return null;
        const seg = h('i', { data: { s: s.colour } });
        seg.style.height = `${Math.min(100, (v / top) * 100)}%`;
        return seg;
      });
      return h('div', {
        class: 'u-bar', data: { slot: String(i), before: end(i) <= since ? 'true' : null, now: i === n - 1 ? 'true' : null, hover: ui.hover === i ? 'true' : null },
      }, segments);
    });
    const grid = [0, 1, 2, 3, 4].map((k) => {
      const line = h('div', { class: 'u-gl' }, h('span', { text: k ? valueOf(ui.metric, (top * k) / 4, true) : '' }));
      line.style.bottom = `${k * 25}%`;
      return line;
    });
    const axis = ticks(d).map(({ i, label }) => {
      const tick = h('span', { text: label });
      tick.style.left = `${((i + 0.5) / n) * 100}%`;
      return tick;
    });
    const legend = series.map((s) => h('span', { class: 'u-key', title: s.title || s.label },
      h('i', { data: { s: s.colour } }), h('span', { class: 't', text: s.label }), h('b', { text: valueOf(ui.metric, s.values.reduce((a, b) => a + b, 0)) })));
    const firstRecorded = d.starts.findIndex((_, i) => end(i) > since);
    const unrecorded = Number.isFinite(since) && firstRecorded > n / 6
      ? h('span', { class: 'u-unrec', text: 'not recorded yet' }) : null;
    if (unrecorded) unrecorded.style.width = `${(firstRecorded / n) * 100}%`;
    const total = totals.reduce((a, b) => a + b, 0);
    const plot = h('div', {
      class: 'u-plot', role: 'img',
      'aria-label': `${METRICS.find(([m]) => m === ui.metric)[1]} over the ${USAGE_RANGES[d.range].label.toLowerCase()}: ${valueOf(ui.metric, total)}`,
    },
    h('div', { class: 'u-grid' }, grid),
    h('div', {
      class: 'u-bars',
      onmousemove: (event) => hoverAt(event, d, agg, series),
      onmouseleave: () => hideTip(),
    }, bars, unrecorded),
    h('div', { class: 'u-axis' }, axis));
    return h('section', { class: 'u-card u-chart', data: { row: 'chart' } },
      h('header', { class: 'u-head' },
        h('span', { class: 'label', text: 'Over time' }),
        h('span', { class: 'u-note', text: `${USAGE_RANGES[d.range].label} · ${slotName(d)}` }),
        h('span', { class: 'u-flex' }),
        segmented('u-metric', METRICS, ui.metric, setMetric),
        segmented('u-split-by', SPLITS, ui.split, setSplit)),
      h('div', { class: 'u-legend' }, legend.length ? legend : h('span', { class: 'u-note', text: 'Nothing in this range' })),
      plot);
  }

  function segmented(id, choices, current, pick) {
    return h('span', { class: `range u-seg ${id}`, role: 'radiogroup', data: { row: id } },
      choices.map(([value, label]) => h('button', { type: 'button', role: 'radio', 'aria-checked': String(value === current), onclick: () => pick(value) }, label)));
  }

  function slotName(d) {
    return d.slot_seconds >= 86400 ? 'a bar a day' : d.slot_seconds === 3600 ? 'a bar an hour' : `a bar every ${d.slot_seconds / 3600} hours`;
  }

  // seriesOf is what the chart stacks: the kinds, or the models with a
  // colour and the rest together.
  function seriesOf(d, agg, colourMap) {
    const metric = ui.metric;
    if (ui.split === 'kind') {
      if (metric === 'requests') {
        return [
          { id: 'ok', label: 'Succeeded', colour: 'ok', values: agg.slots.map((s) => s.requests - s.failed) },
          { id: 'failed', label: 'Failed', colour: 'bad', values: agg.slots.map((s) => s.failed) },
        ];
      }
      return KINDS.map(([id, label, title]) => ({
        id, label, title, colour: `k-${id}`,
        values: agg.slots.map((s) => (metric === 'cost' ? s.costs[id] : s.tokens[id] + (id === 'input' ? s.tokens.other : 0))),
      }));
    }
    const pick = (m) => (metric === 'cost' ? m.cost : metric === 'tokens' ? m.tokens : m.requests);
    const byModel = new Map();
    const other = { id: '*', label: 'Other models', colour: 'o', values: new Array(d.starts.length).fill(0) };
    agg.slots.forEach((slot, i) => {
      for (const [model, m] of slot.models) {
        const colour = colourMap.get(model);
        if (colour == null) {
          other.values[i] += pick(m);
          continue;
        }
        let s = byModel.get(model);
        if (!s) {
          s = { id: model, label: model || 'Unknown model', colour, values: new Array(d.starts.length).fill(0) };
          byModel.set(model, s);
        }
        s.values[i] += pick(m);
      }
    });
    const list = [...byModel.values()].sort((a, b) => Number(a.colour) - Number(b.colour));
    if (other.values.some((v) => v > 0)) list.push(other);
    return list;
  }

  // ticks label the axis: every six hours of a day, the midnights of a
  // week, every seventh day of a month back from today, and the 1st and
  // 15th of a quarter's months.
  function ticks(d) {
    const out = [];
    const n = d.starts.length;
    d.starts.forEach((iso, i) => {
      const t = new Date(iso);
      let label = '';
      if (d.range === '24h') {
        if (t.getHours() % 6 === 0) label = fmt.short(t);
      } else if (d.range === '7d') {
        if (t.getHours() === 0) label = t.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric' });
      } else if (d.range === '30d') {
        if ((n - 1 - i) % 7 === 0) label = t.toLocaleDateString(undefined, { day: 'numeric', month: 'short' });
      } else if (t.getDate() === 1 || t.getDate() === 15) {
        label = t.toLocaleDateString(undefined, { day: 'numeric', month: 'short' });
      }
      if (label) out.push({ i, label });
    });
    return out;
  }

  function slotTitle(d, i) {
    const start = new Date(d.starts[i]);
    const end = new Date(i + 1 < d.starts.length ? d.starts[i + 1] : d.to);
    const day = start.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' });
    if (d.slot_seconds >= 86400) return day;
    return `${day} · ${fmt.short(start)}–${fmt.short(end)}${end > Date.now() ? ' · now' : ''}`;
  }

  // ---------------------------------------------------------------- the chart's tip

  function hoverAt(event, d, agg, series) {
    const bar = event.target.closest?.('.u-bar');
    if (!bar) return;
    const i = Number(bar.dataset.slot);
    if (i !== ui.hover) {
      bar.parentNode.querySelector('.u-bar[data-hover]')?.removeAttribute('data-hover');
      bar.dataset.hover = 'true';
      ui.hover = i;
    }
    const slot = agg.slots[i];
    const rows = series.map((s) => [s, s.values[i]]).filter(([, v]) => v > 0).sort((a, b) => b[1] - a[1]);
    const shown = rows.slice(0, 8);
    const rest = rows.slice(8).reduce((sum, [, v]) => sum + v, 0);
    const content = [
      h('b', { class: 'u-tip-head', text: slotTitle(d, i) }),
      Date.parse(i + 1 < d.starts.length ? d.starts[i + 1] : d.to) <= (d.since ? Date.parse(d.since) : Infinity)
        ? h('span', { class: 'u-tip-note', text: 'Before the ledger began: not recorded' }) : null,
      shown.map(([s, v]) => h('span', { class: 'u-tip-row' }, h('i', { data: { s: s.colour } }), h('span', { class: 't', text: s.label }), h('b', { text: valueOf(ui.metric, v) }))),
      rest > 0 ? h('span', { class: 'u-tip-row' }, h('i', { data: { s: 'o' } }), h('span', { class: 't', text: `${rows.length - 8} more` }), h('b', { text: valueOf(ui.metric, rest) })) : null,
      h('span', { class: 'u-tip-foot', text: slot.requests
        ? `${count(slot.requests)} request${slot.requests === 1 ? '' : 's'}${slot.failed ? ` (${slot.failed} failed)` : ''} · ${fmtTokens(tokensOf(slot.tokens))} tokens · ${money(slot.cost, unit())}`
        : 'No requests' }),
    ];
    showTip(bar, content);
  }

  function showTip(anchor, content) {
    const scroller = $('usage-scroll');
    if (!scroller) return;
    if (tip.parentNode !== scroller) scroller.append(tip);
    tip.replaceChildren(...content.flat().filter(Boolean));
    tip.hidden = false;
    const box = scroller.getBoundingClientRect();
    const a = anchor.getBoundingClientRect();
    const plot = anchor.parentNode.getBoundingClientRect();
    const width = tip.offsetWidth;
    let left = a.right - box.left + 10;
    if (left + width > box.width - 12) left = a.left - box.left - 10 - width;
    tip.style.left = `${Math.max(8, left)}px`;
    tip.style.top = `${plot.top - box.top + scroller.scrollTop + 2}px`;
  }

  function hideTip() {
    tip.hidden = true;
    if (ui.hover >= 0) {
      $('usage-body')?.querySelector('.u-bar[data-hover]')?.removeAttribute('data-hover');
      ui.hover = -1;
    }
  }

  // ---------------------------------------------------------------- breakdowns

  // modelsCard lists the models of the range, the costliest first, each
  // with the price its tokens count at.
  function modelsCard(d, agg, colourMap) {
    const list = [...agg.models.values()].sort((a, b) => b.cost - a.cost || tokensOf(b.tokens) - tokensOf(a.tokens) || b.requests - a.requests);
    const most = Math.max(0, ...list.map((m) => m.cost)) || 0;
    const mostTokens = Math.max(0, ...list.map((m) => tokensOf(m.tokens))) || 1;
    const rows = list.map((m) => {
      const model = modelOf(m.id);
      const share = h('i');
      share.style.width = `${most ? (m.cost / most) * 100 : (tokensOf(m.tokens) / mostTokens) * 100}%`;
      const t = m.tokens;
      return h('tr', { data: { row: `m:${m.id}` }, 'aria-selected': ui.filter.model === m.id ? 'true' : null },
        h('td', { class: 'u-name' }, h('div', { class: 'u-name-box' },
          h('span', { class: 'u-sw', data: { s: colourMap.get(m.id) ?? 'o' } }),
          h('span', { class: 'u-name-col' },
            h('button', { type: 'button', class: 'u-link', title: ui.filter.model === m.id ? 'Show every model' : `Only ${m.id || 'this model'}`, onclick: () => setFilter('model', m.id), text: m.id || 'Unknown model' }),
            priceLine(model)))),
        h('td', { class: 'num', text: count(m.requests), title: m.failed ? `${m.failed} failed` : '' }),
        h('td', { class: 'num', text: fmtTokens(t.input + (t.other || 0)), title: 'Fresh input' }),
        h('td', { class: 'num', text: fmtTokens(t.cache_read + t.cache_write), title: `${fmtTokens(t.cache_read)} read · ${fmtTokens(t.cache_write)} written` }),
        h('td', { class: 'num', text: fmtTokens(t.output), title: t.reasoning ? `${fmtTokens(t.reasoning)} of it thinking` : '' }),
        h('td', { class: 'num u-cost' },
          h('b', { text: model.price?.source === 'none' ? '—' : money(m.cost, unit()), title: model.price?.source === 'none' ? 'No price: its tokens are left out of the cost' : '' }),
          h('span', { class: 'u-share' }, share)));
    });
    return h('section', { class: 'u-card u-models', data: { row: 'models' } },
      h('header', { class: 'u-head' },
        h('span', { class: 'label', text: 'By model' }),
        h('span', { class: 'u-note', text: `${list.length} model${list.length === 1 ? '' : 's'}` })),
      list.length ? h('table', { class: 'u-table' },
        h('thead', null, h('tr', null,
          h('th', { text: 'Model' }), h('th', { class: 'num', text: 'Req' }), h('th', { class: 'num', text: 'Fresh in' }),
          h('th', { class: 'num', text: 'Cache' }), h('th', { class: 'num', text: 'Out' }), h('th', { class: 'num', text: 'API cost' }))),
        h('tbody', null, rows)) : h('p', { class: 'u-note', text: 'No model in this range.' }));
  }

  // priceLine says what a model's tokens count at, and opens its price.
  function priceLine(model) {
    const p = model.price || { source: 'none' };
    const u = unit();
    let text;
    if (p.source === 'none') text = 'no price · set one';
    else if (p.source === 'estimate') text = `≈ ${rate(p.input, u)} in · ${rate(p.output, u)} out · as ${p.like}`;
    else text = `${rate(p.input, u)} in · ${rate(p.output, u)} out${p.source === 'custom' ? ' · yours' : p.source === 'openrouter' ? ' · OpenRouter' : ''}`;
    return h('button', {
      type: 'button', class: 'u-price', data: { source: p.source }, text,
      title: `${SOURCES[p.source]?.[1] || ''}${p.rule ? ` (${p.rule})` : ''}${p.note ? ` · ${p.note}` : ''} · per million tokens · click to set the price`,
      onclick: () => openPrice(model.id),
    });
  }

  function accountsCard(d, agg) {
    const list = [...agg.accounts.values()].sort((a, b) => b.cost - a.cost || tokensOf(b.tokens) - tokensOf(a.tokens) || b.requests - a.requests);
    const most = Math.max(0, ...list.map((a) => a.cost)) || 0;
    const mostTokens = Math.max(0, ...list.map((a) => tokensOf(a.tokens))) || 1;
    return h('section', { class: 'u-card u-accounts', data: { row: 'accounts' } },
      h('header', { class: 'u-head' },
        h('span', { class: 'label', text: 'By account' }),
        h('span', { class: 'u-note', text: 'what each subscription or key would have cost as API credits' })),
      list.length ? h('ol', { class: 'u-list' }, list.map((e) => {
        const a = accountOf(e.id);
        const share = h('i');
        share.style.width = `${most ? (e.cost / most) * 100 : (tokensOf(e.tokens) / mostTokens) * 100}%`;
        return h('li', { data: { row: `a:${e.id}` } },
          h('button', {
            type: 'button', class: 'u-acc', 'aria-pressed': String(ui.filter.account === e.id),
            title: ui.filter.account === e.id ? 'Show every account' : `Only ${a.label}`, onclick: () => setFilter('account', e.id),
          },
          a.provider_name ? h('span', { class: 'ptag', data: { provider: a.kind === 'endpoint' ? null : a.provider || null }, text: a.provider_name }) : null,
          h('span', { class: 'u-label', text: a.label }),
          a.gone ? h('span', { class: 'plan', text: 'removed' }) : null,
          h('b', { class: 'u-val', text: e.cost ? money(e.cost, unit()) : '—' })),
          h('span', { class: 'u-share' }, share),
          h('small', { text: `${count(e.requests)} request${e.requests === 1 ? '' : 's'}${e.failed ? ` · ${e.failed} failed` : ''} · ${fmtTokens(tokensOf(e.tokens))} tokens` }));
      })) : h('p', { class: 'u-note', text: 'No account in this range.' }));
  }

  // mixCard shows how tokens turn into credits: each kind's share of the
  // tokens beside its share of the cost, and what a million of it costs on
  // average over the models used.
  function mixCard(d) {
    const priced = new Set((d.models || []).filter((m) => m.price?.source !== 'none').map((m) => m.id));
    const tokens = zero();
    const costs = zeroCost();
    for (const r of d.rows || []) {
      if (!priced.has(r.m)) continue;
      addTo(tokens, r.t);
      addTo(costs, r.c);
    }
    tokens.input += tokens.other;
    const totalTokens = KINDS.reduce((sum, [id]) => sum + tokens[id], 0);
    const totalCost = KINDS.reduce((sum, [id]) => sum + costs[id], 0);
    const u = unit();
    const bar = (values, total, format) => h('div', { class: 'u-stack' }, KINDS.map(([id, label]) => {
      if (!(values[id] > 0) || !total) return null;
      const seg = h('i', { data: { s: `k-${id}` }, title: `${label}: ${format(values[id])} · ${pct(values[id], total)}` });
      seg.style.width = `${(values[id] / total) * 100}%`;
      return seg;
    }));
    const saved = d.totals.cache_saved || 0;
    return h('section', { class: 'u-card u-mix', data: { row: 'mix' } },
      h('header', { class: 'u-head' },
        h('span', { class: 'label', text: 'Tokens → credits' }),
        h('span', { class: 'u-note', text: `in ${unitName(u)}, over the models with a price` })),
      totalTokens ? [
        h('div', { class: 'u-mixrow' }, h('span', { class: 'k', text: 'Tokens' }), bar(tokens, totalTokens, (v) => fmtTokens(v))),
        h('div', { class: 'u-mixrow' }, h('span', { class: 'k', text: 'Cost' }), bar(costs, totalCost, (v) => money(v, u))),
        h('table', { class: 'u-table u-kinds' },
          h('thead', null, h('tr', null, h('th', { text: 'Kind' }), h('th', { class: 'num', text: 'Tokens' }), h('th', { class: 'num', text: 'Per 1M' }),
            h('th', { class: 'num', text: 'Cost' }), h('th', { class: 'num', text: 'Share' }))),
          h('tbody', null,
            KINDS.map(([id, label, title]) => h('tr', { title },
              h('td', null, h('span', { class: 'u-kind' }, h('span', { class: 'u-sw', data: { s: `k-${id}` } }), label)),
              h('td', { class: 'num', text: fmtTokens(tokens[id]) }),
              h('td', { class: 'num', text: tokens[id] ? rate((costs[id] / tokens[id]) * 1e6, u) : '—' }),
              h('td', { class: 'num', text: money(costs[id], u) }),
              h('td', { class: 'num', text: pct(costs[id], totalCost) }))),
            h('tr', { class: 'u-total' },
              h('td', { text: 'All' }),
              h('td', { class: 'num', text: fmtTokens(totalTokens) }),
              h('td', { class: 'num', text: totalTokens ? rate((totalCost / totalTokens) * 1e6, u) : '—' }),
              h('td', { class: 'num', text: money(totalCost, u) }),
              h('td', { class: 'num', text: totalCost ? '100%' : '—' })))),
        h('p', { class: 'u-note', text: saved
          ? `Caching ${saved > 0 ? 'saved' : 'cost'} ${money(Math.abs(saved), u)}: cache reads cost a fraction of the input${costs.cache_write ? ', less what writing the caches cost beyond it' : ''}.`
          : 'Prices are the providers’ list prices unless you set your own below.' }),
      ] : h('p', { class: 'u-note', text: priced.size || !(d.models || []).length ? 'No tokens in this range.' : 'None of these models has a price yet: set one below.' }));
  }

  // heatCard shows the week's hours, darker where the range spent more.
  function heatCard(d) {
    const metric = ui.metric;
    const cells = d.heat || [];
    const value = (c) => (metric === 'cost' ? c.c : metric === 'tokens' ? c.t : c.n);
    const max = Math.max(0, ...cells.map(value));
    const grid = [];
    grid.push(h('span', { class: 'u-hl' }));
    for (let hour = 0; hour < 24; hour++) grid.push(h('span', { class: 'u-hh', text: hour % 6 === 0 ? String(hour).padStart(2, '0') : '' }));
    WEEKDAYS.forEach((day, row) => {
      grid.push(h('span', { class: 'u-hl', text: day }));
      for (let hour = 0; hour < 24; hour++) {
        const c = cells[row * 24 + hour] || { n: 0, t: 0, c: 0 };
        const v = value(c);
        const level = v > 0 && max > 0 ? Math.max(1, Math.ceil(Math.sqrt(v / max) * 4)) : 0;
        grid.push(h('i', {
          data: { level: String(level) },
          title: `${day} ${String(hour).padStart(2, '0')}:00–${String((hour + 1) % 24).padStart(2, '0')}:00 · ${c.n ? `${count(c.n)} request${c.n === 1 ? '' : 's'} · ${fmtTokens(c.t)} tokens · ${money(c.c, unit())}` : 'nothing'}`,
        }));
      }
    });
    return h('section', { class: 'u-card u-heat', data: { row: 'heat' } },
      h('header', { class: 'u-head' },
        h('span', { class: 'label', text: 'By hour of the week' }),
        h('span', { class: 'u-note', text: `${METRICS.find(([m]) => m === metric)[1].toLowerCase()} · ${d.zone || 'UTC'}` })),
      h('div', { class: 'u-heat-grid' }, grid),
      h('div', { class: 'u-heat-scale' }, h('span', { text: 'less' }), [0, 1, 2, 3, 4].map((level) => h('i', { data: { level: String(level) } })), h('span', { text: max ? `more · up to ${valueOf(metric, max)} an hour` : 'more' })));
  }

  // ---------------------------------------------------------------- prices

  function pricesCard(d) {
    const listing = ui.prices;
    const u = unit();
    const header = h('header', { class: 'u-head' },
      h('span', { class: 'label', text: 'Prices · per 1M tokens', title: 'What a million tokens of each kind costs as API credits' }),
      h('span', { class: 'u-flex' }),
      openRouterNote(listing?.openrouter),
      listing?.openrouter?.enabled ? h('button', {
        class: 'act', type: 'button', disabled: ui.fetching, title: 'Fetch OpenRouter’s prices now', onclick: refreshMarket,
      }, ui.fetching ? 'Fetching…' : 'Update') : null,
      h('button', { class: 'act', type: 'button', title: 'Show amounts in dollars or in credits of your own', onclick: () => openUnit() }, `Amounts in ${unitName(u)}`),
      h('button', { class: 'act strong', type: 'button', title: 'Set a price for a model, or a pattern of models (P)', onclick: () => openPrice(ui.filter.model || '') }, '+ Price'));
    if (!listing) {
      return h('section', { class: 'u-card u-prices', id: 'usage-prices', data: { row: 'prices' } }, header,
        ui.pricesError ? h('p', { class: 'settings-status', data: { kind: 'error' }, text: ui.pricesError }) : loadingLine('Reading the prices'));
    }
    const inRange = new Map((d.facets?.models || []).map((f, i) => [f.id, i]));
    const models = listing.models.filter((m) => !m.media || m.used);
    const rank = (m) => (inRange.has(m.id) ? inRange.get(m.id) : m.used ? 1000 : 2000);
    models.sort((a, b) => rank(a) - rank(b) || a.id.localeCompare(b.id));
    const main = models.filter((m) => inRange.has(m.id) || m.used);
    const rest = models.filter((m) => !inRange.has(m.id) && !m.used);
    const shown = ui.moreModels || !main.length ? models : main;
    const cell = (v) => h('td', { class: 'num', text: rate(v, u) });
    const rows = shown.map((m) => {
      const p = m.price;
      const [badge, why] = SOURCES[p.source] || [p.source, ''];
      return h('tr', { data: { row: `p:${m.id}` }, class: inRange.has(m.id) ? 'u-used' : null },
        h('td', { class: 'u-name' }, h('div', { class: 'u-name-col' },
          h('span', { class: 'u-model', text: m.id, title: m.name || m.id }),
          m.name && m.name !== m.id ? h('small', { text: m.name }) : null)),
        ...(p.source === 'none' ? [h('td', { class: 'num u-none', colspan: '4', text: 'no price: its tokens are left out' })]
          : [cell(p.input), cell(p.cache_read), cell(p.cache_write), cell(p.output)]),
        h('td', null, h('span', {
          class: 'u-src', data: { source: p.source },
          title: `${why}${p.like ? `: ${p.like}` : ''}${p.rule ? ` · by ${p.rule}` : ''}${p.note ? ` · ${p.note}` : ''}`, text: badge,
        })),
        h('td', { class: 'u-edit' }, h('button', { class: 'act', type: 'button', title: `Set the price of ${m.id}`, onclick: () => openPrice(m.id) }, p.source === 'none' ? 'Set' : 'Edit')));
    });
    const rules = listing.rules || [];
    return h('section', { class: 'u-card u-prices', id: 'usage-prices', data: { row: 'prices' } }, header,
      listing.error ? h('p', { class: 'settings-status', data: { kind: 'error' }, text: `${listing.error}; saving a price replaces the file` }) : null,
      shown.length ? h('table', { class: 'u-table u-price-table' },
        h('thead', null, h('tr', null, h('th', { text: 'Model' }), h('th', { class: 'num', text: 'Input' }), h('th', { class: 'num', text: 'Cache read' }),
          h('th', { class: 'num', text: 'Cache write' }), h('th', { class: 'num', text: 'Output' }), h('th', { text: 'Source' }), h('th'))),
        h('tbody', null, rows)) : h('p', { class: 'u-note', text: 'The gateway serves no models yet.' }),
      main.length && rest.length ? h('button', { class: 'lim-more u-more', type: 'button', onclick: () => { ui.moreModels = !ui.moreModels; render(); } },
        ui.moreModels ? 'Only the models used' : `+ ${rest.length} more model${rest.length === 1 ? '' : 's'} the gateway serves`) : null,
      rules.length ? h('div', { class: 'u-rules' },
        h('span', { class: 'label', text: 'Your prices' }),
        rules.map((r) => h('span', { class: 'chip u-rule', title: r.note || '' },
          h('button', { type: 'button', class: 'u-link', title: 'Edit', onclick: () => openPrice(r.match.includes('*') ? '' : r.match, r) }, r.match),
          ` ${rate(r.input, u)} / ${rate(r.output, u)}`,
          h('button', { type: 'button', class: 'u-x', title: 'Remove: back to the price the cockpit knows', 'aria-label': `Remove the price of ${r.match}`, onclick: () => removePrice(r.match) }, '×')))) : null,
      h('p', { class: 'u-note u-fine', text: 'OpenRouter’s prices, fetched twice a day, come before the list prices the cockpit knows, for prompts of ordinary length: long-context, batch and priority tiers aside. ≈ marks a newer model priced as the latest of its family the cockpit knows. Claude’s cache writes count at the one-hour rate the gateway asks for; tokens of no known kind count as input.' }));
  }

  // openRouterNote says when OpenRouter's prices were fetched.
  function openRouterNote(o) {
    if (!o) return null;
    if (!o.enabled) return h('span', { class: 'u-note', text: 'OpenRouter: off', title: 'KOU_CONVEYOR_OPENROUTER_PRICES=off' });
    const when = o.fetched ? `${fmt.ago(o.fetched) === 'now' ? 'just now' : `${fmt.ago(o.fetched)} ago`}` : 'not fetched yet';
    return h('span', {
      class: 'u-note u-market', data: { kind: o.error ? 'error' : null },
      title: o.error ? `The latest fetch failed: ${o.error}` : `OpenRouter lists ${o.models} priced models`,
      text: `OpenRouter · ${o.models} models · ${when}${o.error ? ' · failed' : ''}`,
    });
  }

  async function refreshMarket() {
    ui.fetching = true;
    render();
    try {
      ui.prices = await api('/api/usage/prices/refresh', { method: 'POST', body: {} });
      toast(`OpenRouter prices updated · ${ui.prices.openrouter.models} models`);
      refresh({ quiet: true });
      ctx.onPrices?.();
    } catch (error) {
      toast(error.message, 'error');
      loadPrices();
    }
    ui.fetching = false;
    render();
  }

  // ---------------------------------------------------------------- the price dialog

  const num = (v) => (String(v).trim() === '' ? null : Number(String(v).replace(',', '.')));

  // openPrice opens the dialog for a model's price, or a rule of the user's.
  function openPrice(model = '', rule = null) {
    const listing = ui.prices;
    const match = (rule?.match || model || '').toLowerCase();
    const existing = rule || listing?.rules?.find((r) => r.match === match) || null;
    const current = model ? listing?.models?.find((m) => m.id === model)?.price || modelOf(model).price : null;
    const base = existing || (current?.source && current.source !== 'none' ? current : null);
    const show = (v) => (v == null ? '' : String(v));
    ui.dialog = {
      mode: 'price', model, match: existing?.match || model,
      input: show(base?.input), output: show(base?.output), cache_read: show(base?.cache_read), cache_write: show(base?.cache_write),
      note: existing?.note || '', from: base ? (existing ? 'yours' : current.source) : '', busy: false, error: '',
    };
    ctx.openDialog();
    renderDialog();
    if (!listing) loadPrices();
    setTimeout(() => $('price-dialog')?.querySelector(model || existing ? '#price-input' : '#price-match')?.focus(), 0);
  }

  function openUnit() {
    const u = unit();
    ui.dialog = { mode: 'unit', kind: isDollars(u) ? 'usd' : 'credits', name: isDollars(u) ? 'credits' : u.name, usd: isDollars(u) ? '0.01' : String(u.usd), busy: false, error: '' };
    ctx.openDialog();
    renderDialog();
  }

  function closeDialog() {
    if (!ui.dialog) return false;
    ui.dialog = null;
    ctx.closeDialog();
    return true;
  }

  // matching lists the models a pattern prices: of those the gateway
  // serves or requests used.
  function matching(pattern) {
    const p = String(pattern || '').trim().toLowerCase();
    if (!p) return [];
    return (ui.prices?.models || []).filter((m) => glob(p, m.id)).map((m) => m.id);
  }

  function renderDialog() {
    const d = ui.dialog;
    const title = $('price-title');
    const body = $('price-body');
    const foot = $('price-foot');
    if (!d || !body) return;
    if (d.mode === 'unit') return renderUnit(d, title, body, foot);
    title.textContent = d.model ? `Price of ${d.model}` : 'A price';
    const field = (id, label, key, placeholder) => h('label', { class: 'field' },
      h('span', { class: 'label', text: label }),
      h('input', {
        id: `price-${id}`, type: 'text', inputmode: 'decimal', spellcheck: 'false', autocomplete: 'off', value: d[key], placeholder,
        oninput: (event) => { d[key] = event.target.value; d.error = ''; renderStatus(); },
      }));
    const suggestions = d.model ? families(d.model) : [];
    const builtin = (ui.prices?.builtin || []).filter((b) => b.source === 'list');
    const stem = (d.model || d.match || '').split(/[-/]/)[0].toLowerCase();
    const near = builtin.filter((b) => stem && b.match.some((m) => m.startsWith(stem)));
    const far = builtin.filter((b) => !near.includes(b));
    const borrow = (b) => h('button', {
      type: 'button', class: 'chip u-borrow', title: `${b.match.join(', ')}${b.note ? ` · ${b.note}` : ''}`,
      onclick: () => {
        Object.assign(d, { input: String(b.input), output: String(b.output), cache_read: String(b.cache_read), cache_write: String(b.cache_write), error: '' });
        renderDialog();
      },
    }, h('b', { text: b.name || b.match[0].replace(/\*$/, '') }), ` ${rate(b.input)} / ${rate(b.output)}`);
    body.replaceChildren(
      h('label', { class: 'field' },
        h('span', { class: 'label', text: 'Model, or a pattern with *' }),
        h('input', {
          id: 'price-match', type: 'text', spellcheck: 'false', autocomplete: 'off', value: d.match, placeholder: 'gpt-6-*',
          oninput: (event) => { d.match = event.target.value.trim(); d.error = ''; renderStatus(); },
        })),
      suggestions.length > 1 ? h('div', { class: 'chips u-suggest' }, suggestions.map((s) => h('button', {
        type: 'button', class: 'chip', 'aria-pressed': String(s === d.match), title: `Price ${matching(s).length || 'no'} model${matching(s).length === 1 ? '' : 's'} of the gateway's with it`,
        onclick: () => { d.match = s; renderDialog(); },
      }, s))) : null,
      h('p', { class: 'u-hint', id: 'price-hint' }),
      h('div', { class: 'u-price-grid' },
        field('input', 'Input · $ per 1M', 'input', '0'),
        field('output', 'Output · $ per 1M', 'output', '0'),
        field('cache-read', 'Cache read · $ per 1M', 'cache_read', 'as the input'),
        field('cache-write', 'Cache write · $ per 1M', 'cache_write', 'as the input')),
      builtin.length ? h('div', { class: 'field' },
        h('span', { class: 'label', text: 'Borrow a list price' }),
        h('div', { class: 'chips u-borrows' }, near.map(borrow), far.map(borrow))) : null,
      h('label', { class: 'field' },
        h('span', { class: 'label', text: 'Note' }),
        h('input', { id: 'price-note', type: 'text', value: d.note, placeholder: 'Where the price comes from', maxlength: '200', oninput: (event) => { d.note = event.target.value; } })),
      h('p', { class: 'settings-status', id: 'price-status', role: 'status' }));
    foot.replaceChildren(
      h('span', { class: 'settings-path', id: 'price-foot-note' }),
      h('button', { class: 'act', type: 'button', id: 'price-remove', onclick: () => removePrice(d.match, true) }, 'Remove mine'),
      h('button', { class: 'act', type: 'button', onclick: closeDialog }, 'Cancel'),
      h('button', { class: 'primary', type: 'button', id: 'price-save', onclick: savePrice }, d.busy ? 'Saving…' : 'Save'));
    renderStatus();
  }

  // renderStatus updates what the dialog says as its fields change.
  function renderStatus() {
    const d = ui.dialog;
    if (!d || d.mode !== 'price') return;
    const hint = $('price-hint');
    const models = matching(d.match);
    const mine = ui.prices?.rules?.find((r) => r.match === d.match.toLowerCase());
    if (hint) {
      hint.textContent = !d.match ? 'Name a model, or a pattern such as claude-opus-5* for a family.'
        : models.length ? `Prices ${models.length === 1 ? models[0] : `${models.length} models: ${models.slice(0, 4).join(', ')}${models.length > 4 ? '…' : ''}`}${d.match.includes('*') ? ', and those to come' : ''}.`
          : `No model of the gateway's matches it yet${d.match.includes('*') ? '; those that do will take the price' : ''}.`;
    }
    const remove = $('price-remove');
    if (remove) remove.hidden = !mine;
    const note = $('price-foot-note');
    if (note) note.textContent = mine ? 'Replaces your price of it' : d.from === 'list' ? 'Overrides the list price' : d.from === 'estimate' ? 'Replaces the estimate' : '';
    const status = $('price-status');
    if (status) {
      status.textContent = d.error;
      status.dataset.kind = d.error ? 'error' : '';
    }
    const save = $('price-save');
    if (save) save.disabled = d.busy || !d.match;
  }

  async function savePrice() {
    const d = ui.dialog;
    if (!d || d.busy) return;
    const input = num(d.input) ?? 0;
    const output = num(d.output) ?? 0;
    const read = num(d.cache_read) ?? input;
    const write = num(d.cache_write) ?? input;
    if (![input, output, read, write].every((v) => Number.isFinite(v) && v >= 0)) {
      d.error = 'Prices are dollars per million tokens: numbers, 0 or more.';
      renderStatus();
      return;
    }
    d.busy = true;
    renderDialog();
    try {
      ui.prices = await api('/api/usage/prices', { method: 'POST', body: { match: d.match, input, output, cache_read: read, cache_write: write, note: d.note.trim() } });
      const models = matching(d.match);
      closeDialog();
      toast(`Price saved · ${d.match}${models.length > 1 ? ` · ${models.length} models` : ''}`);
      refresh({ quiet: true });
      ctx.onPrices?.();
    } catch (error) {
      if (ui.dialog !== d) return;
      d.busy = false;
      d.error = error.message;
      renderDialog();
    }
  }

  async function removePrice(match, fromDialog = false) {
    try {
      ui.prices = await api(`/api/usage/prices?${new URLSearchParams({ match })}`, { method: 'DELETE' });
      if (fromDialog) closeDialog();
      toast(`Your price of ${match} removed`);
      refresh({ quiet: true });
      ctx.onPrices?.();
    } catch (error) {
      toast(error.message, 'error');
    }
    render();
  }

  function renderUnit(d, title, body, foot) {
    title.textContent = 'Amounts in';
    const preview = () => {
      const worth = num(d.usd);
      return d.kind === 'usd' ? 'Amounts show in US dollars, as API prices are.'
        : worth > 0 ? `$10 reads as ${money(10, { name: d.name.trim() || 'credits', usd: worth })}; ${money(worth, DOLLARS)} is one.` : 'Say what one credit is worth in dollars.';
    };
    const choice = (value, label, small) => h('label', null,
      h('input', { type: 'radio', name: 'unit-kind', value, checked: d.kind === value, onchange: () => { d.kind = value; d.error = ''; renderDialog(); } }),
      h('span', null, h('b', { text: label }), h('small', { text: small })));
    body.replaceChildren(
      h('fieldset', { class: 'seg two' }, h('legend', { class: 'label', text: 'Show amounts as' }),
        choice('usd', 'US dollars', 'as the providers price them'),
        choice('credits', 'Credits', 'of your own, worth a set amount')),
      d.kind === 'credits' ? h('div', { class: 'u-price-grid' },
        h('label', { class: 'field' }, h('span', { class: 'label', text: 'Name' }),
          h('input', { id: 'unit-name', type: 'text', value: d.name, maxlength: '24', oninput: (event) => { d.name = event.target.value; setText($('unit-preview'), preview()); } })),
        h('label', { class: 'field' }, h('span', { class: 'label', text: 'One is worth, in $' }),
          h('input', { id: 'unit-usd', type: 'text', inputmode: 'decimal', value: d.usd, oninput: (event) => { d.usd = event.target.value; setText($('unit-preview'), preview()); } }))) : null,
      h('p', { class: 'u-hint', id: 'unit-preview', text: preview() }),
      h('p', { class: 'settings-status', data: { kind: d.error ? 'error' : '' }, text: d.error }));
    foot.replaceChildren(
      h('span', { class: 'settings-path', text: 'Kept with the prices, for every page' }),
      h('button', { class: 'act', type: 'button', onclick: closeDialog }, 'Cancel'),
      h('button', { class: 'primary', type: 'button', disabled: d.busy, onclick: saveUnit }, d.busy ? 'Saving…' : 'Save'));
  }

  async function saveUnit() {
    const d = ui.dialog;
    if (!d || d.busy) return;
    const body = d.kind === 'usd' ? { name: 'USD', usd: 1 } : { name: d.name.trim(), usd: num(d.usd) };
    d.busy = true;
    renderDialog();
    try {
      ui.prices = await api('/api/usage/unit', { method: 'PUT', body });
      closeDialog();
      toast(`Amounts in ${unitName(ui.prices.unit)}`);
      if (ui.data) ui.data = { ...ui.data, unit: ui.prices.unit };
      render();
      refresh({ quiet: true });
      ctx.onPrices?.();
    } catch (error) {
      if (ui.dialog !== d) return;
      d.busy = false;
      d.error = error.message;
      renderDialog();
    }
  }

  // ---------------------------------------------------------------- keys

  // key handles a key pressed on the tab; it reports whether it did.
  function key(event) {
    if (!ui.open) return false;
    const range = RANGE_KEYS[event.key];
    let action = null;
    if (range) action = () => setRange(range);
    else if (event.key === 'r') action = () => { refresh(); loadPrices(); };
    else if (event.key === 'p') action = () => openPrice(ui.filter.model || '');
    if (!action) return false;
    event.preventDefault();
    action();
    return true;
  }

  function destroy() {
    ui.open = false;
    clearTimeout(ui.poll);
    tip.remove();
  }

  return {
    open, close, destroy, key, setRange, openPrice, openUnit, closeDialog,
    refresh: (options) => { refresh(options); loadPrices(); },
    visibility: () => { if (ui.open && document.visibilityState === 'visible') refresh({ quiet: true }).finally(schedule); },
    get range() { return ui.range; },
    get dialogOpen() { return !!ui.dialog; },
  };
}

// niceMax rounds the top of a chart up to a value its quarters read well
// at; counts of requests stay whole.
function niceMax(v, whole) {
  if (!(v > 0)) return whole ? 4 : 1;
  if (whole && v <= 20) return Math.max(4, Math.ceil(v / 4) * 4);
  const exp = 10 ** Math.floor(Math.log10(v));
  for (const m of [1, 1.2, 1.6, 2, 2.4, 3, 4, 6, 8, 10]) if (m * exp >= v) return m * exp;
  return 10 * exp;
}

function latency(ms) {
  return ms < 1000 ? `${Math.round(ms)}ms` : fmt.duration(ms);
}

function setText(el, text) {
  if (el && el.textContent !== text) el.textContent = text;
}
