// A terminal tab: restty's surface of panes (vendor/restty.esm.js), split
// with ⌘D and ⌘⇧D, each pane a shell of the server's (terminals.go) that it
// attaches to over a WebSocket. The pane is only a view of its shell: a
// reload, the plugin loaded anew or the server's own restart leaves the
// shell running, and the pane attaches to it again and draws the screen
// from the output the server kept. The tab saves its layout — the splits,
// their sizes and each pane's shell — to come back to.
import { ghostty, colours } from './theme.js';

const encoder = new TextEncoder();
const SPLIT_RIGHT = '<svg viewBox="0 0 16 16" aria-hidden="true"><rect x="1.5" y="2.5" width="13" height="11" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M8 2.5v11" stroke="currentColor" stroke-width="1.4"/></svg>';
const SPLIT_DOWN = '<svg viewBox="0 0 16 16" aria-hidden="true"><rect x="1.5" y="2.5" width="13" height="11" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M1.5 8h13" stroke="currentColor" stroke-width="1.4"/></svg>';
const MORE = '<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="3.5" cy="8" r="1.2" fill="currentColor"/><circle cx="8" cy="8" r="1.2" fill="currentColor"/><circle cx="12.5" cy="8" r="1.2" fill="currentColor"/></svg>';

// ratioOf reads the share of its split a pane's element takes: restty
// sizes the two sides of a split as flex bases in percent.
function ratioOf(el) {
  const basis = /(\d+(?:\.\d+)?)%/.exec(el?.style?.flex || '');
  const value = basis ? Number(basis[1]) / 100 : 0.5;
  return Math.min(0.9, Math.max(0.1, value));
}

const home = (path, root) => (root && path?.startsWith(root) ? `~${path.slice(root.length)}` : path || '');

