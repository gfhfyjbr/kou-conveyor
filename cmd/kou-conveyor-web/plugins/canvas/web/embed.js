// The whole chat of a kou agent node: the cockpit's own page, in a frame
// of the node (nodes/agent.js), showing the agent's session as the stage
// shows any — the transcript with its thinking, tools and their output,
// the composer with images, the model and the effort, the queue, Edit,
// Continue, the changes, the inspector, the commands. The page in the
// frame is the cockpit itself, with every plugin; only its rail is put
// away. Its address says what it is for:
//
//   /?embed=canvas-chat&ws=<workspace>&session=<id>#/w/<workspace>/s/<id>
//
// The page in the frame is one only while the page around it is of the
// same origin. It keeps to its session: an address that leads elsewhere —
// another session, a fork — opens in the page around it, which it tells,
// as it tells it when it has the keys, gives them back (Esc where Esc
// means nothing, Shift+Esc always) or zooms (⌘ and the wheel, a pinch).
export const EMBED = 'canvas-chat';
// The key of the messages between the frame and the page around it.
export const MESSAGE = 'kou-canvas-chat';

const ID = /^[A-Za-z0-9-]{1,128}$/;
const WS = /^[A-Za-z0-9-]{1,80}$/;
// The addresses the frame may hand to the page around it.
const OPENABLE = /^#\/w\/[A-Za-z0-9-]{1,80}(?:\/[sc]\/[A-Za-z0-9-]{1,128})?$/;
// What the page in the frame keeps in the tab's preferences that is the
// page around it's too: what it changes, it gives back as it goes.
const SHARED = ['panel', 'widths', 'changes', 'inspector'];

export const chatHash = (ws, sid) => `#/w/${ws}/s/${sid}`;

// chatURL is the address of the chat of a session, or '' for IDs no
// address takes.
export function chatURL(ws, sid) {
  if (!WS.test(ws || '') || !ID.test(sid || '')) return '';
  return `/?embed=${EMBED}&ws=${ws}&session=${sid}${chatHash(ws, sid)}`;
}

// openable says an address from a frame may be followed by the page.
export const openable = (hash) => typeof hash === 'string' && OPENABLE.test(hash);

// message is what a message event carries for a chat, or null: from the
// window expected, of this origin, with the key.
export function message(event, from, origin) {
  if (!event || event.source !== from || event.origin !== origin) return null;
  const data = event.data;
  if (!data || typeof data !== 'object' || data[MESSAGE] !== true || typeof data.type !== 'string') return null;
  return data;
}

// embedded is what the page in a frame is for — { ws, sid } — or null for
// a page that is not one: not framed, framed by another origin, or with an
// address of another sort.
export function embedded(loc = location, win = window) {
  const query = new URLSearchParams(loc.search);
  if (query.get('embed') !== EMBED) return null;
  const ws = query.get('ws') || '';
  const sid = query.get('session') || '';
  if (!WS.test(ws) || !ID.test(sid)) return null;
  if (!win.parent || win.parent === win) return null;
  try {
    if (win.parent.location.origin !== loc.origin) return null;
  } catch {
    return null; // another origin's page: it may not even be read
  }
  return { ws, sid };
}

// framesAllowed says whether the server lets the chat be framed: a build
// from before it did answers framing with 'none', and the node keeps its
// feed. A yes holds for the page; a no, until askFramesAgain — the server
// came back, maybe as a build that lets it.
let allowed = null;
let answer = null; // what the server said, once it did
export function framesAllowed() {
  allowed ??= fetch(`/?embed=${EMBED}`, { cache: 'no-store', credentials: 'same-origin' })
    .then((res) => res.ok && /frame-ancestors 'self'/.test(res.headers.get('content-security-policy') || ''))
    .catch(() => false)
    .then((ok) => (answer = ok));
  return allowed;
}

// askFramesAgain forgets a no, for the next framesAllowed to ask anew.
export function askFramesAgain() {
  if (answer === false) allowed = answer = null;
}

