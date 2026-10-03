// A shell of the server's shown in another plugin's element: what the
// terminal service's mount does, for the canvas's terminal nodes. It
// attaches to a shell that runs — it never starts one — and draws it with
// restty at the size of the element it is given. A tab's pane does more
// (splits, a shell of its own); this is the pane alone.
//
//   mount(container, { id, fontSize, readOnly, scale, onMeta, onExit, onFocus, onReady })
//     → { focus(), blur(), resize(), setPaused(paused), applyTheme(), applySettings(), dispose(), connected() }
//
// onMeta({ title, cwd }) says where the shell is, onExit(code) that it
// ended (code null: it was gone already), onReady() that it draws.
// scale() is how much an element around the container scales it (the
// canvas's zoom): the grid is the container's own size in cells then, not
// what its scaled box measures — the same at any zoom, so the shell is not
// made to reflow by one.
import { ghostty, colours } from './theme.js';
import { editingKeys } from './editing.js';

const encoder = new TextEncoder();

export function mountTerminal(env, container, options = {}) {
  const { cockpit } = env;
  const id = String(options.id || '');
  const surface = cockpit.h('div', { class: 'term-mount', data: { keys: 'own' } });
  container.append(surface);
  // ⌘⌫, ⌥⌫ and the like go to the shell as Ghostty sends them.
  const stopEditing = editingKeys(surface, (text) => {
    if (!live || socket?.readyState !== WebSocket.OPEN) return false;
    socket.send(encoder.encode(text));
    return true;
  });

  let restty = null;
  let disposed = false;
  let paused = false;
  let exited = false;
  let socket = null;
  let callbacks = null;
  let live = false;
  let wantFocus = false; // focus was asked before the terminal was up
  let stopped = true;
  let muted = false;
  let attempts = 0;
  let timer = 0;
  let sizeTimer = 0;
  let size = null;
  let decoder = new TextDecoder();
  const fontSize = () => options.fontSize || env.settings().fontSize;
  const scale = () => {
    const s = Number(options.scale?.() ?? 1);
    return Number.isFinite(s) && s > 0 ? s : 1;
  };
  const unscaled = () => Math.abs(scale() - 1) < 0.001;

  const transport = {
    connect({ callbacks: given }) {
      callbacks = given;
      stopped = false;
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
      if (options.readOnly || muted || !live || socket?.readyState !== WebSocket.OPEN) return false;
      socket.send(encoder.encode(data));
      return true;
    },
    resize(cols, rows, meta) {
      if (!(cols > 1 && rows > 0)) return false;
      size = { type: 'resize', cols, rows, width: Math.round(meta?.widthPx || 0), height: Math.round(meta?.heightPx || 0) };
      clearTimeout(sizeTimer);
      sizeTimer = setTimeout(sendSize, 80);
      return true;
    },
    isConnected: () => live,
    destroy() { transport.disconnect(); },
  };

  function sendSize() {
    // A view that only watches leaves the shell its size.
    if (!options.readOnly && size && socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify(size));
  }

  function retry() {
    if (stopped || disposed || exited) return;
    live = false;
    attempts++;
    clearTimeout(timer);
    timer = setTimeout(attach, Math.min(5000, 150 * 2 ** Math.min(attempts, 6)));
  }

  async function attach() {
    if (stopped || disposed || exited) return;
    // A shell that ended while nothing showed it is gone.
    try {
      await cockpit.api(`/api/terminals/${encodeURIComponent(id)}`);
    } catch (error) {
      if (error.status === 404) {
        ended(null);
        return;
      }
      retry();
      return;
    }
    if (stopped || disposed) return;
    const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
    const s = new WebSocket(`${scheme}://${location.host}/api/terminals/${encodeURIComponent(id)}/socket`);
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
      retry();
    });
  }

  function control(message) {
    switch (message.type) {
      case 'hello': {
        // The output the server kept draws the screen anew; what the
        // terminal answers to it is not sent: the shell asked long ago.
        muted = true;
        decoder = new TextDecoder();
        callbacks?.onData?.('\x1bc\x1b[3J');
        const info = message.terminal || {};
        cockpit.safely(() => options.onMeta?.({ title: info.title || '', cwd: info.cwd || '' }));
        break;
      }
      case 'live':
        muted = false;
        live = true;
        attempts = 0;
        callbacks?.onConnect?.();
        sendSize();
        cockpit.safely(() => options.onReady?.());
        break;
      case 'meta':
        cockpit.safely(() => options.onMeta?.({ title: message.title || '', cwd: message.cwd || '' }));
        break;
      case 'exit':
        ended(message.code ?? null);
        break;
    }
  }

  function ended(code) {
    if (exited) return;
    exited = true;
    live = false;
    stopped = true;
    cockpit.safely(() => options.onExit?.(code));
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
        searchUi: false,
        defaultContextMenu: false,
        contextMenu: null,
        minPaneSize: 40,
        paneStyles: {
          splitBackground: c.line, paneBackground: c.background, inactivePaneOpacity: 1, activePaneOpacity: 1,
          opacityTransitionMs: 0, dividerColor: c.line, dividerThicknessPx: 1,
        },
        events: { onActivePaneChange: () => cockpit.safely(() => options.onFocus?.()) },
      },
      terminal: () => ({
        renderer: 'auto', fontSize: fontSize(), fonts: env.fonts(), theme, ligatures: false,
        autoResize: false, showResizeOverlay: false, maxScrollbackBytes: 4_000_000,
      }),
      services: () => ({ ptyTransport: transport }),
    });
    const pane = restty.createInitialPane({ focus: false });
    restty.pane(pane.id)?.connectPty('kou-conveyor');
    if (paused) handle()?.setPaused(true);
    resize(true);
    if (wantFocus) {
      wantFocus = false;
      requestAnimationFrame(() => !disposed && handle()?.focus?.());
    }
  }

  const handle = () => restty?.panes?.()[0] || null;

  // resize has the terminal take the room its element has. Restty measures
  // its canvas's box, which a scale around the element shrinks or grows:
  // the element is scaled back for the measure, which reads its layout at
  // once — nothing paints in between — so the grid and the canvas are the
  // element's own size at any zoom. (A quarter of a device pixel more keeps
  // the rounding of the scales from making it a pixel less.)
  let sizing = 0;
  function resize(force = true) {
    if (!restty) return;
    cancelAnimationFrame(sizing);
    sizing = requestAnimationFrame(() => {
      if (!restty || disposed) return;
      const width = surface.clientWidth;
      const height = surface.clientHeight;
      if (width < 40 || height < 20) return;
      const pane = handle();
      if (!pane) return;
      if (unscaled()) {
        pane.updateSize(force);
      } else {
        const s = scale();
        const more = 0.25 / (window.devicePixelRatio || 1);
        surface.style.transformOrigin = '0 0';
        surface.style.transform = `scale(${(width + more) / (width * s)}, ${(height + more) / (height * s)})`;
        try {
          pane.updateSize(true);
        } finally {
          surface.style.transform = '';
          surface.style.transformOrigin = '';
        }
      }
      clearTimeout(sizeTimer);
      sizeTimer = setTimeout(sendSize, 80);
    });
  }

  const started = start().catch((error) => {
    console.error(error);
    cockpit.safely(() => options.onError?.(error));
  });

  return {
    started,
    focus() {
      if (!restty) {
        wantFocus = true;
        return;
      }
      requestAnimationFrame(() => (restty?.activePane?.() || handle())?.focus?.());
    },
    blur() {
      wantFocus = false;
      handle()?.blur?.();
    },
    resize,
    setPaused(value) {
      paused = !!value;
      handle()?.setPaused(paused);
    },
    applyTheme() {
      if (!restty) return;
      env.restty().then((lib) => {
        if (!restty) return;
        const c = colours();
        handle()?.applyTheme(lib.parseGhosttyTheme(ghostty()), 'kou-conveyor');
        restty.setPaneStyleOptions({ splitBackground: c.line, paneBackground: c.background, dividerColor: c.line });
      });
    },
    applySettings() {
      const pane = handle();
      if (!pane) return;
      pane.setFontSize(fontSize());
      pane.setFonts(env.fonts());
      resize(true);
    },
    // write types into the shell, as a paste would.
    write(text) {
      if (!socket || socket.readyState !== WebSocket.OPEN) return false;
      socket.send(encoder.encode(String(text)));
      return true;
    },
    connected: () => live,
    exited: () => exited,
    contains: (node) => surface.contains(node),
    dispose() {
      if (disposed) return;
      disposed = true;
      stopEditing();
      cancelAnimationFrame(sizing);
      transport.disconnect();
      const r = restty;
      restty = null;
      try {
        r?.destroy();
      } catch (error) {
        console.error(error);
      }
      surface.remove();
      cockpit.safely(() => options.onDispose?.());
    },
  };
}
