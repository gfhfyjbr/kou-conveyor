// The kernel: the plugin host the page is. It builds nothing of its own —
// the sessions, the transcript, the composer, the inspector, the accounts,
// even the layout they sit in are plugins, the cockpit's built-in ones on
// the same API as a user's or a workspace's. The kernel loads the plugins
// the server lists for the workspace in view, gives each the cockpit API
// (see docs/plugins.md), and keeps everything a plugin contributes under
// its name: its UI, commands, keys, services, listeners, timers and styles.
// So a plugin can be taken away whole, or swapped for a newer version while
// the page runs: the server streams the listing again whenever a plugin's
// files change, and the kernel loads the changed plugins anew — the new
// version in first, then the old one taken away — and nothing else.

import { fmt, h, kv, svg, typingIn, uuid } from './dom.js';

export const API_VERSION = 2;
const PREFIX = 'kou-conveyor.';

// ---------------------------------------------------------------- storage

// prefs keeps small preferences in localStorage, as JSON.
export const prefs = Object.freeze({
  get(key, fallback) {
    try {
      const raw = localStorage.getItem(PREFIX + key);
      return raw == null ? fallback : JSON.parse(raw);
    } catch { return fallback; }
  },
  set(key, value) {
    try {
      if (value == null) localStorage.removeItem(PREFIX + key);
      else localStorage.setItem(PREFIX + key, JSON.stringify(value));
    } catch { /* storage unavailable: preferences are best effort */ }
  },
});

// ---------------------------------------------------------------- server

export class ApiError extends Error {
  constructor(status, body) {
    super(body?.error || (status ? `Request failed (${status})` : 'Server unreachable'));
    this.status = status;
    this.body = body;
  }
}

const net = { online: true };

// api calls the cockpit's HTTP API and returns its JSON answer.
export async function api(path, { method = 'GET', body, signal, headers } = {}) {
  const init = { method, signal, headers: { Accept: 'application/json', ...headers } };
  if (body !== undefined) {
    init.headers['Content-Type'] = 'application/json';
    init.body = JSON.stringify(body);
  }
  let res;
  try {
    res = await fetch(path, init);
  } catch (error) {
    if (error.name === 'AbortError') throw error;
    setOnline(false);
    throw new ApiError(0, null);
  }
  setOnline(true);
  const data = (res.headers.get('Content-Type') || '').includes('application/json') ? await res.json().catch(() => null) : null;
  if (!res.ok) throw new ApiError(res.status, data);
  return data;
}

function setOnline(online) {
  if (net.online === online) return;
  net.online = online;
  emit('online', online);
}

export function wsPath(ws, path) {
  return ws ? `/api/w/${encodeURIComponent(ws)}${path}` : `/api${path}`;
}

// ---------------------------------------------------------------- failures

// A plugin's code that throws is reported and the page goes on: every call
// into a plugin goes through safely.
const failures = new Map(); // plugin → { at, message, count }
let reporting = false;

function failed(plugin, error, where = '') {
  console.error(`plugin ${plugin}${where ? ` (${where})` : ''}:`, error);
  const message = String(error?.message || error);
  const last = failures.get(plugin);
  failures.set(plugin, { at: Date.now(), message, count: (last?.count || 0) + 1, where });
  if (reporting) return;
  reporting = true;
  try {
    emit('plugin-error', { plugin, error, where, text: `plugin ${plugin}${where ? ` (${where})` : ''}: ${error?.stack || error}` });
    if (!last || Date.now() - last.at > 10_000) toast(`Plugin ${plugin} failed${where ? ` to ${where}` : ''}: ${message}`, 'error', `plugin-${plugin}`);
  } finally {
    reporting = false;
  }
}

function safely(plugin, fn, ...args) {
  try {
    return fn(...args);
  } catch (error) {
    failed(plugin, error);
    return undefined;
  }
}

// toast shows a message through the toast service a plugin provides, or, if
// none does, in the kernel's own plain way.
function toast(text, kind = 'info', key = '') {
  const service = current('toast');
  if (service?.show) {
    try {
      return service.show(String(text), kind, key);
    } catch (error) {
      console.error(error);
    }
  }
  const box = document.getElementById('kernel-toasts') || document.body.appendChild(h('div', { id: 'kernel-toasts', class: 'kernel-toasts' }));
  if (key) box.querySelector(`[data-key="${CSS.escape(key)}"]`)?.remove();
  const el = h('div', { class: `kernel-toast ${kind}`, data: { key: key || null }, text: String(text) });
  box.append(el);
  setTimeout(() => el.remove(), kind === 'error' ? 8000 : 3500);
  return undefined;
}

// ---------------------------------------------------------------- scopes

// A scope holds what one loaded plugin added, to take it all away again.
class Scope {
  constructor(plugin) {
    this.plugin = plugin;
    this.items = new Set();
    this.closed = false;
  }

  // add registers fn to run when the scope closes, and returns a function
  // that runs it now, once.
  add(fn) {
    if (this.closed) {
      safely(this.plugin, fn);
      return () => {};
    }
    let done = false;
    const entry = () => {
      if (done) return;
      done = true;
      this.items.delete(entry);
      safely(this.plugin, fn);
    };
    this.items.add(entry);
    return entry;
  }

  close() {
    this.closed = true;
    for (const entry of [...this.items].reverse()) entry();
  }
}