// activateEmbed is the canvas plugin in the page in a frame: no canvas,
// only what keeps the page to its session and tells the page around it.
export function activateEmbed(cockpit, { ws, sid }) {
  const { prefs } = cockpit;
  const root = document.documentElement;
  const session = cockpit.use('session');
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const target = chatHash(ws, sid);
  const parent = window.parent;
  const post = (type, data = {}) => {
    try {
      parent.postMessage({ ...data, [MESSAGE]: true, type }, location.origin);
    } catch { /* the page around it went */ }
  };
  root.dataset.cvEmbed = 'chat';
  cockpit.onDispose(() => { delete root.dataset.cvEmbed; });

  // The preferences of the page around it, as they were.
  const kept = Object.fromEntries(SHARED.map((key) => [key, prefs.get(key, null)]));
  const giveBack = () => { for (const key of SHARED) prefs.set(key, kept[key]); };
  // A panel the page around it had open would cover the chat here.
  service('layout')?.closePanel?.();
  giveBack();
  cockpit.listen(window, 'pagehide', giveBack);
  cockpit.onDispose(giveBack);

  // Light or dark, as the page around it is, either way. The page around it
  // says which (nodes/agent.js) once this page is ready, and each time it
  // changes; what the user switches here, this page tells it. Neither page
  // holds anything of the other: this one goes whole once its frame is let
  // go.
  const themed = (t) => {
    if (t === 'light' || t === 'dark') {
      if (root.dataset.theme !== t) root.dataset.theme = t;
    } else if (t === '') {
      delete root.dataset.theme; // the page around it follows the system's
    }
  };
  cockpit.on('theme', (t) => post('theme', { theme: t }));

  // ---------------------------------------------------------------- its session

  // A view of another session goes to the page around it, and this one comes
  // back; a session that will not stay (gone, deleted) is said to be gone.
  let reverts = [];
  let gone = false;
  function keep() {
    if (gone) return;
    const now = Date.now();
    reverts = reverts.filter((t) => now - t < 5000);
    reverts.push(now);
    if (reverts.length > 3) {
      gone = true;
      post('gone');
      return;
    }
    if (location.hash !== target) history.replaceState(null, '', target);
    session.openSession(sid, { push: false });
  }
  const ours = (v) => !!v && v.id === sid && v.ws === ws;
  cockpit.on('session:view', (v) => {
    if (!v || ours(v)) return;
    // The address follows the view a moment later (openSession, newSession).
    setTimeout(() => {
      if (ours(session.view?.())) return;
      if (!v.fresh) {
        const hash = openable(location.hash) && location.hash !== target ? location.hash : `#/w/${v.ws}/s/${v.id}`;
        if (openable(hash)) post('open', { hash });
      }
      keep();
    }, 0);
  });
  cockpit.on('session:loaded', (v) => {
    if (!ours(v)) return;
    reverts = [];
    post('ready');
  });
  cockpit.on('session:gone', (v) => { if (ours(v)) post('gone'); });
  // Its session before the node's first run: the node has its ID from when
  // it was made, the session its transcript with the first prompt. Not to
  // be found yet, it is a new session under that ID (the session plugin's
  // "session.missing"), whose first prompt the server gives the node's
  // brief.
  cockpit.hooks.tap('session.missing', ({ id, ws: w } = {}) => {
    if (id !== sid || w !== ws) return undefined;
    setTimeout(() => {
      if (!ours(session.view?.())) return;
      reverts = [];
      post('ready');
    }, 0);
    return true;
  });
  // Its title is the session list's — the node's, which the canvas gave the
  // session — once the list has it: a session the chat made has none of its
  // own before then but its first prompt, which starts with the brief.
  cockpit.on('session:sessions', (list) => {
    const v = session.view?.();
    if (!ours(v) || v.renamed || !Array.isArray(list)) return;
    const title = list.find((s) => s.id === sid)?.title;
    if (!title || v.title === title) return;
    v.title = title;
    cockpit.render();
  });

  // ---------------------------------------------------------------- keys, focus, zoom

  // The keys come here only when the user brings them: a page that boots
  // does not take them from the board (its composer would).
  let engaged = false;
  const engage = () => {
    if (!engaged) post('focus');
    engaged = true;
  };
  const stray = () => {
    const el = document.activeElement;
    if (engaged || !el || el === document.body) return;
    el.blur?.();
    post('unfocus');
  };
  stray();
  cockpit.listen(document, 'focusin', () => queueMicrotask(stray), { capture: true });
  cockpit.listen(window, 'pointerdown', engage, { capture: true });
  cockpit.listen(window, 'blur', () => { engaged = false; });

  // What the page around it asks.
  cockpit.listen(window, 'message', (event) => {
    const data = message(event, parent, location.origin);
    if (data?.type === 'theme') {
      themed(data.theme);
      return;
    }
    if (data?.type !== 'focus') return;
    engaged = true;
    window.focus();
    service('composer')?.focus?.();
  });

  // Esc that meant nothing here, and Shift+Esc, give the keys back to the
  // board. The cockpit's own Esc (stop, edit, close) goes first.
  cockpit.listen(window, 'keydown', (event) => {
    if (event.key !== 'Escape' || event.isComposing || event.defaultPrevented) return;
    event.preventDefault();
    engaged = false;
    post('exit');
  });
  cockpit.contribute('help.keys', { keys: ['Shift+Esc'], text: 'Back to the canvas', order: 5 });
  cockpit.keys.register({ key: 'Shift+Escape', global: true, priority: 200, run: () => { engaged = false; post('exit'); } });

  // ⌘ and the wheel, a pinch: the board zooms, not this page.
  cockpit.listen(window, 'wheel', (event) => {
    if (!event.ctrlKey && !event.metaKey) return;
    event.preventDefault();
    const k = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? window.innerHeight : 1;
    post('zoom', { deltaY: event.deltaY * k, x: event.clientX, y: event.clientY });
  }, { passive: false, capture: true });
  let pinch = 1;
  cockpit.listen(window, 'gesturestart', (event) => {
    event.preventDefault();
    pinch = 1;
  });
  cockpit.listen(window, 'gesturechange', (event) => {
    event.preventDefault();
    const ratio = event.scale / pinch;
    pinch = event.scale;
    if (ratio > 0) post('zoom', { deltaY: -Math.log(ratio) / 0.006, x: event.clientX, y: event.clientY });
  });
  cockpit.listen(window, 'gestureend', (event) => event.preventDefault());

  // A session opened before this plugin came (a reload of it) is ready now.
  const v = session.view?.();
  if (ours(v) && !v.loading) post('ready');
}
