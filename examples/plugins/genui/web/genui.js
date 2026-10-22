// genui: the web part of the genui plugin. The UI tool keeps each version of
// a surface in .harness/ui/<session>/<surface>/v<N>.json, and names it on
// the first line of its result. Here the newest call of each surface in the
// session on screen is drawn as a card in the transcript, live as the files
// it reads change; the calls before it stay rows, which open to the
// snapshot their version took. A card opens large over the transcript, or
// is pinned to the sidebar, where it follows the surface's newest version;
// while the newest card has scrolled away above, a chip at the top of the
// transcript stands for it, as long as the surface is the current
// exchange's (see «the chip»). A button's event reaches the agent as a
// message of the user's. An Html component is a page of its own, which the
// cockpit serves sandboxed and a card frames (see «live HTML» below).
//
// This is a prototype on the plugin API alone: the page reads the files
// (every POLL ms, while a card is in view and the page is), so a surface is
// live while the cockpit is open. The terminal (cmd/kou-conveyor-tui) and
// the iPhone app (ios/KouConveyor) draw the same versions on their own.

import { MISSING, get, set, resolve, inferFormat, parseSource, callOf, fill, shown, num } from './data.js';
import { renderSurface } from './surface.js';

const POLL = 1500;
const PREFS = 'genui';
const time = (stamp) => Date.parse(stamp || '') || 0; // the tool and the server write their times differently
// isUI is whether a tool is the UI tool: a transcript may keep it under an
// alias, <word>_UI, or mcp__<server>__<word>_UI.
const isUI = (name) => name === 'UI' || (typeof name === 'string' && name.endsWith('_UI'));
// put sets an element's children, skipping the absent: the DOM's own
// replaceChildren would write null as text.
const put = (el, ...kids) => el.replaceChildren(...kids.flat().filter((k) => k != null && k !== false && k !== ''));
const KEEP = 200; // versions whose inputs and used buttons are remembered
const ICON = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 1.8 14.2 8 8 14.2 1.8 8Z" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M8 5.6 10.4 8 8 10.4 5.6 8Z" fill="currentColor"/></svg>';