// createTerminal makes the view of a terminal tab. env gives what the
// plugin shares between its tabs: restty, the fonts, the settings.
export function createTerminal(env, tab) {
  const { cockpit } = env;
  const { h, svg } = cockpit;
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);

  const where = h('span', { class: 'term-where', title: 'Where the shell is' });
  const running = h('span', { class: 'term-running', hidden: true });
  const status = h('div', { class: 'term-status', hidden: true, role: 'status' });
  const banner = h('div', { class: 'term-banner', hidden: true, role: 'alert' });
  const surface = h('div', { class: 'term-surface' });
  const bar = h('div', { class: 'term-bar' },
    h('span', { class: 'term-mark', 'aria-hidden': 'true' }),
    where, running,
    h('span', { class: 'term-fill' }),
    h('button', { class: 'icon small', type: 'button', title: `Split right (${env.keys.split})`, 'aria-label': 'Split right', onclick: () => split('vertical') }, svg(SPLIT_RIGHT)),
    h('button', { class: 'icon small', type: 'button', title: `Split down (${env.keys.splitDown})`, 'aria-label': 'Split down', onclick: () => split('horizontal') }, svg(SPLIT_DOWN)),
    h('button', { class: 'icon small', type: 'button', title: 'More', 'aria-label': 'More', 'aria-haspopup': 'menu', onclick: (event) => menu(event.currentTarget) }, svg(MORE)));
  // Keys pressed in the terminal are the terminal's: the cockpit's own
  // bindings do not run there (the kernel reads data-keys).
  const root = h('div', { class: 'term', data: { keys: 'own' } }, bar, banner, surface, status);
  const anchor = h('span', { class: 'term-anchor', 'aria-hidden': 'true' });
  root.append(anchor);

  let restty = null;
  let starting = null;
  // A size the tab was zoomed to (⌘+ ⌘−), else the settings' size.
  let zoomed = Number.isFinite(tab.state?.fontSize) ? tab.state.fontSize : 0;
  const fontSize = () => zoomed || env.settings().fontSize;
  let disposed = false;
  let visible = false;
  let closing = false;
  const panes = new Map(); // restty pane ID → pane state

  // ---------------------------------------------------------------- layout

  // The layout saved: { t: "pane", id, cwd } or { t: "split", dir, ratio, a, b }.
  function serialize(el) {
    if (!el) return null;
    if (el.classList.contains('pane')) {
      const state = panes.get(Number(el.dataset.paneId));
      return { t: 'pane', id: state?.terminal || null, cwd: state?.cwd || null };
    }
    if (el.classList.contains('pane-split')) {
      const children = [...el.children].filter((child) => !child.classList.contains('pane-divider'));
      if (children.length !== 2) return serialize(children[0]);
      return {
        t: 'split', dir: el.classList.contains('is-vertical') ? 'vertical' : 'horizontal',
        ratio: ratioOf(children[0]), a: serialize(children[0]), b: serialize(children[1]),
      };
    }
    return null;
  }

  let saving = 0;
  function saveLayout() {
    if (!restty || disposed) return;
    clearTimeout(saving);
    saving = setTimeout(() => {
      if (!restty || disposed) return;
      const layout = serialize(surface.firstElementChild);
      if (layout) tab.save(zoomed ? { layout, fontSize: zoomed } : { layout });
    }, 120);
  }

  // restore builds a saved layout from a pane: it splits as the layout did,
  // then gives each pane its shell.
  function restore(node, pane, assigned) {
    if (!node || node.t === 'pane') {
      const state = panes.get(pane.id);
      if (state) {
        state.terminal = node?.id || null;
        state.cwd = node?.cwd || null;
      }
      assigned.push(pane);
      return;
    }
    const other = restty.splitPane(pane.id, node.dir === 'horizontal' ? 'horizontal' : 'vertical');
    if (!other) {
      restore(node.a, pane, assigned);
      return;
    }
    const ratio = Math.min(0.9, Math.max(0.1, Number(node.ratio) || 0.5));
    pane.container.style.flex = `0 0 ${(ratio * 100).toFixed(3)}%`;
    other.container.style.flex = `0 0 ${((1 - ratio) * 100).toFixed(3)}%`;
    restore(node.a, pane, assigned);
    restore(node.b, other, assigned);
  }

  // ---------------------------------------------------------------- shells

  // transport connects a pane to its shell: it starts one when the pane has
  // none, attaches over a WebSocket and attaches again when the socket
  // drops, until the shell ends or the pane goes.
  function transport(state) {
    let socket = null;
    let callbacks = null;
    let live = false;
    let stopped = true;
    let muted = false;
    let attempts = 0;
    let timer = 0;
    let sizeTimer = 0;
    let size = null;
    let decoder = new TextDecoder();

    const t = {
      connect({ callbacks: given, cols, rows }) {
        callbacks = given;
        stopped = false;
        state.cols = cols || state.cols || 80;
        state.rows = rows || state.rows || 24;
        attach();
      },
      disconnect() {
        stopped = true;
        live = false;
        clearTimeout(timer);
        clearTimeout(sizeTimer);
        const s = socket;
        socket = null;
        s?.close();
      },
      sendInput(data) {
        if (muted || !live || socket?.readyState !== WebSocket.OPEN) return false;
        socket.send(encoder.encode(data));
        return true;
      },
      resize(cols, rows, meta) {
        if (!(cols > 1 && rows > 0)) return false;
        state.cols = cols;
        state.rows = rows;
        size = { type: 'resize', cols, rows, width: Math.round(meta?.widthPx || 0), height: Math.round(meta?.heightPx || 0) };
        clearTimeout(sizeTimer);
        // A drag resizes many times; the shell hears the last.
        sizeTimer = setTimeout(sendSize, 50);
        return true;
      },
      isConnected: () => live,
      destroy() { t.disconnect(); },
      write(bytes) {
        if (socket?.readyState !== WebSocket.OPEN) return false;
        socket.send(bytes);
        return true;
      },
    };
    state.transport = t;

    function sendSize() {
      if (size && socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify(size));
    }

    function retry(error) {
      if (stopped || disposed || state.exited) return;
      live = false;
      attempts++;
      const wait = Math.min(5000, 150 * 2 ** Math.min(attempts, 6));
      if (attempts > 1) say(error ? `Reconnecting… ${error}` : 'Reconnecting to the shell…', 'wait');
      clearTimeout(timer);
      timer = setTimeout(attach, wait);
    }

    async function attach() {
      if (stopped || disposed) return;
      try {
        if (state.terminal) {
          // A shell that ended while no page showed it is gone.
          try {
            await cockpit.api(`/api/terminals/${encodeURIComponent(state.terminal)}`);
          } catch (error) {
            if (error.status !== 404) throw error;
            state.terminal = null;
            state.renewed = true;
          }
        }
        if (!state.terminal) {
          const body = { cols: state.cols || 80, rows: state.rows || 24 };
          if (state.from) body.from = state.from;
          else if (state.cwd) body.cwd = state.cwd;
          let info;
          try {
            info = await cockpit.api('/terminals', { method: 'POST', body });
          } catch (error) {
            // A folder that went: the workspace's instead.
            if (error.status !== 400 || !body.cwd) throw error;
            delete body.cwd;
            info = await cockpit.api('/terminals', { method: 'POST', body });
          }
          if (stopped || disposed) {
            cockpit.api(`/api/terminals/${encodeURIComponent(info.id)}`, { method: 'DELETE' }).catch(() => {});
            return;
          }
          state.terminal = info.id;
          state.from = null;
          state.shell = info.shell;
          state.cwd = info.cwd || state.cwd;
          saveLayout();
        }
      } catch (error) {
        retry(error.message);
        return;
      }
      if (stopped || disposed) return;
      const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
      const s = new WebSocket(`${scheme}://${location.host}/api/terminals/${encodeURIComponent(state.terminal)}/socket`);
      s.binaryType = 'arraybuffer';
      socket = s;
      s.addEventListener('message', (event) => {
        if (socket !== s) return;
        if (typeof event.data === 'string') {
          let message;
          try {
            message = JSON.parse(event.data);
          } catch {
            return;
          }
          control(message);
          return;
        }
        const text = decoder.decode(new Uint8Array(event.data), { stream: true });
        if (text) callbacks?.onData?.(text);
      });
      s.addEventListener('close', () => {
        if (socket !== s) return;
        socket = null;
        live = false;
        retry('');
      });
    }

    function control(message) {
      switch (message.type) {
        case 'hello': {
          // What the server kept draws the screen, on a clean terminal; the
          // answers the terminal gives to what it replays are not sent: the
          // shell asked them long ago.
          muted = true;
          decoder = new TextDecoder();
          callbacks?.onData?.('\x1bc\x1b[3J');
          if (state.renewed) {
            state.renewed = false;
            callbacks?.onData?.('\x1b[2m[the shell ended while no page showed it: this is a new one]\x1b[0m\r\n');
          }
          const info = message.terminal || {};
          state.shell = info.shell || state.shell;
          state.title = info.title || state.title;
          state.cwd = info.cwd || state.cwd;
          state.theme = !!info.theme;
          news();
          break;
        }
        case 'live':
          muted = false;
          live = true;
          attempts = 0;
          say('');
          callbacks?.onConnect?.();
          sendSize();
          break;
        case 'meta':
          state.title = message.title || '';
          if (message.cwd) state.cwd = message.cwd;
          news();
          saveLayout();
          break;
        case 'exit':
          state.exited = true;
          state.code = message.code;
          live = false;
          ended(state);
          break;
      }
    }
    return t;
  }

  // ---------------------------------------------------------------- restty

  function paneState(id) {
    let state = panes.get(id);
    if (!state) {
      state = { pane: id, terminal: null, from: null, cwd: null, title: '', shell: '', exited: false, code: 0, cols: 0, rows: 0 };
      panes.set(id, state);
    }
    return state;
  }

  async function start() {
    const lib = await env.restty();
    if (disposed) return;
    const c = colours();
    const theme = lib.parseGhosttyTheme(ghostty());
    env.markStyles();
    restty = new lib.Restty({
      root: surface,
      surface: {
        createInitialPane: false,
        shortcuts: false,
        searchUi: true,
        defaultContextMenu: false,
        contextMenu: null,
        minPaneSize: 80,
        paneStyles: {
          splitBackground: c.line, paneBackground: c.background, inactivePaneOpacity: 0.66, activePaneOpacity: 1,
          opacityTransitionMs: 120, dividerColor: c.line, dividerThicknessPx: 1,
        },
        events: {
          onPaneClosed: (pane) => { panes.delete(pane.id); news(); saveLayout(); },
          onActivePaneChange: () => news(),
          onLayoutChanged: () => { syncSize(); saveLayout(); },
        },
      },
      terminal: () => ({
        renderer: 'auto', fontSize: fontSize(), fonts: env.fonts(), theme, ligatures: false,
        autoResize: false, showResizeOverlay: true, maxScrollbackBytes: 8_000_000,
      }),
      services: ({ id }) => ({ ptyTransport: transport(paneState(id)) }),
    });
    const first = restty.createInitialPane({ focus: visible });
    const assigned = [];
    restore(tab.state?.layout || null, first, assigned);
    syncSize(true);
    for (const pane of assigned) restty.pane(pane.id)?.connectPty('kou-conveyor');
    if (visible) focus();
    news();
  }

  // syncSize has the panes take the room they have, while the tab shows;
  // a tab that does not show keeps its size, and its shells theirs.
  let sizing = 0;
  function syncSize(force = false) {
    if (!restty || !visible) return;
    cancelAnimationFrame(sizing);
    sizing = requestAnimationFrame(() => {
      if (!restty || !visible) return;
      const box = surface.getBoundingClientRect();
      if (box.width < 60 || box.height < 40) return;
      for (const pane of restty.panes()) pane.updateSize(force);
    });
  }

  // ---------------------------------------------------------------- what the tab says

  // news redraws what the tab says of its shells: its title, where the
  // active pane's shell is and what runs there.
  function news() {
    if (!restty || disposed) return;
    const active = restty.activePane();
    const state = active ? panes.get(active.id) : null;
    const root = env.home();
    const place = home(state?.cwd, root);
    where.textContent = place || (state?.shell ? state.shell.split('/').pop() : '');
    where.title = state?.cwd || '';
    const count = restty.panes().length;
    const title = state?.title && state.title !== state.cwd ? home(state.title, root) : place;
    tab.setTitle(title || 'Terminal');
    tab.setBadge(count > 1 ? String(count) : '');
    refreshRunning();
  }

  // What runs in the foreground of the active pane, asked of the server
  // now and then while the tab shows.
  let runningTimer = 0;
  async function refreshRunning() {
    clearTimeout(runningTimer);
    if (!visible || !restty || disposed) return;
    const active = restty.activePane();
    const state = active ? panes.get(active.id) : null;
    if (state?.terminal) {
      try {
        const info = await cockpit.api(`/api/terminals/${encodeURIComponent(state.terminal)}`);
        running.hidden = !info.running;
        running.textContent = info.running ? `● ${info.running}` : '';
      } catch {
        running.hidden = true;
      }
    }
    runningTimer = setTimeout(refreshRunning, 2500);
  }

  function say(text, kind = '') {
    status.hidden = !text;
    status.textContent = text;
    status.dataset.kind = kind;
  }

  // ended takes a pane whose shell ended away; the last one closes the tab.
  function ended(state) {
    if (disposed) return;
    if (restty && restty.panes().length > 1) {
      restty.closePane(state.pane);
      saveLayout();
      focus();
      return;
    }
    closing = true;
    tab.close();
  }

  // ---------------------------------------------------------------- acting

  function activePane() {
    return restty?.focusedPane?.() || restty?.activePane?.() || null;
  }

  // split splits the active pane: vertical side by side, horizontal one
  // above the other; the new shell starts where the active one is.
  function split(direction) {
    if (!restty) return;
    const pane = activePane();
    if (!pane) return;
    const source = panes.get(pane.id);
    const created = restty.splitPane(pane.id, direction);
    if (!created) return cockpit.toast('No room for another pane here', 'error');
    const state = paneState(created.id);
    state.from = source?.terminal || null;
    state.cwd = source?.cwd || null;
    restty.pane(created.id)?.connectPty('kou-conveyor');
    syncSize();
    saveLayout();
    news();
  }

  function focus() {
    requestAnimationFrame(() => activePane()?.focus?.());
  }

  // move goes to the pane beside the active one, by where it is.
  function move(direction) {
    if (!restty) return;
    const current = activePane();
    if (!current) return;
    const from = current.container ? current.container.getBoundingClientRect() : restty.getPaneById(current.id)?.container.getBoundingClientRect();
    if (!from) return;
    const cx = from.left + from.width / 2;
    const cy = from.top + from.height / 2;
    let best = null;
    let bestScore = Infinity;
    for (const pane of restty.getPanes()) {
      if (pane.id === current.id) continue;
      const box = pane.container.getBoundingClientRect();
      const x = box.left + box.width / 2;
      const y = box.top + box.height / 2;
      const ok = { left: box.right <= from.left + 2, right: box.left >= from.right - 2, up: box.bottom <= from.top + 2, down: box.top >= from.bottom - 2 }[direction];
      if (!ok) continue;
      const score = Math.hypot(x - cx, y - cy);
      if (score < bestScore) {
        best = pane;
        bestScore = score;
      }
    }
    if (best) restty.setActivePane(best.id, { focus: true });
  }

  // clear clears the active pane's screen and what scrolled off it; the
  // shell draws its prompt again.
  function clear() {
    const pane = activePane();
    if (!pane) return;
    const state = panes.get(pane.id);
    restty.pane(pane.id)?.sendInput('\x1b[H\x1b[2J\x1b[3J', 'pty');
    state?.transport?.write(encoder.encode('\x0c'));
  }

  // deleteWord deletes the word before the cursor, as ^W does in a shell,
  // vim or less: what ⌘⌫ does.
  function deleteWord() {
    const pane = activePane();
    const state = pane ? panes.get(pane.id) : null;
    state?.transport?.write(encoder.encode('\x17'));
  }

  function find() {
    const pane = activePane();
    if (pane) restty.pane(pane.id)?.openSearch({ selectAll: true });
  }

  // restart ends a pane's shell and starts another where it was.
  async function restart(pane = activePane()) {
    if (!pane) return;
    const state = panes.get(pane.id);
    if (!state) return;
    const old = state.terminal;
    state.transport?.disconnect();
    state.terminal = null;
    state.exited = false;
    if (old) cockpit.api(`/api/terminals/${encodeURIComponent(old)}`, { method: 'DELETE' }).catch(() => {});
    restty.pane(pane.id)?.connectPty('kou-conveyor');
  }

  // restartAll restarts every pane's shell, where each was.
  function restartAll() {
    if (!restty) return;
    for (const pane of restty.panes()) restart(pane);
  }

  // closePane ends the active pane's shell; the pane goes when it has.
  function closePane() {
    const pane = activePane();
    const state = pane ? panes.get(pane.id) : null;
    if (!state) return;
    if (!state.terminal) return ended(state);
    cockpit.api(`/api/terminals/${encodeURIComponent(state.terminal)}`, { method: 'DELETE' }).catch((error) => cockpit.toast(error.message, 'error'));
  }

  function items() {
    const pane = activePane();
    return [
      { icon: '⧉', label: 'Copy', hint: env.keys.copy, disabled: !pane, run: () => restty.pane(pane.id)?.copySelectionToClipboard() },
      { icon: '⎘', label: 'Paste', hint: env.keys.paste, disabled: !pane, run: () => restty.pane(pane.id)?.pasteFromClipboard() },
      { separator: true },
      { icon: '▯', label: 'Split right', hint: env.keys.split, run: () => split('vertical') },
      { icon: '▭', label: 'Split down', hint: env.keys.splitDown, run: () => split('horizontal') },
      { icon: '⌕', label: 'Find', hint: env.keys.find, disabled: !pane, run: find },
      { icon: '⌫', label: 'Clear', hint: env.keys.clear, disabled: !pane, run: clear },
      { separator: true },
      { icon: '↻', label: 'Restart the shell', detail: 'Ends it and starts another where it was', disabled: !pane, run: () => restart(pane) },
      { icon: '⚙', label: 'Terminal settings…', detail: 'Prompt theme, font', run: () => service('sidebar')?.open('settings', { reuse: true }) },
      { separator: true },
      { icon: '＋', label: 'Larger text', hint: env.keys.zoomIn, run: () => zoom(1) },
      { icon: '－', label: 'Smaller text', hint: env.keys.zoomOut, run: () => zoom(-1) },
      { separator: true },
      { icon: '×', label: restty && restty.panes().length > 1 ? 'Close the pane' : 'Close the terminal', detail: `${env.keys.reopen} brings it back within ${env.grace}s`, hint: env.keys.close, disabled: !pane, run: closeActive },
      { icon: '⊘', label: 'End the shell now', danger: true, confirm: 'It ends at once — click again', disabled: !pane, run: closePane },
    ];
  }

  function menu(at) {
    const ui = service('menu');
    if (!ui) return;
    ui.toggle(at, items);
  }

  // A right click opens the menu where it was, on the pane it was on.
  surface.addEventListener('contextmenu', (event) => {
    const el = event.target instanceof Element ? event.target.closest('.pane') : null;
    if (!el || !restty) return;
    event.preventDefault();
    restty.setActivePane(Number(el.dataset.paneId), { focus: true });
    const box = root.getBoundingClientRect();
    anchor.style.setProperty('--x', `${event.clientX - box.left}px`);
    anchor.style.setProperty('--y', `${event.clientY - box.top}px`);
    service('menu')?.open(anchor, items());
  });

  // ---------------------------------------------------------------- the view

  function applyTheme() {
    if (!restty) return;
    env.restty().then((lib) => {
      if (!restty) return;
      const c = colours();
      const theme = lib.parseGhosttyTheme(ghostty());
      for (const pane of restty.panes()) pane.applyTheme(theme, 'kou-conveyor');
      restty.setPaneStyleOptions({ splitBackground: c.line, paneBackground: c.background, dividerColor: c.line });
    });
  }

  function applySettings() {
    if (!restty) return;
    for (const pane of restty.panes()) {
      pane.setFontSize(fontSize());
      pane.setFonts(env.fonts());
    }
    syncSize(true);
  }

  // zoom makes the tab's text larger (step 1) or smaller (−1), or gives it
  // the settings' size back (0); the page itself keeps its size.
  function zoom(step) {
    const next = step === 0 ? 0 : Math.min(32, Math.max(8, fontSize() + step));
    zoomed = next === env.settings().fontSize ? 0 : next;
    if (!restty) return;
    for (const pane of restty.panes()) pane.setFontSize(fontSize());
    syncSize(true);
    saveLayout();
    say(`${fontSize()} px`, 'zoom');
    clearTimeout(zoomTimer);
    zoomTimer = setTimeout(() => { if (status.dataset.kind === 'zoom') say(''); }, 900);
  }
  let zoomTimer = 0;

  // closeLater ends a shell after the grace a closed terminal has, while
  // ⌘⇧T can take it back.
  function closeLater(state) {
    state.transport?.disconnect();
    if (!state.terminal || state.exited) return;
    cockpit.api(`/api/terminals/${encodeURIComponent(state.terminal)}?after=${env.grace}`, { method: 'DELETE' }).catch(() => {});
  }

  // close closes the tab: its shells end after the grace, unless the tab
  // is opened again (⌘⇧T); a tab whose last shell ended just closes.
  async function close() {
    const layout = restty ? serialize(surface.firstElementChild) : tab.state?.layout;
    const live = [...panes.values()].filter((state) => state.terminal && !state.exited);
    const index = service('sidebar')?.tabs?.().findIndex((t) => t.id === tab.id) ?? -1;
    for (const state of panes.values()) closeLater(state);
    if (!closing && live.length) {
      env.closed({
        kind: 'tab', tab: tab.id, index: index >= 0 ? index : undefined, layout, fontSize: zoomed,
        title: where.textContent || 'Terminal', terminals: live.map((state) => state.terminal),
      });
    }
    return true;
  }

  // placementOf says where a pane is in its split, to put it back there:
  // the shell of the pane beside it, the split's direction, whether it was
  // the first side (left, top) and its share.
  function placementOf(id) {
    const el = restty?.getPaneById(id)?.container;
    const split = el?.parentElement;
    if (!split?.classList.contains('pane-split')) return null;
    const sides = [...split.children].filter((child) => !child.classList.contains('pane-divider'));
    const first = sides[0] === el;
    const other = first ? sides[1] : sides[0];
    const beside = other?.classList.contains('pane') ? other : other?.querySelector('.pane');
    return {
      sibling: beside ? panes.get(Number(beside.dataset.paneId))?.terminal || null : null,
      dir: split.classList.contains('is-vertical') ? 'vertical' : 'horizontal',
      first, ratio: ratioOf(el),
    };
  }

  // closeActive closes the active pane — the tab, when it is the last —
  // as ⌃W does.
  function closeActive() {
    const pane = activePane();
    if (!restty || !pane || restty.panes().length < 2) {
      tab.close();
      return;
    }
    const state = panes.get(pane.id);
    if (state) {
      const placement = placementOf(pane.id);
      closeLater(state);
      if (state.terminal && !state.exited) {
        env.closed({ kind: 'pane', tab: tab.id, terminals: [state.terminal], cwd: state.cwd, placement, title: home(state.cwd, env.home()) || 'Terminal' });
      }
    }
    restty.closePane(pane.id);
    saveLayout();
    news();
    focus();
  }

  // adopt shows a pane closed again, with its shell: where it was — beside
  // the pane it was beside, on its side of the split, at its share — or,
  // that pane gone, beside the active one.
  async function adopt(terminal, cwd, placement = null) {
    await (starting || Promise.resolve());
    if (!restty) return false;
    let target = null;
    if (placement?.sibling) {
      for (const [id, state] of panes) if (state.terminal === placement.sibling) target = restty.getPaneById(id);
    }
    target ??= restty.getPaneById(activePane()?.id ?? -1) || restty.getPanes()[0] || null;
    if (!target) return false;
    const dir = placement?.dir === 'horizontal' ? 'horizontal' : 'vertical';
    const created = restty.splitPane(target.id, dir);
    if (!created) return false;
    const added = paneState(created.id);
    let back = created; // the pane that shows the shell taken back
    if (placement?.first) {
      // restty adds a pane after the one it splits: the shell taken back
      // goes to the first pane, and the other's to the second.
      const kept = panes.get(target.id);
      added.terminal = kept.terminal;
      added.cwd = kept.cwd;
      kept.transport?.disconnect();
      kept.terminal = terminal;
      kept.cwd = cwd || null;
      kept.exited = false;
      restty.pane(target.id)?.connectPty('kou-conveyor');
      back = target;
    } else {
      added.terminal = terminal;
      added.cwd = cwd || null;
    }
    restty.pane(created.id)?.connectPty('kou-conveyor');
    if (placement?.ratio) {
      const share = Math.min(0.9, Math.max(0.1, placement.ratio));
      back.container.style.flex = `0 0 ${(share * 100).toFixed(3)}%`;
      (back === target ? created : target).container.style.flex = `0 0 ${((1 - share) * 100).toFixed(3)}%`;
    }
    restty.setActivePane(back.id, { focus: visible });
    syncSize();
    saveLayout();
    news();
    return true;
  }

  const view = {
    node: root,
    title: 'Terminal',
    shown() {
      visible = true;
      if (!restty && !starting) {
        say('Starting the terminal…', 'wait');
        starting = start().then(() => { if (!restty) return; say(''); }, (error) => {
          console.error(error);
          say(`The terminal did not start: ${error.message}`, 'error');
        });
      }
      if (restty) for (const pane of restty.panes()) pane.setPaused(false);
      syncSize();
      refreshRunning();
    },
    hidden() {
      visible = false;
      clearTimeout(runningTimer);
      if (restty) for (const pane of restty.panes()) pane.setPaused(true);
    },
    resized: () => syncSize(),
    focus,
    close,
    dispose() {
      disposed = true;
      clearTimeout(saving);
      clearTimeout(runningTimer);
      cancelAnimationFrame(sizing);
      const r = restty;
      restty = null;
      // The shells run on; the panes let them go.
      try {
        r?.destroy();
      } catch (error) {
        console.error(error);
      }
      env.forget(view);
    },
    // What the plugin asks of its tabs.
    split, move, clear, find, applyTheme, applySettings, restartAll, zoom, closeActive, adopt, deleteWord,
    started: () => !!restty,
    contains: (node) => root.contains(node),
    terminals: () => [...panes.values()].map((state) => state.terminal).filter(Boolean),
  };
  return view;
}