// ---------------------------------------------------------------- events

const listeners = new Map(); // event → [{ fn, plugin }]

function on(scope, event, fn) {
  const record = { fn, plugin: scope.plugin };
  if (!listeners.has(event)) listeners.set(event, []);
  listeners.get(event).push(record);
  return scope.add(() => {
    const list = listeners.get(event);
    const at = list?.indexOf(record) ?? -1;
    if (at >= 0) list.splice(at, 1);
  });
}

function listening(event) {
  return !!listeners.get(event)?.length;
}

export function emit(event, ...args) {
  const list = listeners.get(event);
  if (!list?.length) return;
  for (const record of [...list]) safely(record.plugin, record.fn, ...args);
}

// ---------------------------------------------------------------- contributions

// A contribution point is a named list any plugin adds to and any plugin
// reads: the slash commands are the point "commands", the command palette's
// items "palette", what fills a slot of the layout "slot:<name>". Points
// need no declaring. Readers are told of changes by the event
// "point:<name>", once per task however many changes it made.
let sequence = 0;
const points = new Map(); // point → [{ item, plugin, seq }]
const dirtyPoints = new Set();
let pointsQueued = false;

function contribute(scope, point, item) {
  if (!item || typeof item !== 'object') throw new TypeError(`a contribution to ${point} must be an object`);
  if (item.plugin === undefined) {
    try {
      Object.defineProperty(item, 'plugin', { value: scope.plugin, enumerable: true, configurable: true, writable: true });
    } catch { /* a frozen item keeps no owner */ }
  }
  const record = { item, plugin: scope.plugin, seq: ++sequence };
  if (!points.has(point)) points.set(point, []);
  points.get(point).push(record);
  touchPoint(point);
  return scope.add(() => {
    const list = points.get(point);
    const at = list?.indexOf(record) ?? -1;
    if (at >= 0) list.splice(at, 1);
    touchPoint(point);
  });
}

// rank orders what plugins of different sources contribute: a workspace's
// over a user's over a built-in one, whichever was loaded last — so a
// plugin that replaces a built-in's item keeps it replaced when the
// built-in plugin is loaded anew.
const RANK = { builtin: 0, user: 1, workspace: 2 };
function rank(plugin) {
  return RANK[host.loaded.get(plugin)?.meta.source] ?? 0;
}
const later = (a, b) => rank(a.plugin) - rank(b.plugin) || a.seq - b.seq;

// contributions lists a point's items by their order, then as they came.
// With unique, of the items that share that key the one of the latest
// source, else the latest, counts, in the place of the first: a plugin
// replaces another's item by giving its own the same id.
function contributions(point, { unique = '' } = {}) {
  const records = [...(points.get(point) || [])]
    .sort((a, b) => (a.item.order ?? 0) - (b.item.order ?? 0) || later(a, b));
  if (!unique) return records.map((record) => record.item);
  const out = [];
  const at = new Map();
  for (const record of records) {
    const key = record.item[unique];
    if (key == null) {
      out.push(record);
      continue;
    }
    if (!at.has(key)) {
      at.set(key, out.length);
      out.push(record);
    } else if (later(record, out[at.get(key)]) > 0) {
      out[at.get(key)] = record;
    }
  }
  return out.map((record) => record.item);
}

function touchPoint(point) {
  dirtyPoints.add(point);
  if (pointsQueued) return;
  pointsQueued = true;
  queueMicrotask(flushPoints);
}

function flushPoints() {
  pointsQueued = false;
  const list = [...dirtyPoints];
  dirtyPoints.clear();
  for (const point of list) {
    if (point.startsWith('slot:')) mountSlot(point.slice(5));
    emit(`point:${point}`);
  }
  emit('points', list);
}

// ---------------------------------------------------------------- services

// A service is an object a plugin provides under a name for others to use:
// the session, the composer, the models. use(name) returns a stand-in that
// always reaches the latest provider, so a plugin that holds it keeps
// working when the provider is loaded again, or replaced by another
// plugin's service of the same name.
const services = new Map(); // name → [{ impl, plugin, seq }]
const proxies = new Map();

function provide(scope, name, impl) {
  if (!impl || (typeof impl !== 'object' && typeof impl !== 'function')) throw new TypeError(`service ${name} must be an object`);
  const record = { impl, plugin: scope.plugin, seq: ++sequence };
  if (!services.has(name)) services.set(name, []);
  services.get(name).push(record);
  queueMicrotask(() => emit('service', name));
  return scope.add(() => {
    const list = services.get(name);
    const at = list?.indexOf(record) ?? -1;
    if (at >= 0) list.splice(at, 1);
    queueMicrotask(() => emit('service', name));
  });
}

function current(name) {
  const list = services.get(name);
  if (!list?.length) return undefined;
  return list.reduce((best, record) => (later(record, best) > 0 ? record : best)).impl;
}

function use(name) {
  if (proxies.has(name)) return proxies.get(name);
  const proxy = new Proxy(Object.create(null), {
    get(_, key) {
      if (key === 'then') return undefined; // not a promise
      if (key === Symbol.toStringTag) return `Service ${name}`;
      const impl = current(name);
      if (!impl) return undefined;
      const value = impl[key];
      return typeof value === 'function' ? value.bind(impl) : value;
    },
    set(_, key, value) {
      const impl = current(name);
      if (impl) impl[key] = value;
      return true;
    },
    has(_, key) {
      const impl = current(name);
      return !!impl && key in impl;
    },
  });
  proxies.set(name, proxy);
  return proxy;
}