export default function activate(cockpit) {
  const { h, fmt } = cockpit;
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const currentWs = () => service('session')?.view?.()?.ws || cockpit.host?.workspace?.() || '';
  const fileUrl = (ws, path) => cockpit.wsPath(ws, `/file?path=${encodeURIComponent(path)}`);
  const rawUrl = (ws, path) => cockpit.wsPath(ws, `/raw?path=${encodeURIComponent(path)}`);
  const folderOf = (path) => (path || '').split('/')[2] || '';

  // readFile reads a file of the workspace as the file view does: its text,
  // or { missing } when there is none.
  async function readFile(ws, path) {
    try {
      return await cockpit.api(fileUrl(ws, path));
    } catch (error) {
      if (error?.status === 404) return { missing: true };
      throw error;
    }
  }

  // A version's file never changes once written: it is read once.
  const versions = new Map();
  function loadVersion(ws, path) {
    const key = `${ws}\n${path}`;
    let promise = versions.get(key);
    if (!promise) {
      promise = readFile(ws, path).then((r) => {
        if (r.missing) throw new Error(`There is no ${path} any more.`);
        if (typeof r.text !== 'string') throw new Error(`${path} is not text.`);
        return JSON.parse(r.text);
      });
      promise.catch(() => versions.delete(key));
      versions.set(key, promise);
    }
    return promise;
  }

  // A file a Code component shows, read again at most every POLL ms.
  const codes = new Map();
  function codeOf(ws, c) {
    const key = `${ws}\n${c.file}`;
    const seen = codes.get(key);
    if (seen && Date.now() - seen.at < POLL) return seen.promise;
    const promise = readFile(ws, c.file).then((r) => {
      if (r.missing) return { error: `There is no ${c.file}.` };
      if (typeof r.text !== 'string') return { error: r.too_large ? `${c.file} is bigger than 8 MB.` : `${c.file} is not text.` };
      return { text: r.text, runs: r.runs || [], classes: r.classes || [] };
    });
    codes.set(key, { at: Date.now(), promise });
    return promise;
  }

  const markdown = (text) => (service('markdown')?.render ? service('markdown').render(text) : h('p', { text }));

  function openFile(path) {
    if (!path) return;
    const explorer = service('explorer');
    if (explorer?.open) explorer.open(path);
    else cockpit.toast(`The Files plugin is off: ${path}`, 'warn', 'genui');
  }

  // ---------------------------------------------------------------- what the user did

  // Inputs and used once-buttons, by version: a new version starts from
  // what the agent sent.
  const kept = (() => {
    const stored = cockpit.prefs.get(PREFS, null);
    return stored && typeof stored === 'object' ? { edits: stored.edits || {}, used: stored.used || {}, order: stored.order || [] } : { edits: {}, used: {}, order: [] };
  })();
  function touch(path) {
    kept.order = kept.order.filter((p) => p !== path);
    kept.order.push(path);
    for (const old of kept.order.splice(0, Math.max(0, kept.order.length - KEEP))) {
      delete kept.edits[old];
      for (const key of Object.keys(kept.used)) if (key.startsWith(`${old}#`)) delete kept.used[key];
    }
    cockpit.prefs.set(PREFS, kept);
  }

  // The cards that draw each version, to draw again what another changed.
  const cardsOf = new Map(); // version path → Set of cards
  function redrawOthers(path, card) {
    for (const other of cardsOf.get(path) || []) if (other !== card) other.redraw();
  }

  // ---------------------------------------------------------------- the calls of a view

  const parsed = new Map(); // entry id → { output, call }
  function callFor(entry) {
    const tool = entry?.tool;
    if (!tool || !isUI(tool.name) || (tool.state || 'done') !== 'done') return null;
    const seen = parsed.get(entry.id);
    if (seen && seen.output === tool.output) return seen.call;
    const call = callOf(tool.output);
    parsed.set(entry.id, { output: tool.output, call });
    return call;
  }

  // heads are the newest call of each surface in a view: those are drawn
  // as cards, the others as rows.
  let memo = null;
  let dirty = true;
  cockpit.on('entry', (entry) => { if (isUI(entry?.tool?.name)) dirty = true; });
  function headsOf(v) {
    if (memo && memo.v === v && memo.n === v.order.length && !dirty) return memo;
    const head = new Map(); // surface → entry id
    const calls = new Map(); // surface → its newest call
    const at = new Map(); // surface → where its newest call is
    v.order.forEach((id, i) => {
      const entry = v.entries.get(id);
      if (entry?.kind !== 'tool') return;
      const call = callFor(entry);
      if (!call) return;
      head.set(call.surface, id);
      calls.set(call.surface, call);
      at.set(call.surface, i);
    });
    let newest = null;
    let newestAt = -1;
    for (const [surface, i] of at) {
      if (!calls.get(surface).deleted && i > newestAt) {
        newest = head.get(surface);
        newestAt = i;
      }
    }
    // A call that was a surface's newest is drawn again, as a row.
    if (memo?.v === v) {
      for (const [surface, id] of memo.head) {
        if (head.get(surface) === id) continue;
        cards.get(id)?.dispose();
        cards.delete(id);
        service('timeline')?.schedule?.(id);
      }
    }
    memo = { v, n: v.order.length, head, calls, newest };
    dirty = false;
    return memo;
  }

  // ---------------------------------------------------------------- live HTML

  // An Html is a page of its own, which the cockpit serves sandboxed, in an
  // origin of none, with window.kou ahead of what the agent wrote
  // (cmd/kou-conveyor-web/surfaceframe.go); a card frames it. The page
  // reaches only its card, by messages. It gets the cockpit's theme and the
  // values of the pointers its "inputs" name, and says how tall it is. While
  // the user works in it, it sets those values (not the files'), sends the
  // events its "actions" name, and opens the surface's files. Its frame
  // outlives the card being drawn again (moveBefore); put elsewhere, it
  // loads again, and gets its data again.
  const FRAME_MIN = 24;
  const FRAME_MAX = 1600;
  const FRAME_START = 120;
  const EVENT_GAP = 1000; // ms between two events of a page
  const EVENT_BYTES = 8 << 10;
  const VALUE_BYTES = 32 << 10;
  const MOVES = typeof Element.prototype.moveBefore === 'function';
  const TOKENS = ['--bg', '--bg-2', '--bg-3', '--bg-4', '--line', '--line-2', '--fg', '--fg-2', '--fg-3', '--fg-4',
    '--accent', '--accent-ink', '--accent-soft', '--ok', '--warn', '--err', '--err-soft', '--code', '--selection', '--mono', '--sans',
    '--radius', '--radius-sm', '--radius-lg'];
  const pages = new Set(); // every page framed, to know whose a message is
  // The height each page last had, by its URL: the timeline lets go of a
  // card far from the view, and one drawn again frames its page at the
  // height it had rather than growing from FRAME_START under the reader.
  const heights = new Map();
  const HEIGHTS = 256;
  const inputsOf = (c) => (Array.isArray(c?.inputs) ? c.inputs.filter((p) => typeof p === 'string' && p.startsWith('/')) : []);
  const namesOf = (c) => (Array.isArray(c?.actions) ? c.actions.filter((n) => typeof n === 'string' && n.trim()) : []);

  // themeOf is the theme as a page gets it: the tokens, and the series'
  // colours as --s0 … --s5, as they are where it is framed.
  function themeOf(el) {
    const style = getComputedStyle(el?.isConnected ? el : document.documentElement);
    const vars = {};
    for (const name of TOKENS) {
      const value = style.getPropertyValue(name).trim();
      if (value) vars[name] = value;
    }
    for (let i = 0; i < 6; i++) {
      const value = style.getPropertyValue(`--gu-s${i}`).trim();
      if (value) vars[`--s${i}`] = value;
    }
    const declared = getComputedStyle(document.documentElement).colorScheme || '';
    const light = /\blight\b/.test(declared);
    const scheme = light !== /\bdark\b/.test(declared) ? (light ? 'light' : 'dark')
      : matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
    return { scheme, vars };
  }

  // A page's messages: a sandboxed page's origin is "null", and its window
  // is the one of a frame drawn here.
  cockpit.listen(window, 'message', (event) => {
    const m = event.data;
    if (!event.source || event.origin !== 'null' || !m || typeof m !== 'object' || m.kou !== 1 || typeof m.type !== 'string') return;
    for (const page of pages) {
      if (page.iframe?.contentWindow === event.source) {
        page.heard(m);
        return;
      }
    }
  });

  // The theme changed: every page gets it again.
  let repaint = 0;
  function themeChanged() {
    if (repaint) return;
    repaint = requestAnimationFrame(() => {
      repaint = 0;
      for (const page of [...pages]) page.paint();
    });
  }
  const themes = new MutationObserver(themeChanged);
  themes.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme', 'class', 'style'] });
  cockpit.listen(matchMedia('(prefers-color-scheme: light)'), 'change', themeChanged);
  cockpit.on('appearance', themeChanged);
  cockpit.onDispose(() => {
    themes.disconnect();
    if (repaint) cancelAnimationFrame(repaint);
  });

  // Without moveBefore a frame put in a card drawn anew loads again: what its
  // page set is drawn once the user is back in the cockpit, not while they
  // type in the page.
  const later = new Set(); // cards to draw then
  function caughtUp() {
    if (document.activeElement instanceof HTMLIFrameElement) return;
    for (const card of later) if (!card.disposed) card.redraw();
    later.clear();
  }
  cockpit.listen(window, 'focus', caughtUp);
  cockpit.listen(document, 'focusin', caughtUp);

  // framesOf keeps the frames of a drawing of surfaces, a card or a
  // snapshot, as it is drawn again. o is { ws; path(): the version's file;
  // live: whether its pages may set values and send events; files(): the
  // files they may open; edit(pointer, value); event(name, data) }.
  function framesOf(o) {
    const pageOf = new Map(); // its URL → a page
    let used = new Set(); // the pages of the drawing being made

    function urlOf(c) {
      const [, , folder, surface, file] = (o.path() || '').split('/'); // .harness/ui/<folder>/<surface>/v<N>.json
      const version = /^v(\d+)\.json$/.exec(file || '')?.[1];
      if (!folder || !surface || !version) return '';
      return cockpit.wsPath(o.ws, `/sessions/${encodeURIComponent(folder)}/surfaces/${encodeURIComponent(surface)}/html/${encodeURIComponent(c.id)}?v=${version}`);
    }

    // frame is the node of an Html in the drawing being made: its page's
    // own, or, while that is still in the drawing before, a slot that place
    // moves it to.
    function frame(c, doc) {
      const url = urlOf(c);
      if (!url) return h('div', { class: 'gu-none', text: `${c.id}: this Html is of no version.` });
      let page = pageOf.get(url);
      if (page && used.has(page)) return h('div', { class: 'gu-none', text: `${c.id} is shown once, above.` });
      if (!page) {
        page = createPage(url, c);
        pageOf.set(url, page);
      }
      page.c = c;
      page.doc = doc;
      used.add(page);
      if (!page.node.isConnected) return page.node;
      page.slot = h('div', { class: 'gu-html-slot' });
      return page.slot;
    }

    function createPage(url, c) {
      const page = { url, c, doc: null, iframe: null, slot: null, tag: '', ready: false, sent: null, acted: 0, probing: false, gone: false };
      page.note = h('div', { class: 'gu-html-note', hidden: true });
      page.node = h('div', { class: 'gu-html', data: { state: 'loading' } }, h('div', { class: 'gu-none', text: 'Loading the page…' }), page.note);
      // Loading, it stands as tall as it was, if it was framed before.
      if (heights.has(url)) page.node.style.setProperty('--gu-frame-h', `${heights.get(url)}px`);
      page.heard = (m) => heard(page, m);
      page.paint = () => paint(page);
      pages.add(page);
      probe(page);
      return page;
    }

    // probe asks the cockpit for the page, without its body: the page is
    // framed once it is served as a page of a surface, and framed anew as
    // its ETag changes (an Html of a file whose file changed).
    async function probe(page) {
      if (page.probing || page.gone) return;
      page.probing = true;
      try {
        let res;
        try {
          res = await fetch(page.url, { method: 'HEAD', cache: 'no-cache', credentials: 'same-origin' });
        } catch (error) {
          if (!page.iframe && !page.gone) fail(page, `The page did not load: ${error.message}`);
          return;
        }
        if (page.gone) return;
        if (!res.ok || !(res.headers.get('Content-Security-Policy') || '').startsWith('sandbox')) {
          fail(page, await refusal(page.url, res));
          return;
        }
        const tag = res.headers.get('ETag') || '';
        if (!page.iframe || tag !== page.tag) mount(page, tag);
      } finally {
        page.probing = false;
      }
    }

    async function refusal(url, res) {
      if (res.ok) return 'This cockpit serves the page without its sandbox, so it is not framed.';
      let error = '';
      try {
        error = (await (await fetch(url, { cache: 'no-cache', credentials: 'same-origin' })).json())?.error || '';
      } catch { /* not the cockpit's error */ }
      if (res.status === 404 && (!error || error === 'not found')) return 'This cockpit is older than live HTML: it frames the page once it is rebuilt and this page is reloaded.';
      return `The page is not served: ${error || `HTTP ${res.status}`}.`;
    }

    // mount frames the page anew. The theme comes in the URL's fragment, so
    // the page is drawn in it from the first frame; an Html of a file has
    // its ETag in the URL, as the frame's page changes with its file.
    function mount(page, tag) {
      const c = page.c;
      const iframe = h('iframe', { class: 'gu-frame', sandbox: 'allow-scripts', referrerpolicy: 'no-referrer', title: String(c.alt ?? '') });
      const rev = c.file && tag ? `&rev=${encodeURIComponent(tag.replace(/[^A-Za-z0-9]/g, ''))}` : '';
      iframe.src = `${page.url}${rev}#${encodeURIComponent(JSON.stringify(themeOf(page.node)))}`;
      if (!page.iframe) size(page, Number.isInteger(c.height) ? c.height : heights.get(page.url) ?? FRAME_START);
      page.tag = tag;
      page.ready = false;
      page.sent = null;
      // A new frame, not the old one sent elsewhere: no entry in the history.
      if (page.iframe?.parentNode) page.iframe.replaceWith(iframe);
      else put(page.node, iframe, page.note);
      page.iframe = iframe;
      page.node.dataset.state = 'live';
      say(page, '');
    }

    function fail(page, why) {
      page.iframe = null;
      page.ready = false;
      page.node.dataset.state = 'error';
      put(page.node, h('div', { class: 'gu-unknown' },
        h('span', { class: 'gu-badge', data: { tone: 'neutral' }, text: 'HTML' }),
        h('span', { class: 'gu-unknown-alt', text: String(page.c.alt ?? '') }),
        h('span', { class: 'gu-dim', text: why })));
    }

    function size(page, height) {
      const px = Math.max(FRAME_MIN, Math.min(FRAME_MAX, Math.ceil(height)));
      page.node.style.setProperty('--gu-frame-h', `${px}px`);
      heights.delete(page.url);
      heights.set(page.url, px);
      if (heights.size > HEIGHTS) heights.delete(heights.keys().next().value);
    }

    // say puts a line under the page: what it asked that was not done.
    function say(page, text, tone = '') {
      page.note.hidden = !text;
      page.note.textContent = text;
      page.note.dataset.tone = tone;
    }

    // post gives the page its data: the values of its inputs, by pointer;
    // 'data' only when they changed since it last got them.
    function post(page, type) {
      const win = page.iframe?.contentWindow;
      if (!win || !page.ready || !page.doc) return;
      const data = {};
      for (const ptr of inputsOf(page.c)) {
        const value = get(page.doc, ptr);
        data[ptr] = value === MISSING ? null : value;
      }
      const text = JSON.stringify(data);
      if (type === 'data' && text === page.sent) return;
      page.sent = text;
      try {
        win.postMessage(type === 'init' ? { kou: 1, type, data, theme: themeOf(page.iframe) } : { kou: 1, type, data }, '*');
      } catch (error) {
        console.warn('genui:', error);
      }
    }

    // paint gives the page the theme again. A frame out of the document has
    // no page; put back, it loads again, and gets the theme then.
    function paint(page) {
      const win = page.iframe?.contentWindow;
      if (win && page.ready) win.postMessage({ kou: 1, type: 'theme', theme: themeOf(page.iframe) }, '*');
    }

    // working: the user works in the page now. A click or a key in a frame
    // activates the page that frames it too.
    const working = (page) => document.activeElement === page.iframe && (navigator.userActivation?.isActive ?? true);

    function heard(page, m) {
      switch (m.type) {
        case 'ready':
          page.ready = true;
          page.sent = null;
          say(page, '');
          post(page, 'init');
          return;
        case 'size':
          if (!Number.isInteger(page.c.height) && Number.isFinite(m.height)) size(page, m.height);
          return;
        case 'error':
          say(page, `The page's script failed: ${String(m.message ?? '').slice(0, 300)}`, 'err');
          return;
        case 'action':
          return action(page, m);
        case 'set':
          return setValue(page, m);
        case 'open':
          return open(page, m);
        default:
      }
    }

    // json is a value of the page's as JSON, when it is of at most limit
    // characters.
    function json(value, limit) {
      try {
        const text = JSON.stringify(value ?? null);
        return text.length <= limit ? text : null;
      } catch {
        return null;
      }
    }

    function action(page, m) {
      const name = typeof m.name === 'string' ? m.name : '';
      if (!o.live) return say(page, 'This is a snapshot: events work in the newest version.', 'warn');
      if (!namesOf(page.c).includes(name)) return say(page, `"${name}" is not one of this Html's actions, so it was not sent.`, 'err');
      if (!working(page)) return say(page, 'An event goes to the agent only from a click or a key in the page.', 'warn');
      const now = Date.now();
      if (now - page.acted < EVENT_GAP) return;
      const text = json(m.data, EVENT_BYTES);
      if (text == null) return say(page, `An event's data is JSON of at most ${EVENT_BYTES >> 10} KB.`, 'err');
      page.acted = now;
      say(page, '');
      o.event(name, JSON.parse(text));
    }

    function setValue(page, m) {
      const ptr = typeof m.pointer === 'string' ? m.pointer : '';
      if (!o.live) return say(page, 'This is a snapshot: inputs work in the newest version.', 'warn');
      if (!inputsOf(page.c).includes(ptr) || /^\/(sources|sourceInfo)(\/|$)/.test(ptr)) {
        return say(page, `The page sets only its inputs outside /sources, not ${ptr || '""'}.`, 'err');
      }
      if (document.activeElement !== page.iframe) return;
      const text = json(m.value, VALUE_BYTES);
      if (text == null) return say(page, `A value the page sets is JSON of at most ${VALUE_BYTES >> 10} KB.`, 'err');
      o.edit(ptr, JSON.parse(text));
    }

    function open(page, m) {
      const file = String(m.path ?? '').trim().replace(/^\$/, '').replace(/^\.\//, '');
      if (!working(page)) return;
      if (!file || (!o.files().includes(file) && file !== page.c.file)) return say(page, `The page opens only the surface's files, not ${file || '""'}.`, 'err');
      openFile(file);
    }

    function drop(page) {
      page.gone = true;
      pages.delete(page);
      if (pageOf.get(page.url) === page) pageOf.delete(page.url);
    }

    return {
      frame,
      begin() {
        used = new Set();
      },
      // place puts a surface drawn anew in body, in the place of the one
      // before, moving its frames there rather than loading them again;
      // then the pages get what changed of their data.
      place(body, tree) {
        const before = [...body.childNodes];
        body.append(tree);
        const kept = new Set();
        for (const page of used) {
          kept.add(page.node);
          const slot = page.slot;
          page.slot = null;
          if (!slot?.parentNode) continue;
          let moved = false;
          if (MOVES && page.node.isConnected && slot.isConnected) {
            try {
              slot.parentNode.moveBefore(page.node, slot);
              moved = true;
            } catch { /* not where it can move from: it loads again */ }
          }
          if (!moved) slot.parentNode.insertBefore(page.node, slot);
          slot.remove();
        }
        // Taking away what has the focus fires blur, whose handlers may
        // draw the card again first: then the node is gone already.
        for (const node of before) {
          if (node.parentNode !== body || kept.has(node)) continue;
          try { node.remove(); } catch { /* drawn again meanwhile */ }
        }
        for (const page of [...pageOf.values()]) if (!used.has(page)) drop(page);
        for (const page of used) post(page, 'data');
      },
      // clear lets every page go: the drawing shows none.
      clear() {
        for (const page of [...pageOf.values()]) drop(page);
      },
      // refresh gives the pages the data model as it is now, when an input
      // changed it without the surface drawn again.
      refresh(doc) {
        for (const page of pageOf.values()) {
          page.doc = doc;
          post(page, 'data');
        }
      },
      // check asks again for the pages of files: as a file changes, its page
      // is framed anew, and a file not there yet is framed once it is.
      check() {
        for (const page of pageOf.values()) if (page.c?.file) probe(page);
      },
    };
  }

  // ---------------------------------------------------------------- a card

  // createCard draws a surface: the version a call made (call), or the
  // newest of a surface (follow: { folder, surface }), in the transcript,
  // over it or in the sidebar (place).
  function createCard({ ws, session, call = null, follow = null, entryId = null, place }) {
    const card = { place, entryId, path: call?.path || null, title: call?.title || follow?.title || '', glance: '', disposed: false };
    const ui = { tabs: new Map(), open: new Map() };
    let state = null;
    let failure = '';
    let latest = ''; // the version of latest.json last read, when following
    const values = {};
    const infos = {};
    const seen = {}; // source → its file and what was read of it last: a version, or why nothing
    let changedAt = '';
    let pulse = false; // a file changed in the last poll
    let timer = 0;
    let misses = 0;
    let busy = false;

    const title = h('span', { class: 'gu-title' });
    const status = h('span', { class: 'gu-status' });
    const more = h('button', { class: 'gu-icon', type: 'button', title: 'More', 'aria-label': 'More', onclick: () => menu() }, '⋯');
    const head = h('header', { class: 'gu-head' },
      h('span', { class: 'gu-mark', 'aria-hidden': 'true', text: '◇' }), title, status,
      h('span', { class: 'gu-tools' },
        place !== 'overlay' ? h('button', { class: 'gu-icon', type: 'button', title: 'Open large', 'aria-label': 'Open large', onclick: () => overlay.open(spec()) }, '⤢') : null,
        place !== 'tab' ? h('button', { class: 'gu-icon', type: 'button', title: 'Pin to the sidebar', 'aria-label': 'Pin to the sidebar', onclick: () => pin() }, '⇥') : null,
        more,
        place === 'overlay' ? h('button', { class: 'gu-icon gu-close', type: 'button', title: 'Close (Esc)', 'aria-label': 'Close', onclick: () => overlay.hide() }, '✕') : null));
    const body = h('div', { class: 'gu-body' });
    const foot = h('footer', { class: 'gu-foot' });
    card.node = h('div', { class: 'gu-card', data: { place, state: 'loading' } }, head, body, foot);

    const spec = () => ({ ws, session, call, follow, entryId });
    const latestPath = () => `.harness/ui/${follow.folder}/${follow.surface}/latest.json`;
    let soon = 0; // a drawing a page asked for, in the next frame
    const frames = framesOf({
      ws, path: () => card.path, live: true,
      files: () => Object.values(state?.sources || {}).map((d) => d?.file).filter(Boolean),
      // What a page sets reaches the pages at once; the card is drawn again
      // in the next frame, or, where frames do not move, once the user is
      // back in the cockpit.
      edit(ptr, value) {
        edit(ptr, value, false);
        if (!MOVES) return later.add(card);
        soon ||= requestAnimationFrame(() => {
          soon = 0;
          if (card.disposed) return;
          draw();
          redrawOthers(card.path, card);
        });
      },
      event: (name, data) => sendFrom(name, data),
    });

    function register(path) {
      if (card.path && cardsOf.get(card.path)) cardsOf.get(card.path).delete(card);
      card.path = path;
      if (!cardsOf.has(path)) cardsOf.set(path, new Set());
      cardsOf.get(path).add(card);
    }
    if (card.path) register(card.path);

    // load reads the version to draw; true when it is another than before.
    async function load() {
      try {
        if (follow) {
          const r = await readFile(ws, latestPath());
          if (r.missing) throw new Error(`There is no ${latestPath()}.`);
          if (r.version === latest) return false;
          latest = r.version;
          const next = JSON.parse(r.text);
          if (next.deleted) {
            state = null;
            failure = `The agent removed this surface in v${next.version}.`;
            return true;
          }
          if (state?.version === next.version) return false;
          register(`.harness/ui/${follow.folder}/${follow.surface}/v${next.version}.json`);
          state = next;
        } else {
          state = await loadVersion(ws, call.path);
        }
        failure = '';
        return true;
      } catch (error) {
        const changed = failure !== error.message;
        failure = error.message;
        return changed;
      }
    }

    // readSources reads the files the version names; true when one changed.
    async function readSources() {
      const decls = state.sources || {};
      let changed = false;
      for (const name of Object.keys(seen)) {
        if (!decls[name]) {
          delete seen[name];
          delete values[name];
          delete infos[name];
          changed = true;
        }
      }
      await Promise.all(Object.entries(decls).map(async ([name, decl]) => {
        const format = decl.format || inferFormat(decl.file);
        // What was read of a source is marked with its file, which a newer
        // version may change.
        const failed = (mark, error) => {
          if (seen[name] === mark) return;
          seen[name] = mark;
          delete values[name];
          infos[name] = { path: decl.file, format, error };
          changed = true;
        };
        let r;
        try {
          r = await readFile(ws, decl.file);
        } catch (error) {
          // Off line, it is read again the next time; the server's refusal shows.
          if (error?.status) failed(`${decl.file}\n!${error.status}`, error.message);
          return;
        }
        if (r.missing) return failed(`${decl.file}\n!missing`, 'no such file');
        const mark = `${decl.file}\n${r.version}`;
        if (mark === seen[name]) return;
        if (seen[name] !== undefined) pulse = true; // changed while shown, not read the first time
        seen[name] = mark;
        const info = { path: decl.file, format, size: r.size, mtime: r.modified };
        if (r.too_large) info.error = 'bigger than 8 MB';
        else if (typeof r.text !== 'string') info.error = 'not text';
        else {
          try {
            values[name] = parseSource(format, r.text);
          } catch (error) {
            info.error = `not ${format}: ${error.message}`;
          }
        }
        if (info.error) delete values[name];
        infos[name] = info;
        changed = true;
        if (r.modified && time(r.modified) > time(changedAt)) changedAt = r.modified;
      }));
      return changed;
    }

    async function poll() {
      if (busy || card.disposed) return;
      const drawn = card.node.isConnected;
      if (!drawn && !card.keep) {
        // Out of the transcript (scrolled far, or another session):
        // reading stops until it is drawn again — unless the chip stands
        // for it (keep), which says what it says as its files change.
        if (++misses > 2) sleep();
        return;
      }
      misses = 0;
      if (document.visibilityState === 'hidden') return;
      busy = true;
      let changed = false;
      try {
        if (follow || !state) changed = (await load()) || changed;
        if (state) {
          pulse = false;
          if (await readSources()) changed = true;
          if (pulse && state) {
            status.classList.remove('gu-fresh');
            void status.offsetWidth; // the pulse starts again
            status.classList.add('gu-fresh');
            if (card === chip.tracked()) chip.pulse();
          }
          if (drawn) frames.check();
        }
      } catch (error) {
        console.warn('genui:', error);
      } finally {
        busy = false;
      }
      if (changed) draw();
    }

    function wake() {
      if (card.disposed || timer) return;
      misses = 0;
      timer = setInterval(poll, POLL);
      setTimeout(poll, 0);
    }

    function sleep() {
      if (timer) clearInterval(timer);
      timer = 0;
    }

    // doc is the data model now: what the agent sent, what the user
    // changed, and the files as last read (or as the call saw them, until
    // they are read).
    function docOf() {
      const data = structuredClone(state.data || {});
      for (const [ptr, value] of Object.entries(kept.edits[card.path] || {})) set(data, ptr, value);
      const sources = {};
      const sourceInfo = {};
      const snap = state.snapshot || {};
      for (const name of Object.keys(state.sources || {})) {
        if (seen[name] !== undefined) {
          if (Object.hasOwn(values, name)) sources[name] = values[name];
          if (infos[name]) sourceInfo[name] = infos[name];
        } else {
          if (snap.sources?.[name] != null) sources[name] = snap.sources[name];
          if (snap.sourceInfo?.[name]) sourceInfo[name] = snap.sourceInfo[name];
        }
      }
      return { ...data, sources, sourceInfo };
    }

    // say puts what the header says of the files: tone is live, reading,
    // warn, err or static (a surface that reads no file).
    function say(tone, text, tip = '') {
      status.dataset.tone = tone;
      status.replaceChildren(h('span', { text }));
      status.title = tip;
    }

    function draw() {
      if (card.disposed) return;
      if (!state) {
        card.node.dataset.state = failure ? 'error' : 'loading';
        title.textContent = card.title || follow?.surface || call?.surface || 'Surface';
        say(failure ? 'err' : 'reading', failure ? 'unavailable' : 'reading', failure);
        put(body, h('div', { class: 'gu-none', text: failure || 'Reading the surface…' }),
          failure && call?.text ? h('pre', { class: 'gu-fallback', text: call.text }) : null);
        frames.clear();
        foot.replaceChildren();
        return;
      }
      const doc = docOf();
      const comps = new Map((state.components || []).filter((c) => c && typeof c.id === 'string').map((c) => [c.id, c]));
      card.title = state.title || state.surface;
      card.glance = state.glance ? fill(doc, comps, state.glance) : '';
      title.textContent = card.title;
      card.onTitle?.(card.title, `${state.surface} v${state.version}`);

      const focus = keepFocus(body);
      frames.begin();
      frames.place(body, renderSurface({
        h, doc, comps, live: true, ui, redraw: draw,
        edit: (ptr, value, again) => edit(ptr, value, again),
        act: (c) => act(c, doc, comps),
        used: (id) => !!kept.used[`${card.path}#${id}`],
        openFile, rawUrl: (path) => rawUrl(ws, path), markdown,
        code: (c) => codeOf(ws, c),
        diffs: state.snapshot?.diffs || {},
        frame: (c) => frames.frame(c, doc),
      }));
      focus?.();

      // What the header says: live, or what it waits for.
      const waiting = Object.entries(state.sources || {}).filter(([name]) => doc.sourceInfo[name]?.error === 'no such file').map(([, d]) => d.file);
      const broken = Object.entries(state.sources || {}).filter(([name]) => doc.sourceInfo[name]?.error && doc.sourceInfo[name].error !== 'no such file');
      const reading = Object.keys(state.sources || {}).some((name) => seen[name] === undefined);
      card.node.dataset.state = broken.length ? 'error' : waiting.length ? 'waiting' : 'live';
      const at = time(changedAt) > time(state.at) ? changedAt : state.at;
      const reads = Object.keys(state.sources || {}).length > 0;
      const tip = reads ? `Reads its files every ${POLL / 1000} s while it is in view. Last change ${fmt.stamp(at)}.` : `Made ${fmt.stamp(state.at)}; it reads no file.`;
      if (broken.length) say('err', `${broken[0][1].file}: ${doc.sourceInfo[broken[0][0]].error}`, tip);
      else if (waiting.length) say('warn', `waiting for ${waiting[0]}${waiting.length > 1 ? ` +${waiting.length - 1}` : ''}`, tip);
      else if (!reads) say('static', `v${state.version} · ${fmt.short(state.at)}`, tip);
      else say(reading ? 'reading' : 'live', `live · ${fmt.short(at)}`, tip);

      const sources = Object.entries(state.sources || {}).map(([name, decl]) => {
        const info = doc.sourceInfo[name] || {};
        const value = doc.sources[name];
        const bits = [decl.file];
        if (info.error) bits.push(info.error === 'no such file' ? 'no file yet' : info.error);
        else if (value && typeof value === 'object' && Number.isInteger(value.count)) bits.push(`${num(value.count)} ${value.count === 1 ? 'row' : 'rows'}`);
        else if (Number.isFinite(info.size)) bits.push(fmt.bytes(info.size));
        return h('button', { class: 'gu-source', type: 'button', title: `Open ${decl.file}`, data: { error: info.error ? 'true' : null }, onclick: () => openFile(decl.file) },
          h('b', { text: name }), ` ${bits.join(' · ')}`);
      });
      const warnings = call?.warnings?.length && call.version === state.version ? call.warnings : [];
      put(foot,
        h('span', { class: 'gu-sources' }, sources.length ? sources : h('span', { class: 'gu-dim', text: 'no files' })),
        warnings.length ? h('span', { class: 'gu-warnings', title: warnings.join('\n'), text: `⚠ ${warnings.length}` }) : null,
        h('span', { class: 'gu-version', title: card.path, text: `${state.surface} v${state.version}` }));
      if (card === chip.tracked()) chip.update(card);
    }

    function edit(ptr, value, again) {
      if (!ptr || !card.path) return;
      (kept.edits[card.path] ||= {})[ptr] = value;
      touch(card.path);
      if (again) {
        draw();
        redrawOthers(card.path, card);
      } else if (state) {
        frames.refresh(docOf()); // the pages show it as it is typed
      }
    }

    async function act(c, doc, comps) {
      const action = c.action && typeof c.action === 'object' ? c.action : {};
      if (action.set && typeof action.set === 'object') {
        const edits = (kept.edits[card.path] ||= {});
        for (const [ptr, value] of Object.entries(action.set)) {
          const v = resolve(doc, value);
          edits[ptr] = v === MISSING ? null : v;
        }
        touch(card.path);
        draw();
        redrawOthers(card.path, card);
      } else if (typeof action.open === 'string') {
        openFile(action.open);
      } else if (action.copy != null) {
        const v = typeof action.copy === 'string' ? fill(doc, comps, action.copy) : resolve(doc, action.copy);
        cockpit.copy(typeof v === 'string' ? v : shown(v), 'Copied');
      } else if (action.event && typeof action.event.name === 'string') {
        await send(c, action.event, doc);
      }
    }

    // deliver tells the agent of the session what the user did, as a
    // message of theirs: it reads it when it is done, or at once when it is
    // idle. name names it in the toast.
    async function deliver(text, name) {
      const sessions = service('session');
      const here = sessions?.view?.();
      const v = here && here.id === session ? here : null;
      if (v?.run?.ready) await v.run.ready;
      const res = await cockpit.api(cockpit.wsPath(ws, `/sessions/${encodeURIComponent(session)}/queue`), {
        method: 'POST',
        body: { text, model: (v && sessions.nextModel?.(v)) || undefined },
      });
      if (v && v === sessions.view?.()) {
        if (res.queue) sessions.setQueue?.(v, res.queue);
        if (res.run) sessions.followQueued?.(v, res.run);
      }
      if (res.note) cockpit.toast(res.note, 'warn', 'genui');
      else cockpit.toast(res.run ? `Sent to the agent: ${name}` : `Queued for the agent: ${name}`, 'info', 'genui');
    }

    // send sends a button's event, with its bindings read now.
    async function send(c, event, doc) {
      const data = {};
      if (event.data && typeof event.data === 'object') {
        for (const [key, value] of Object.entries(event.data)) {
          const v = resolve(doc, value);
          data[key] = v === MISSING ? null : v;
        }
      }
      const text = `[UI ${state.surface} v${state.version}] ${event.name}${Object.keys(data).length ? ` ${JSON.stringify(data)}` : ''}`;
      const button = card.node.querySelector(`[data-gu-id="${CSS.escape(c.id)}"]`);
      if (button) button.disabled = true;
      try {
        await deliver(text, event.name);
        if (c.once) {
          kept.used[`${card.path}#${c.id}`] = true;
          touch(card.path);
        }
      } catch (error) {
        cockpit.toast(`Not sent: ${error.message}`, 'error', 'genui');
      }
      draw();
      redrawOthers(card.path, card);
    }

    // sendFrom sends an event of an Html's page, with the data it gave.
    async function sendFrom(name, data) {
      if (!state) return;
      const empty = data == null || (typeof data === 'object' && !Array.isArray(data) && !Object.keys(data).length);
      try {
        await deliver(`[UI ${state.surface} v${state.version}] ${name}${empty ? '' : ` ${JSON.stringify(data)}`}`, name);
      } catch (error) {
        cockpit.toast(`Not sent: ${error.message}`, 'error', 'genui');
      }
    }

    function menu() {
      const items = [
        { label: 'Copy as text', detail: 'The surface as the agent read it', run: () => cockpit.copy(state?.text || call?.text || '', 'Copied') },
        state ? { label: 'Copy as JSON', run: () => cockpit.copy(JSON.stringify(state, null, 2), 'Copied') } : null,
        card.path ? { label: 'Open the version file', detail: card.path, run: () => openFile(card.path) } : null,
        { separator: true },
        place !== 'overlay' ? { label: 'Open large', run: () => overlay.open(spec()) } : null,
        place !== 'tab' ? { label: 'Pin to the sidebar', detail: 'It follows the newest version', run: () => pin() } : null,
        Object.keys(kept.edits[card.path] || {}).length ? {
          label: 'Reset the inputs', run: () => {
            delete kept.edits[card.path];
            touch(card.path);
            draw();
            redrawOthers(card.path, card);
          },
        } : null,
      ].filter(Boolean);
      service('menu')?.open?.(more, items);
    }

    function pin() {
      const sidebar = service('sidebar');
      const surface = state?.surface || call?.surface || follow?.surface;
      if (!sidebar?.open || !surface) return;
      const folder = follow?.folder || folderOf(card.path);
      // One tab per surface: pinned again, the tab there is shows.
      const id = `genui:${folder}/${surface}`;
      if (sidebar.tabs?.().some((t) => t.id === id)) sidebar.activate(id);
      else sidebar.open('genui', { state: { folder, surface, session, title: card.title }, title: card.title || surface, id });
    }

    card.wake = wake;
    card.sleep = sleep;
    card.redraw = draw;
    card.spec = spec;
    card.changed = () => changedAt; // the newest change of its files, as read
    card.dispose = () => {
      card.disposed = true;
      sleep();
      cardsOf.get(card.path)?.delete(card);
      if (soon) cancelAnimationFrame(soon);
      frames.clear();
      later.delete(card);
    };

    draw();
    if (!follow) load().then(draw);
    return card;
  }

  // keepFocus keeps the input typed in as the card is drawn again.
  function keepFocus(root) {
    const active = document.activeElement;
    const key = active?.dataset?.guKey;
    if (!key || !root.contains(active)) return null;
    const start = active.selectionStart;
    const end = active.selectionEnd;
    return () => {
      const again = root.querySelector(`[data-gu-key="${CSS.escape(key)}"]`);
      if (!again) return;
      again.focus({ preventScroll: true });
      try {
        if (start != null) again.setSelectionRange(start, end);
      } catch { /* not a text field */ }
    };
  }

  // ---------------------------------------------------------------- the transcript

  const cards = new Map(); // entry id → its card
  // cardFor is the card of a surface's newest call: the transcript draws
  // it, and the chip reads it while it is not drawn.
  function cardFor(v, id, call) {
    let card = cards.get(id);
    if (!card || card.path !== call.path || card.disposed) {
      card?.dispose();
      card = createCard({ ws: v.ws, session: v.id, call, entryId: id, place: 'inline' });
      cards.set(id, card);
    }
    return card;
  }
  cockpit.contribute('timeline.renderer', {
    kind: 'tool',
    order: 10,
    render(entry, ctx) {
      const v = ctx.view;
      const call = callFor(entry);
      if (!call || call.deleted || !v) return null;
      const heads = headsOf(v);
      if (heads.head.get(call.surface) !== entry.id) return null;
      const card = cardFor(v, entry.id, call);
      const li = h('li', { class: 'entry gu-entry', id: `entry-${entry.id}`, data: { kind: entry.kind, id: entry.id } },
        h('div', { class: 'gutter' }, h('span', { class: 'tick', text: fmt.short(entry.at) })),
        h('div', { class: 'body' }, card.node));
      card.wake();
      return li;
    },
  });

  cockpit.on('session:view', (v, { reload } = {}) => {
    dirty = true;
    for (const [id, card] of cards) {
      if (card.spec().session !== v?.id) {
        card.dispose();
        cards.delete(id);
      }
    }
    // The same session read again keeps its chip where it is.
    if (reload) chip.soon();
    else chip.reset();
    overlay.hide();
  });
  cockpit.on('layout:view', () => {
    overlay.hide();
    chip.soon();
  });

  // The calls before a surface's newest: a row, which opens to the
  // version's snapshot — the files as the call read them.
  cockpit.tools.register('UI', {
    summary(entry) {
      const tool = entry.tool || {};
      const call = callOf(tool.output);
      if (!call) {
        const failed = /^UI ([a-z0-9][a-z0-9_-]*|\?): nothing shown/.exec(tool.output || '');
        if (failed) return `◇ ${failed[1]} · nothing shown: ${(tool.output.match(/^- /gm) || []).length} to fix`;
        const named = /"surface"\s*:\s*"([a-z0-9][a-z0-9_-]*)"/.exec(tool.input || '');
        return named ? `◇ ${named[1]}` : null;
      }
      if (call.deleted) return `◇ ${call.surface} · removed (v${call.version})`;
      // What became of the surface, when the heads are those of the session on screen.
      const newest = memo && memo.v === service('session')?.view?.() ? memo.calls.get(call.surface) : null;
      const after = !newest || newest.version === call.version ? '' : newest.deleted ? ' → removed' : ` → v${newest.version}`;
      return `◇ ${call.title || call.surface} · v${call.version}${after} · snapshot`;
    },
    render(entry) {
      const call = callOf(entry.tool?.output);
      return call && !call.deleted ? snapshot(call) : null;
    },
  });

  function snapshot(call) {
    const ws = currentWs();
    const box = h('div', { class: 'gu-snapshot' }, h('div', { class: 'gu-none', text: 'Reading the snapshot…' }));
    loadVersion(ws, call.path).then((state) => {
      const ui = { tabs: new Map(), open: new Map() };
      const comps = new Map((state.components || []).filter((c) => c && typeof c.id === 'string').map((c) => [c.id, c]));
      const sources = Object.fromEntries(Object.entries(state.snapshot?.sources || {}).filter(([, v]) => v != null));
      const doc = { ...(state.data || {}), sources, sourceInfo: state.snapshot?.sourceInfo || {} };
      // Its pages get the files as the call read them; an Html of a file
      // shows the file as it is now.
      const frames = framesOf({
        ws, path: () => call.path, live: false,
        files: () => Object.values(state.sources || {}).map((d) => d?.file).filter(Boolean),
        edit() {}, event() {},
      });
      const body = h('div', { class: 'gu-snapshot-body' });
      box.replaceChildren(
        h('div', { class: 'gu-snapshot-head' },
          h('span', { class: 'gu-dim', text: `Snapshot of v${state.version}: the files as of ${fmt.stamp(state.at)}. Inputs and buttons work in the newest version.` })),
        body);
      const draw = () => {
        frames.begin();
        frames.place(body, renderSurface({
          h, doc, comps, live: false, ui, redraw: draw, edit() {}, act() {}, used: () => false,
          openFile, rawUrl: (path) => rawUrl(ws, path), markdown, code: (c) => codeOf(ws, c), diffs: state.snapshot?.diffs || {},
          frame: (c) => frames.frame(c, doc),
        }));
      };
      draw();
    }, (error) => put(box, h('div', { class: 'gu-none', text: error.message }), call.text ? h('pre', { class: 'gu-fallback', text: call.text }) : null));
    return box;
  }

  // ---------------------------------------------------------------- the chip

  // The chip stands for the newest surface of the session in view while its
  // card has scrolled away above: at the top of the transcript, where the
  // card went, it says what the surface says — its title and glance, live —
  // and a click brings the card back. Below the view, the transcript's jump
  // button over the composer has the bottom to itself.
  //
  // It shows while the surface is the current exchange's: called after the
  // user's last message (the messages a surface's own buttons send are not
  // one), or with a file changed since — a benchmark run again shows on it.
  // Where the card is comes from the transcript's order, not from whether
  // the card is drawn: far away, the timeline does not draw it.
  const EVENT = /^\[UI [a-z0-9][a-z0-9_-]* v\d+\]/;
  const chip = (() => {
    const title = h('span', { class: 'gu-chip-title' });
    const glance = h('span', { class: 'gu-chip-glance' });
    const node = h('button', { class: 'gu-chip', type: 'button', hidden: true, title: 'A live surface, above: show it',
      onclick: () => { if (current) service('timeline')?.reveal?.(current.entryId); } },
    h('span', { class: 'gu-chip-mark', 'aria-hidden': 'true', text: '◇' }), title, glance,
    h('span', { class: 'gu-chip-up', 'aria-hidden': 'true', text: '↑' }));
    cockpit.ui.mount('stage.overlay', { id: 'genui-chip', order: 10, node });
    let current = null; // the card of the newest surface of the session in view
    let frame = 0;

    // soon looks again in the next frame: the transcript scrolled, was
    // drawn, or the card changed.
    function soon() {
      frame ||= requestAnimationFrame(check);
    }

    function check() {
      frame = 0;
      const v = service('session')?.view?.();
      const id = v ? headsOf(v).newest : null;
      const call = id ? callFor(v.entries.get(id)) : null;
      const card = call && !call.deleted ? cardFor(v, id, call) : null;
      if (current !== card) {
        if (current) current.keep = false;
        current = card;
        if (card) {
          card.keep = true; // it reads its files while it is not drawn
          card.wake();
        }
      }
      const show = !!card && above(v, id) && ofNow(v, id, card);
      if (show) {
        const text = card.title || call.title || call.surface;
        if (title.textContent !== text) title.textContent = text;
        if (glance.textContent !== (card.glance || '')) glance.textContent = card.glance || '';
        glance.hidden = !card.glance;
      }
      if (node.hidden === show) {
        node.hidden = !show;
        // Shown again, it comes down from the bar, not ringing as before.
        if (!show) node.classList.remove('gu-chip-fresh');
      }
    }

    // above is whether an entry is above the view: drawn, by where it is;
    // not drawn, by whether what is in view comes after it.
    function above(v, id) {
      const timeline = service('timeline');
      const el = timeline?.scroller?.();
      const list = document.getElementById('entries');
      if (!el || !el.clientHeight || !list) return false;
      const top = el.getBoundingClientRect().top;
      const drawn = timeline.node?.(id);
      if (drawn?.isConnected) return drawn.getBoundingClientRect().bottom <= top + 1;
      for (const item of list.children) {
        const at = item.dataset.id;
        if (!at || item.getBoundingClientRect().bottom <= top) continue; // a gap, or above the view
        return v.order.indexOf(id) < v.order.indexOf(at);
      }
      return false;
    }

    // ofNow is whether the surface is the current exchange's.
    function ofNow(v, id, card) {
      const at = v.order.indexOf(id);
      for (let i = v.order.length - 1; i > at; i--) {
        const entry = v.entries.get(v.order[i]);
        if (entry?.kind !== 'user' || EVENT.test(entry.text || '')) continue;
        return time(card.changed()) > time(entry.at);
      }
      return at >= 0;
    }

    // pulse rings the chip once as a file of its surface changes.
    function pulse() {
      if (node.hidden) return;
      node.classList.remove('gu-chip-fresh');
      void node.offsetWidth;
      node.classList.add('gu-chip-fresh');
    }

    function reset() {
      if (current) current.keep = false;
      current = null;
      node.hidden = true;
      node.classList.remove('gu-chip-fresh');
      soon();
    }

    cockpit.listen(node, 'animationend', (event) => {
      if (event.animationName === 'gu-chip-fresh') node.classList.remove('gu-chip-fresh');
    });
    cockpit.on('timeline:flush', soon);
    cockpit.on('timeline:scroll', soon);
    cockpit.listen(window, 'resize', soon);
    cockpit.onDispose(() => cancelAnimationFrame(frame));
    soon();
    return { soon, update: soon, reset, pulse, tracked: () => current };
  })();

  // ---------------------------------------------------------------- open large

  const overlay = (() => {
    const frame = h('div', { class: 'gu-overlay-frame' });
    const node = h('div', { class: 'gu-overlay', hidden: true, onclick: (event) => { if (event.target === node) hide(); } },
      h('div', { class: 'gu-overlay-box' }, frame));
    cockpit.ui.mount('stage.overlay', { id: 'genui-overlay', order: 50, node });
    let card = null;

    function open(spec) {
      hide();
      card = createCard({ ...spec, place: 'overlay' });
      frame.replaceChildren(card.node);
      node.hidden = false;
      card.wake();
      card.node.querySelector('.gu-close')?.focus({ preventScroll: true });
    }

    function hide() {
      card?.dispose();
      card = null;
      frame.replaceChildren();
      node.hidden = true;
    }

    cockpit.listen(document, 'keydown', (event) => {
      if (!card || event.key !== 'Escape' || event.isComposing) return;
      event.preventDefault();
      event.stopPropagation(); // not the first Esc of an Esc Esc
      hide();
    }, true);
    return { open, hide };
  })();

  // ---------------------------------------------------------------- the sidebar

  cockpit.contribute('sidebar.tab', {
    id: 'genui',
    title: 'Surface',
    icon: ICON,
    order: 80,
    multiple: true,
    hello: false,
    description: 'A surface the agent made, pinned: it follows the newest version, live as its files change.',
    create(tab) {
      const s = tab.state || {};
      if (!s.folder || !s.surface) {
        return { node: h('div', { class: 'gu-pane' }, h('div', { class: 'gu-none', text: 'Pin a surface with ⇥ on its card.' })) };
      }
      const card = createCard({ ws: currentWs(), session: s.session, follow: { folder: s.folder, surface: s.surface, title: s.title }, place: 'tab' });
      card.onTitle = (text, detail) => tab.setTitle?.(text, detail);
      card.wake();
      return {
        node: h('div', { class: 'gu-pane' }, card.node),
        shown: () => card.wake(),
        hidden: () => card.sleep(),
        dispose: () => card.dispose(),
      };
    },
  });

  cockpit.onDispose(() => {
    for (const card of cards.values()) card.dispose();
    cards.clear();
    overlay.hide();
  });
}