// ---------------------------------------------------------------- hooks

// A hook lets plugins take part in what another does: the composer asks
// "composer.submit" whether a plugin takes what was typed (a command, a
// message for the queue); the session passes a run's request through
// "run.request" before sending it.
function tap(scope, name, fn, { order = 0 } = {}) {
  return contribute(scope, `hook:${name}`, { fn, order });
}

// runHooks passes value through every hook in order; a hook returns the
// value to go on with, or nothing to leave it.
function runHooks(name, value, ...args) {
  for (const hook of contributions(`hook:${name}`)) {
    const result = safely(hook.plugin, hook.fn, value, ...args);
    if (result !== undefined) value = result;
  }
  return value;
}

async function runHooksAsync(name, value, ...args) {
  for (const hook of contributions(`hook:${name}`)) {
    try {
      const result = await hook.fn(value, ...args);
      if (result !== undefined) value = result;
    } catch (error) {
      failed(hook.plugin, error, name);
    }
  }
  return value;
}

// firstHook returns what the first hook to answer says; the others are not
// asked.
function firstHook(name, ...args) {
  for (const hook of contributions(`hook:${name}`)) {
    const result = safely(hook.plugin, hook.fn, ...args);
    if (result !== undefined && result !== null && result !== false) return result;
  }
  return undefined;
}

// ---------------------------------------------------------------- store

// The store holds small shared state, such as the view shown; "store:<key>"
// fires when a key changes.
const store = new Map();

function storeSet(key, value) {
  if (Object.is(store.get(key), value)) return;
  store.set(key, value);
  emit(`store:${key}`, value);
}

// ---------------------------------------------------------------- slots

// A slot is a named place in the page a plugin offers, backed by an element
// of its own: the layout offers "bar.end", the composer "composer.row".
// What plugins mount into a slot goes in by order; an item mounted with
// the id of another's takes its place (and gives it back when it goes). An
// item mounted before its slot exists waits for it, and a slot whose
// element is replaced — its plugin loaded again — takes its items along.
const slots = new Map(); // name → [{ el, plugin, seq }]
const placed = new WeakSet();

function declareSlot(scope, name, el) {
  if (!(el instanceof Element)) throw new TypeError(`slot ${name} needs an element`);
  const record = { el, plugin: scope.plugin, seq: ++sequence };
  if (!slots.has(name)) slots.set(name, []);
  slots.get(name).push(record);
  mountSlot(name);
  emit('slots', name);
  return scope.add(() => {
    const list = slots.get(name);
    const at = list?.indexOf(record) ?? -1;
    if (at >= 0) list.splice(at, 1);
    // The items leave the element that goes; the slot's next element, if
    // any, takes them.
    for (const child of [...record.el.childNodes]) if (placed.has(child)) child.remove();
    mountSlot(name);
    emit('slots', name);
  });
}

function slotElement(name) {
  const list = slots.get(name);
  if (!list?.length) return null;
  return list.reduce((best, record) => (later(record, best) > 0 ? record : best)).el;
}

function mountSlot(name) {
  const el = slotElement(name);
  const items = contributions(`slot:${name}`, { unique: 'id' });
  const nodes = items.map((item) => item.node).filter((node) => node instanceof Node);
  const wanted = new Set(nodes);
  for (const record of slots.get(name) || []) {
    for (const child of [...record.el.childNodes]) if (placed.has(child) && !wanted.has(child) && child.dataset?.slot === name) child.remove();
  }
  if (!el) {
    // Waiting for the slot: the nodes stay out of the page.
    for (const node of nodes) if (node.parentNode && node.dataset?.slot === name) node.remove();
    return;
  }
  let cursor = nextPlaced(el.firstChild);
  for (const node of nodes) {
    placed.add(node);
    if (node.dataset) node.dataset.slot = name;
    if (node === cursor) {
      cursor = nextPlaced(cursor.nextSibling);
      continue;
    }
    el.insertBefore(node, cursor);
  }
}

function nextPlaced(node) {
  while (node && !placed.has(node)) node = node.nextSibling;
  return node;
}

function mount(scope, slot, spec) {
  let item;
  if (spec instanceof Node) item = { node: spec };
  else if (typeof spec === 'function') item = { node: spec() };
  else {
    item = { ...spec };
    if (!('node' in item) && typeof spec.render === 'function') item.node = spec.render();
  }
  if (item.node != null && !(item.node instanceof Node)) throw new TypeError(`what is mounted in ${slot} must be a DOM node`);
  const dispose = contribute(scope, `slot:${slot}`, item);
  return scope.add(() => {
    dispose();
    // A node that is no one's any more leaves at once, not with the batch.
    if (item.node?.dataset?.slot === slot) item.node.remove();
  });
}

// ---------------------------------------------------------------- keys

// Keys are contributions to "keys": { key, run, when, global, priority,
// views, repeat }. key is what KeyboardEvent.key says, after modifiers:
// "n", "E", "?", "Escape", "Mod+k", "Alt+ArrowUp". A key not global does
// nothing while the user types in a field or a guard says so (a dialog is
// open), and one with views only on those views. The first key that runs
// and does not return false has the event.
function comboOf(event) {
  const parts = [];
  if (event.metaKey || event.ctrlKey) parts.push('Mod');
  if (event.altKey) parts.push('Alt');
  if (event.shiftKey && event.key.length > 1) parts.push('Shift');
  parts.push(parts.length && event.key.length === 1 ? event.key.toLowerCase() : event.key);
  return parts.join('+');
}

function normalizeKey(key) {
  let rest = String(key);
  const mods = new Set();
  for (let m; (m = /^(mod|cmd|ctrl|meta|alt|option|shift)\+(.+)$/i.exec(rest)); rest = m[2]) {
    const mod = m[1].toLowerCase();
    mods.add(/^(mod|cmd|ctrl|meta)$/.test(mod) ? 'Mod' : mod === 'shift' ? 'Shift' : 'Alt');
  }
  const out = ['Mod', 'Alt', 'Shift'].filter((m) => mods.has(m));
  out.push(out.length && rest.length === 1 ? rest.toLowerCase() : rest);
  return out.join('+');
}

function onKey(event) {
  if (event.defaultPrevented || event.isComposing) return;
  const combo = comboOf(event);
  const bindings = contributions('keys')
    .filter((b) => (Array.isArray(b.key) ? b.key : [b.key]).some((k) => normalizeKey(k) === combo))
    .sort((a, b) => (b.priority ?? 0) - (a.priority ?? 0));
  if (!bindings.length) return;
  const typing = typingIn(event.target);
  let guarded = null;
  const view = store.get('view');
  for (const binding of bindings) {
    if (!binding.global) {
      if (typing) continue;
      guarded ??= contributions('keys.guard').some((guard) => !!safely(guard.plugin, guard.fn, event));
      if (guarded) continue;
    }
    if (binding.views && !binding.views.includes(view)) continue;
    if (binding.repeat === false && event.repeat) continue;
    if (binding.when && !safely(binding.plugin, binding.when, event)) continue;
    const result = safely(binding.plugin, binding.run, event);
    if (result === false) continue;
    event.preventDefault();
    return;
  }
}

// ---------------------------------------------------------------- routes

// Routes are contributions to "routes": { match(hash), enter(match, hash),
// priority }. The first route by priority whose match answers takes the
// address.
let routing = '';

function route() {
  const hash = location.hash;
  const list = contributions('routes').sort((a, b) => (b.priority ?? 0) - (a.priority ?? 0));
  for (const r of list) {
    const match = safely(r.plugin, r.match, hash);
    if (match) {
      safely(r.plugin, r.enter, match, hash);
      return true;
    }
  }
  return false;
}

function onAddress() {
  // popstate and hashchange both come for a step back; one route will do.
  if (routing === location.href) return;
  routing = location.href;
  queueMicrotask(() => { routing = ''; });
  route();
}

// ---------------------------------------------------------------- rendering

// render asks for a redraw: "render" fires on the next frame, however many
// asked. Plugins redraw what they show from the state they read then.
let frame = 0;

function render() {
  if (frame) return;
  frame = requestAnimationFrame(renderNow);
}

function renderNow() {
  if (frame) cancelAnimationFrame(frame);
  frame = 0;
  emit('render');
}

// ---------------------------------------------------------------- styles

function addStyle(scope, css) {
  const sheet = new CSSStyleSheet();
  sheet.replaceSync(String(css));
  document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
  return scope.add(() => { document.adoptedStyleSheets = document.adoptedStyleSheets.filter((s) => s !== sheet); });
}

function linkStyle(scope, href) {
  const link = h('link', { rel: 'stylesheet', href, data: { plugin: scope.plugin } });
  document.head.append(link);
  return scope.add(() => link.remove());
}

// ---------------------------------------------------------------- plugins

// host is what the kernel knows of plugins: the listing of the workspace
// in view, and the plugins loaded from it.
const host = {
  listing: null,
  ws: null, // the workspace whose plugins are loaded
  loaded: new Map(), // name → Instance
  hot: new Map(), // name → data kept across a plugin's versions
  ticket: 0,
  source: null, // the stream that says when plugins change
  booted: false,
  safe: new URLSearchParams(location.search).has('safe'),
  reloads: 0,
  order: [], // the plugins by the order they load
  instance: '', // the run of the server the page talks to
};

// wanted says whether a listed plugin has anything for the page.
function wanted(meta) {
  if (!meta.active) return false;
  if (host.safe && meta.source !== 'builtin') return false;
  return !!(meta.script || meta.style || meta.commands?.length);
}

// ordered sorts plugins so that each comes after those it names in after,
// keeping the listing's order otherwise.
function ordered(list) {
  const byName = new Map(list.map((meta) => [meta.name, meta]));
  const out = [];
  const state = new Map();
  const visit = (meta) => {
    if (state.get(meta.name) === 2) return;
    if (state.get(meta.name) === 1) return; // a cycle: the listing's order decides
    state.set(meta.name, 1);
    for (const name of meta.after || []) {
      const dependency = byName.get(name);
      if (dependency) visit(dependency);
    }
    state.set(meta.name, 2);
    out.push(meta);
  };
  list.forEach(visit);
  return out;
}

// reconcile makes the loaded plugins those the listing says: it takes away
// the plugins that went, loads those that came, and loads anew those whose
// files changed — their new modules imported before the old ones leave, so
// a plugin that no longer loads keeps its running version.
async function reconcile(listing, { force = false } = {}) {
  const ticket = ++host.ticket;
  host.listing = listing;
  // Another run of the server than before: it restarted, with a new build.
  const instance = listing?.server?.instance;
  if (instance && host.instance && instance !== host.instance) emit('server-restarted', listing.server);
  if (instance) host.instance = instance;
  const want = ordered((listing?.plugins || []).filter(wanted));
  // Style sheets cascade in the order plugins load: a plugin's rules come
  // after those of the plugins it loads after, and a user's after the
  // built-in ones.
  host.order = want.map((meta) => meta.name);
  const names = new Set(want.map((meta) => meta.name));
  for (const [name, instance] of [...host.loaded]) {
    if (!names.has(name)) unload(instance);
  }
  const jobs = [];
  for (const meta of want) {
    const running = host.loaded.get(meta.name);
    // A plugin is the same while its files are: its addresses differ from
    // one workspace to another, and a plugin both have stays as it is.
    const same = running && !force && !running.broken && running.meta.code_version === meta.code_version && running.meta.source === meta.source &&
      !!running.meta.script === !!meta.script && JSON.stringify(running.meta.commands) === JSON.stringify(meta.commands);
    if (same) {
      if (running.meta.style_version !== meta.style_version || !!running.meta.style !== !!meta.style) swapStyle(running, meta.style);
      running.meta = meta;
      continue;
    }
    // A version that failed to load is not tried again until it changes.
    if (running && !force && (running.stale?.version === meta.code_version || (running.broken && running.meta.code_version === meta.code_version))) continue;
    const url = meta.script && (force ? `${meta.script}${meta.script.includes('?') ? '&' : '?'}reload=${++host.reloads}` : meta.script);
    const module = url ? import(url).then((m) => ({ module: m }), (error) => ({ error })) : Promise.resolve({ module: null });
    const style = meta.style ? preloadStyle(meta) : Promise.resolve(null);
    jobs.push({ meta, running, module, style });
  }
  // Everything is fetched first; then the old versions go and the new ones
  // start in one go, with nothing else happening in between.
  const results = await Promise.all(jobs.map((job) => Promise.all([job.module, job.style])));
  if (ticket !== host.ticket) {
    for (const [, link] of results) link?.remove();
    return; // a newer listing took over
  }
  const changed = [];
  for (const [n, job] of jobs.entries()) {
    const [{ module, error }, link] = results[n];
    if (error) {
      link?.remove();
      failed(job.meta.name, error, job.running ? 'load its new version' : 'load');
      if (job.running) job.running.stale = { version: job.meta.code_version, error: String(error?.message || error) };
      else host.loaded.set(job.meta.name, broken(job.meta, error));
      continue;
    }
    if (job.running) unload(job.running, { keepHot: true });
    activate(job.meta, module, link, !!job.running && !job.running.broken);
    changed.push({ name: job.meta.name, reloaded: !!job.running });
  }
  if (ticket !== host.ticket) return;
  for (const change of changed) if (change.reloaded) emit('plugin-reloaded', change.name);
  emit('plugins', listing);
  if (host.booted && changed.length) route();
  render();
  bootDone();
}

// broken is what stands for a plugin whose module did not load.
function broken(meta, error) {
  return { meta, scope: new Scope(meta.name), link: null, error: String(error?.message || error), since: Date.now(), broken: true };
}

// preloadStyle fetches a plugin's style sheet before the plugin starts, so
// what it builds is never shown unstyled.
function preloadStyle(meta) {
  return new Promise((resolve) => {
    const link = h('link', { rel: 'stylesheet', href: meta.style, data: { plugin: meta.name } });
    const done = () => resolve(link);
    link.addEventListener('load', done, { once: true });
    link.addEventListener('error', done, { once: true });
    placeStyle(link, meta.name);
  });
}

// placeStyle puts a plugin's style sheet in the order plugins load, so a
// later plugin's rules win over an earlier one's.
function placeStyle(link, name) {
  const order = host.order || [];
  const at = order.indexOf(name);
  const after = [...document.head.querySelectorAll('link[data-plugin]')].find((other) => order.indexOf(other.dataset.plugin) > at);
  document.head.insertBefore(link, after || null);
}

// swapStyle takes up a plugin's changed style sheet: the new one is in
// before the old one leaves, so nothing flashes unstyled.
function swapStyle(instance, href) {
  const old = instance.link;
  if (!href) {
    old?.remove();
    instance.link = null;
    return;
  }
  const link = h('link', { rel: 'stylesheet', href, data: { plugin: instance.meta.name } });
  const done = () => { if (instance.link === link) old?.remove(); };
  link.addEventListener('load', done, { once: true });
  link.addEventListener('error', done, { once: true });
  if (old?.parentNode) old.after(link); else placeStyle(link, instance.meta.name);
  instance.link = link;
  emit('plugin-restyled', instance.meta.name);
}

function activate(meta, module, link, reloaded = false) {
  const scope = new Scope(meta.name);
  const instance = { meta, scope, link, error: '', since: Date.now(), module, previous: reloaded };
  host.loaded.set(meta.name, instance);
  scope.add(() => instance.link?.remove());
  // The commands its manifest declares send their prompt.
  for (const command of meta.commands || []) {
    contribute(scope, 'commands', {
      name: command.name, args: command.args || '', help: command.description, source: 'manifest', order: 1000,
      run: (arg) => prompt(expandPrompt(command.prompt, arg)),
    });
  }
  if (!module) return instance;
  const cockpit = createAPI(instance);
  const start = module.default || module.activate;
  try {
    const result = typeof start === 'function' ? start(cockpit) : undefined;
    if (result && typeof result.then === 'function') {
      result.then((stop) => { if (typeof stop === 'function') scope.add(stop); }, (error) => {
        instance.error = String(error?.message || error);
        failed(meta.name, error, 'start');
      });
    } else if (typeof result === 'function') {
      scope.add(result);
    }
    if (typeof module.deactivate === 'function') scope.add(() => module.deactivate(cockpit));
  } catch (error) {
    instance.error = String(error?.message || error);
    failed(meta.name, error, 'start');
  }
  return instance;
}

function unload(instance, { keepHot = false } = {}) {
  if (host.loaded.get(instance.meta.name) === instance) host.loaded.delete(instance.meta.name);
  instance.scope.close();
  instance.link?.remove();
  if (!keepHot && !host.loaded.has(instance.meta.name)) host.hot.delete(instance.meta.name);
}

function expandPrompt(template, args) {
  args = String(args || '').trim();
  if (template.includes('{{args}}')) return template.replaceAll('{{args}}', args).trim();
  return args ? `${template.trim()}\n\n${args}` : template.trim();
}

// ---------------------------------------------------------------- following the server

// follow streams the workspace's plugin listing; each listing that comes is
// reconciled with what runs.
function follow(ws) {
  host.source?.close();
  const source = new EventSource(ws ? `${wsPath(ws, '/plugins/events')}` : '/api/plugins/events');
  host.source = source;
  source.addEventListener('plugins', (event) => {
    if (host.source !== source) return;
    let listing;
    try { listing = JSON.parse(event.data); } catch { return; }
    if (listing.workspace && host.ws && listing.workspace !== host.ws) return;
    reconcile(listing);
  });
  source.addEventListener('kernel', () => {
    if (host.source !== source) return;
    // The page itself changed: it loads again where it was.
    emit('kernel-changed');
    setTimeout(() => location.reload(), 60);
  });
  source.addEventListener('open', () => emit('plugins-connected'));
  // The server builds itself anew as its Go code changes, and says how it
  // goes: building, failed, waiting, restarting.
  source.addEventListener('server', (event) => {
    if (host.source !== source) return;
    try { emit('server', JSON.parse(event.data)); } catch { /* not ours */ }
  });
}

// switchWorkspace loads the plugins of another workspace: its own come, the
// last one's go, and those both share stay as they are.
async function switchWorkspace(ws) {
  if (ws === host.ws) return;
  host.ws = ws;
  follow(ws);
  let listing;
  try {
    listing = await api(wsPath(ws, '/plugins'));
  } catch (error) {
    toast(`Plugins did not load: ${error.message}`, 'error', 'plugins');
    return;
  }
  if (ws !== host.ws) return;
  await reconcile(listing);
}

// reloadPlugins asks the server for the listing again; with force, every
// plugin is loaded anew even if its files did not change.
async function reloadPlugins({ force = false } = {}) {
  const listing = await api(wsPath(host.ws, '/plugins'));
  await reconcile(listing, { force });
  return listing;
}

// ---------------------------------------------------------------- what plugins can do

// The prompt a manifest's command sends goes to the session service.
function prompt(text) {
  const session = current('session');
  if (!session?.prompt) return toast('No plugin runs prompts: the session plugin is off.', 'error');
  return session.prompt(String(text));
}

function serviceCall(name, method, ...args) {
  const service = current(name);
  if (typeof service?.[method] !== 'function') {
    toast(`That needs the ${name} plugin, which is off.`, 'error', `missing-${name}`);
    return undefined;
  }
  return service[method](...args);
}

// actions are what the cockpit's own commands do, for plugins to call; each
// is a service's, and says so when the plugin that provides it is off.
const actions = Object.freeze({
  compact: (focus = '') => serviceCall('session', 'compact', String(focus), { fromComposer: true }),
  continueRun: () => serviceCall('session', 'continueRun'),
  stop: () => serviceCall('session', 'stopCommand'),
  editPrompt: (n = '') => serviceCall('edit', 'command', String(n)),
  newSession: () => serviceCall('session', 'newSession'),
  openSession: (id) => serviceCall('session', 'open', String(id)),
  resume: (query = '') => serviceCall('session', 'resume', String(query)),
  rename: (title = '') => serviceCall('session', 'renameCommand', String(title)),
  pin: () => serviceCall('session', 'pinCommand'),
  fork: (n = '') => serviceCall('session', 'forkCommand', String(n)),
  exportSession: () => serviceCall('session', 'exportCommand'),
  deleteSession: () => serviceCall('session', 'deleteCommand'),
  effort: (level = '') => serviceCall('effort', 'command', String(level)),
  model: (id = '') => serviceCall('models', 'command', String(id).trim()),
  openSettings: () => serviceCall('connection', 'open'),
  openAccounts: () => serviceCall('accounts', 'show'),
  addAccount: (provider = '') => serviceCall('accounts', 'addAccount', String(provider)),
  addEndpoint: (kind = '') => serviceCall('accounts', 'addEndpoint', String(kind)),
  workspace: (query = '') => serviceCall('workspaces', 'command', String(query)),
  copyAnswer: () => serviceCall('session', 'copyAnswer'),
  expandAll: (open) => serviceCall('timeline', 'setAllTools', !!open),
  toggleChanges: () => serviceCall('changes', 'toggle'),
  toggleInspector: () => serviceCall('inspector', 'toggle'),
  toggleTheme: () => serviceCall('theme', 'toggle'),
  openHelp: () => serviceCall('help', 'open'),
  showPlugins: () => serviceCall('plugins', 'show'),
  trustPlugins: (trusted) => serviceCall('plugins', 'trust', !!trusted),
  enablePlugin: (name, enabled) => serviceCall('plugins', 'enable', String(name), !!enabled),
  reloadPlugins: () => serviceCall('plugins', 'reload'),
  queue: (arg = '') => serviceCall('queue', 'command', String(arg).trim()),
});

// copyEntry is what a plugin gets of an entry: its own copy.
function copyEntry(entry) {
  return entry && { ...entry, tool: entry.tool && { ...entry.tool }, images: entry.images && [...entry.images] };
}

// createAPI makes the cockpit API a plugin's module is started with. What a
// plugin adds through it belongs to the plugin, and goes when it does.
function createAPI(instance) {
  const { meta, scope } = instance;
  const name = meta.name;
  if (!host.hot.has(name)) host.hot.set(name, {});
  const guard = (fn) => (typeof fn === 'function' ? (...args) => safely(name, fn, ...args) : fn);
  const cockpit = {
    version: API_VERSION,
    plugin: Object.freeze({ ...meta }),
    h, svg, fmt, uuid, prefs, ApiError,
    api: (path, options) => api(String(path).startsWith('/api/') ? path : wsPath(host.ws, path), options),
    wsPath,
    // hot.data is kept across the plugin's versions: a plugin loaded anew
    // finds there what the version before left.
    hot: Object.freeze({ data: host.hot.get(name), get reloaded() { return !!instance.previous; } }),
    onDispose: (fn) => scope.add(fn),
    listen(target, type, fn, options) {
      const handler = guard(fn);
      target.addEventListener(type, handler, options);
      return scope.add(() => target.removeEventListener(type, handler, options));
    },
    timeout(fn, ms) {
      const id = setTimeout(() => { cancel(); safely(name, fn); }, ms);
      const cancel = scope.add(() => clearTimeout(id));
      return cancel;
    },
    interval(fn, ms) {
      const id = setInterval(() => safely(name, fn), ms);
      return scope.add(() => clearInterval(id));
    },
    on: (event, fn) => on(scope, event, fn),
    emit: (event, ...args) => emit(event, ...args),
    listening: (event) => listening(event),
    provide: (service, impl) => provide(scope, service, impl),
    use: (service) => use(service),
    has: (service) => !!current(service),
    contribute: (point, item) => contribute(scope, point, item),
    contributions: (point, options) => contributions(point, options),
    hooks: Object.freeze({
      tap: (hook, fn, options) => tap(scope, hook, fn, options),
      run: (hook, value, ...args) => runHooks(hook, value, ...args),
      runAsync: (hook, value, ...args) => runHooksAsync(hook, value, ...args),
      first: (hook, ...args) => firstHook(hook, ...args),
    }),
    store: Object.freeze({
      get: (key) => store.get(key),
      set: (key, value) => storeSet(key, value),
      watch: (key, fn) => on(scope, `store:${key}`, fn),
    }),
    render,
    renderNow,
    safely: (fn, ...args) => safely(name, fn, ...args),
    ui: Object.freeze({
      slot: (slot, el) => declareSlot(scope, slot, el),
      mount: (slot, spec) => mount(scope, slot, spec),
      hide: (slot, id) => mount(scope, slot, { id, node: null }),
      slots: () => [...slots.keys()].filter((key) => slots.get(key).length),
      items: (slot) => contributions(`slot:${slot}`, { unique: 'id' }).map((item) => ({ id: item.id, order: item.order ?? 0, plugin: item.plugin })),
      kv,
      button: (label, run, title = '') => h('button', { class: 'act', type: 'button', title, onclick: () => safely(name, run) }, label),
    }),
    keys: Object.freeze({
      register: (spec) => contribute(scope, 'keys', spec),
      guard: (fn) => contribute(scope, 'keys.guard', { fn }),
    }),
    routes: Object.freeze({
      register: (spec) => contribute(scope, 'routes', spec),
    }),
    route,
    styles: Object.freeze({
      add: (css) => addStyle(scope, css),
      link: (href) => linkStyle(scope, href),
    }),
    host: Object.freeze({
      listing: () => host.listing,
      workspace: () => host.ws,
      setWorkspace: (ws) => switchWorkspace(ws),
      booted: () => host.booted,
      loaded: () => [...host.loaded.values()].map((i) => ({
        name: i.meta.name, source: i.meta.source, code_version: i.meta.code_version, style_version: i.meta.style_version,
        error: i.error || '', broken: !!i.broken, stale: i.stale || null, since: i.since,
      })),
      failures: () => Object.fromEntries(failures),
      reload: (options) => reloadPlugins(options),
      safe: host.safe,
    }),

    // The API plugins had first, kept as it was.
    commands: Object.freeze({
      register(spec) {
        if (!/^[a-z?][a-z0-9-]*$/.test(spec?.name || '') || typeof spec.run !== 'function') {
          throw new Error('a command needs a lowercase name and a run function');
        }
        return contribute(scope, 'commands', { help: '', order: 1000, ...spec });
      },
    }),
    palette: Object.freeze({
      register: (spec) => contribute(scope, 'palette', { group: 'Plugins', icon: '◇', order: 400, ...spec }),
    }),
    inspector: Object.freeze({
      register(spec) {
        const dispose = contribute(scope, 'inspector.section', { id: `${name}-${++sequence}`, order: 50, ...spec });
        render();
        return scope.add(() => { dispose(); render(); });
      },
      refresh: render,
    }),
    tools: Object.freeze({
      // register renders the calls of a tool: summary(entry) is the line in the
      // timeline, render(entry, helpers) the body of the open call.
      register: (tool, spec) => contribute(scope, 'tool.view', { ...spec, tool: String(tool) }),
    }),
    view: () => current('session')?.summary?.() ?? null,
    entries: () => (current('session')?.entries?.() || []).map(copyEntry),
    sessions: () => current('session')?.sessionList?.() || [],
    workspaces: () => current('session')?.workspaceList?.() || [],
    effort: () => current('effort')?.info?.() || { current: 'high', levels: ['low', 'medium', 'high', 'xhigh', 'max'] },
    models: () => current('models')?.info?.() || { current: '', default: '', list: [] },
    plugins: () => host.listing,
    prompt: (text) => prompt(text),
    toast: (text, kind = 'info', key = '') => toast(text, kind, key),
    copy: (text, label) => (current('clipboard')?.copy ? current('clipboard').copy(String(text), label) : navigator.clipboard?.writeText(String(text))),
    actions,
  };
  return Object.freeze(cockpit);
}

// ---------------------------------------------------------------- boot

// Until the plugins have built the page, it says it is loading; with no
// plugin to build it, it says why.
let bootTimer = 0;

function bootDone() {
  if (host.booted) return;
  host.booted = true;
  document.getElementById('kernel-boot')?.remove();
  document.documentElement.dataset.kernel = 'ready';
  emit('ready');
  route();
  render();
  clearTimeout(bootTimer);
  bootTimer = setTimeout(checkBuilt, 1500);
}

// checkBuilt offers a way back when nothing built the page: every plugin
// that would have is off, or failed.
function checkBuilt() {
  // Plugins that build parts of the page into slots of a layout that is
  // not there build nothing that shows.
  const built = [...document.body.children].some((el) => !el.matches('#kernel-boot, .kernel-toasts, .kernel-rescue, script'));
  if (built) return;
  const listing = host.listing;
  const box = h('div', { class: 'kernel-rescue' },
    h('h1', { text: 'No plugin built this page' }),
    h('p', { text: 'The cockpit is made of plugins, and those that build its layout are off or failed to load. Turn them back on:' }),
    h('ul', null, (listing?.plugins || []).filter((p) => p.source === 'builtin' && (p.script || p.style || !p.active)).map((p) => h('li', null,
      h('b', { text: p.name }), ' ', h('span', { text: p.active ? (host.loaded.get(p.name)?.error || 'on') : p.reason || 'off' }), ' ',
      !p.active && p.reason === 'turned off' ? h('button', {
        type: 'button', text: 'Turn on',
        onclick: async () => {
          await api(wsPath(host.ws, `/plugins/${encodeURIComponent(p.name)}/enabled`), { method: 'PUT', body: { enabled: true } }).catch((e) => toast(e.message, 'error'));
          location.reload();
        },
      }) : null))),
    [...failures].length ? h('pre', { text: [...failures].map(([plugin, f]) => `${plugin}: ${f.message}`).join('\n') }) : null,
    h('p', null, h('a', { href: '?safe#/', text: 'Open with the built-in plugins only' })));
  document.body.append(box);
}

async function boot() {
  document.addEventListener('keydown', onKey);
  window.addEventListener('hashchange', onAddress);
  window.addEventListener('popstate', onAddress);
  if (host.safe) document.documentElement.dataset.safe = 'true';
  let listing = null;
  for (let attempt = 0; !listing; attempt++) {
    try {
      listing = await api('/api/plugins');
    } catch (error) {
      const box = document.getElementById('kernel-boot');
      if (box) box.textContent = `The server does not answer (${error.message}); trying again…`;
      await new Promise((resolve) => setTimeout(resolve, Math.min(8000, 800 * 2 ** attempt)));
    }
  }
  host.ws = listing.workspace || null;
  follow(host.ws);
  await reconcile(listing);
  bootDone();
}

// The kernel's own face, for the console and for tests.
window.cockpitKernel = Object.freeze({
  version: API_VERSION,
  loaded: () => [...host.loaded.keys()],
  instances: () => [...host.loaded.values()].map((i) => ({ name: i.meta.name, source: i.meta.source, code: i.meta.code_version, style: i.meta.style_version, since: i.since, error: i.error || '', stale: i.stale || null })),
  listing: () => host.listing,
  failures: () => Object.fromEntries(failures),
  services: () => [...services.keys()].filter((key) => services.get(key).length),
  slots: () => [...slots.keys()].filter((key) => slots.get(key).length),
  points: () => [...points.keys()],
  contributions: (point) => contributions(point),
  use,
  store: (key) => store.get(key),
  reload: (options) => reloadPlugins(options),
});

boot();
