// Event streams: what the server says as it comes, such as the workspace's
// plugins (host.js), a run's events (the session plugin) and a canvas (its
// model). Where the page came over HTTP/1.1 they are read over WebSockets,
// as an EventSource would read them.
//
// Over HTTP/1.1 a browser keeps at most six connections to a host, for all
// its tabs together, and an EventSource holds one for as long as it is
// open. A canvas whose kou agents show their chats runs out of them: each
// chat is a page of the cockpit in a frame, with streams of its own. Then
// everything else its pages ask for waits behind them, and the chats never
// come. A WebSocket is not one of the six. The server answers a GET of
// …/events that asks to be upgraded with the same stream, as binary
// messages (eventsocket.go). A stream openEvents gives reads them as an
// EventSource would: the same events with their IDs, resuming from the
// last one it had (last_event_id).
//
// Over HTTP/2 or HTTP/3 the streams share one connection, so openEvents
// gives a plain EventSource. Otherwise, before the first stream, the page
// asks the server whether it takes sockets (PROBE). A server from before
// sockets says no, and the page reads its streams as EventSources until a
// newer server comes back.

const PROBE = '/api/sockets/events';
// The server's streams ping at least every 20 s: a socket silent for
// longer than SILENT is broken, though the browser may not know it yet. A
// stream read with openEvents pings so too (a comment does), or it is
// connected anew each time it is silent so long.
const SILENT = 45_000;
// How long the server may take to answer the probe.
const ASK_WITHIN = 8_000;
// After attempts that heard nothing, each wait is the reconnection time
// once more, up to this: a server that builds itself anew is back in
// seconds.
const MAX_WAIT = 10_000;

const CONNECTING = 0;
const OPEN = 1;
const CLOSED = 2;

// How the page reads its streams:
//   'h2'      - its connection carries any number of them, as EventSources;
//   'sockets' - over WebSockets;
//   'events'  - as EventSources, because the server takes no sockets;
//   ''        - not known yet.
let how = typeof WebSocket !== 'function' ? 'events' : multiplexed() ? 'h2' : '';
const following = new Set(); // the streams not closed
const watchers = new Set();
let asking = null;
const RETARGET = Symbol('retarget');

function multiplexed() {
  try {
    return /^h[23]/.test(globalThis.performance?.getEntriesByType?.('navigation')?.[0]?.nextHopProtocol || '');
  } catch {
    return false;
  }
}

// streams says how the page reads its streams: 'h2', 'sockets', 'events'
// or '' (see how above).
export function streams() {
  return how;
}

// onStreams calls fn with how the streams are read each time that changes.
// It returns a function that stops it.
export function onStreams(fn) {
  watchers.add(fn);
  return () => watchers.delete(fn);
}

// settled resolves with how the streams are read once that is known, or
// with '' after within ms.
export function settled(within = 3000) {
  if (how) return Promise.resolve(how);
  ask();
  return new Promise((resolve) => {
    const done = () => {
      clearTimeout(timer);
      stop();
      resolve(how);
    };
    const stop = onStreams(done);
    const timer = setTimeout(done, within);
  });
}

// openEvents follows the event stream at url, as new EventSource(url)
// would. What it returns is used the same way: addEventListener, onopen,
// onmessage, onerror, readyState (EventSource.CONNECTING, OPEN, CLOSED),
// lastEventId and close().
export function openEvents(url) {
  if (how === 'h2' || typeof WebSocket !== 'function') return new EventSource(url);
  return new Stream(url);
}

function decide(found) {
  if (!found || found === how) return;
  how = found;
  for (const stream of [...following]) stream[RETARGET]();
  for (const fn of [...watchers]) {
    try {
      fn(how);
    } catch (error) {
      console.error(error);
    }
  }
}

// ask finds out whether the server takes streams over sockets. It opens one
// to PROBE, which the server upgrades and closes at once. If that is
// refused, the page asks plainly. A server that answers takes no sockets:
// it is a build from before them, or a proxy in between passes none. If
// nothing answers, the server is away (it restarts) and nothing is known.
function ask() {
  asking ||= (async () => {
    if (await opens(socketURL(PROBE))) return 'sockets';
    try {
      const res = await fetch(PROBE, { cache: 'no-store' });
      return res.status < 500 ? 'events' : '';
    } catch {
      return '';
    }
  })().then((found) => {
    decide(found);
    // The answer holds for a moment, so streams that come back together
    // ask only once.
    setTimeout(() => { asking = null; }, 1000);
    return found;
  });
  return asking;
}

function opens(url) {
  return new Promise((resolve) => {
    let socket;
    try {
      socket = new WebSocket(url);
    } catch {
      resolve(false);
      return;
    }
    const done = (yes) => {
      clearTimeout(timer);
      socket.onopen = socket.onerror = socket.onclose = null;
      if (socket.readyState === WebSocket.CONNECTING || socket.readyState === WebSocket.OPEN) socket.close();
      resolve(yes);
    };
    const timer = setTimeout(() => done(false), ASK_WITHIN);
    socket.onopen = () => done(true);
    socket.onerror = socket.onclose = () => done(false);
  });
}

function socketURL(address, lastEventId = '') {
  const url = new URL(address, location.href);
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
  if (lastEventId) url.searchParams.set('last_event_id', lastEventId);
  return url.href;
}

// A Stream is what openEvents gives where streams may go over sockets. It
// reads a socket or, if the server takes none, an EventSource of its own,
// and it moves from one to the other as the server changes.
class Stream extends EventTarget {
  static CONNECTING = CONNECTING;
  static OPEN = OPEN;
  static CLOSED = CLOSED;
  CONNECTING = CONNECTING;
  OPEN = OPEN;
  CLOSED = CLOSED;

  url;
  withCredentials = false;
  readyState = CONNECTING;
  lastEventId = '';

  #handlers = { open: null, message: null, error: null };
  // The types listened to: an EventSource underneath passes these on.
  #types = new Set(['open', 'message', 'error']);
  #retry = 3000; // the reconnection time, which the server's retry: sets
  #failures = 0; // attempts in a row that heard nothing
  #socket = null;
  #source = null;
  #relay = null;
  #timer = 0; // the next attempt
  #waiting = false; // until the server says how streams come
  #heard = 0; // when the socket last said something
  #watchdog = 0;

  constructor(url) {
    super();
    this.url = new URL(url, location.href).href;
    for (const type of Object.keys(this.#handlers)) {
      super.addEventListener(type, (event) => this.#handlers[type]?.call(this, event));
    }
    following.add(this);
    this.#connect();
  }

  get onopen() { return this.#handlers.open; }
  set onopen(fn) { this.#handlers.open = typeof fn === 'function' ? fn : null; }
  get onmessage() { return this.#handlers.message; }
  set onmessage(fn) { this.#handlers.message = typeof fn === 'function' ? fn : null; }
  get onerror() { return this.#handlers.error; }
  set onerror(fn) { this.#handlers.error = typeof fn === 'function' ? fn : null; }

  addEventListener(type, listener, options) {
    super.addEventListener(type, listener, options);
    if (this.#types.has(type)) return;
    this.#types.add(type);
    this.#source?.addEventListener(type, this.#relay);
  }

  close() {
    this.readyState = CLOSED;
    this.#stop();
  }

  // #stop lets go of everything: socket, EventSource, the next attempt.
  #stop() {
    following.delete(this);
    clearTimeout(this.#timer);
    clearTimeout(this.#watchdog);
    this.#timer = this.#watchdog = 0;
    this.#waiting = false;
    const socket = this.#socket;
    const source = this.#source;
    this.#socket = this.#source = this.#relay = null;
    socket?.close();
    source?.close();
  }

  #connect() {
    this.#timer = 0;
    if (this.readyState === CLOSED) return;
    if (how === 'sockets') return this.#openSocket();
    if (how) return this.#openSource();
    // How streams come is not known yet. The server is asked once for all
    // the streams that wait.
    this.#waiting = true;
    ask().then(() => {
      if (!this.#waiting) return; // decided meanwhile: RETARGET connected it
      this.#waiting = false;
      if (how) this.#connect();
      else this.#lost(false); // no server
    });
  }

  // #lost: the connection went, or none came. As an EventSource does, the
  // stream connects again after the server's reconnection time, from the
  // last event it had. The wait grows each time nothing came.
  #lost(heard) {
    if (this.readyState === CLOSED) return;
    clearTimeout(this.#watchdog);
    this.#watchdog = 0;
    if (!heard) this.#failures++;
    this.readyState = CONNECTING;
    this.#fire(new Event('error'));
    if (this.readyState === CLOSED) return; // a listener closed it
    // Sockets that fail again and again may mean the server takes none now
    // (a build from before them is back), or something in between passes
    // none.
    if (this.#failures >= 3 && how === 'sockets') ask();
    const wait = Math.min(this.#retry * Math.max(1, this.#failures), MAX_WAIT);
    this.#timer = setTimeout(() => this.#connect(), wait);
  }

  #fire(event) {
    this.dispatchEvent(event);
  }

  #openSocket() {
    let socket;
    try {
      socket = new WebSocket(socketURL(this.url, this.lastEventId));
    } catch {
      this.#lost(false);
      return;
    }
    socket.binaryType = 'arraybuffer';
    this.#socket = socket;
    const decoder = new TextDecoder();
    let heard = false;
    const parser = createParser({
      lastEventId: this.lastEventId,
      onRetry: (ms) => { this.#retry = ms; },
      onEvent: ({ type, data, lastEventId }) => {
        if (this.#socket !== socket) return;
        this.lastEventId = lastEventId;
        if (data !== null) this.#fire(new MessageEvent(type, { data, lastEventId, origin: location.origin }));
      },
    });
    socket.onmessage = ({ data }) => {
      if (this.#socket !== socket) return;
      this.#heard = performance.now();
      if (!heard) {
        heard = true;
        this.#failures = 0;
        this.#watch(SILENT);
        // The stream is open once the server streams. A server with nothing
        // to stream closes the socket at once, and never opens it.
        if (this.readyState !== OPEN) {
          this.readyState = OPEN;
          this.#fire(new Event('open'));
          if (this.#socket !== socket) return;
        }
      }
      parser.push(typeof data === 'string' ? data : decoder.decode(data, { stream: true }));
    };
    socket.onclose = ({ code }) => {
      if (this.#socket !== socket) return;
      this.#socket = null;
      // 4000 plus a status means the server answered with no stream (204:
      // nothing more; 404: not found). An EventSource gives up on that.
      if (code >= 4000 && code < 5000) {
        this.readyState = CLOSED;
        this.#stop();
        this.#fire(new Event('error'));
        return;
      }
      this.#lost(heard);
    };
  }

  #watch(after) {
    clearTimeout(this.#watchdog);
    this.#watchdog = setTimeout(() => {
      this.#watchdog = 0;
      const socket = this.#socket;
      if (!socket) return;
      const quiet = performance.now() - this.#heard;
      if (quiet < SILENT) {
        this.#watch(SILENT - quiet);
        return;
      }
      // Silent past its pings: the connection broke without a word (the
      // network went, or the machine slept). Connect again.
      this.#socket = null;
      socket.close();
      this.#lost(true);
    }, after);
  }

  #openSource() {
    const source = new EventSource(this.url);
    let erred = false;
    const relay = (event) => {
      if (this.#source !== source) return;
      if (event instanceof MessageEvent) {
        this.lastEventId = event.lastEventId;
        this.#fire(new MessageEvent(event.type, { data: event.data, lastEventId: event.lastEventId, origin: event.origin }));
      } else if (event.type === 'open') {
        this.#failures = 0;
        if (this.readyState !== OPEN) {
          this.readyState = OPEN;
          this.#fire(new Event('open'));
        }
        // Back after an error: the server may be a newer build now, one
        // that takes sockets.
        if (erred && how === 'events') ask();
        erred = false;
      } else if (event.type === 'error') {
        erred = true;
        if (source.readyState === EventSource.CLOSED) {
          this.readyState = CLOSED;
          this.#stop();
        } else {
          this.readyState = CONNECTING;
        }
        this.#fire(new Event('error'));
      }
    };
    this.#source = source;
    this.#relay = relay;
    for (const type of this.#types) source.addEventListener(type, relay);
  }

  // RETARGET follows a change in how streams are read.
  [RETARGET]() {
    if (this.readyState === CLOSED) return;
    if (this.#waiting) {
      this.#waiting = false;
      this.#connect();
    } else if (this.#timer) {
      // Waiting to connect again: do it now, the way the server takes.
      clearTimeout(this.#timer);
      this.#connect();
    } else if (how === 'sockets' && this.#source) {
      // The EventSource holds a connection the page is short of. The stream
      // goes over a socket instead, from the last event it had.
      const source = this.#source;
      this.#source = this.#relay = null;
      source.close();
      this.#openSocket();
    }
  }
}

// createParser reads an event stream as it comes, in pieces cut anywhere,
// the way the HTML standard has an EventSource read it. Lines end with CR
// LF, LF or CR. A line that begins with a colon is a comment (the servers'
// pings). A blank line ends an event: onEvent takes {type, data,
// lastEventId}, with data null if no data line came (then only the ID
// counts). onRetry takes the reconnection time a retry: line sets.
export function createParser({ lastEventId = '', onEvent = () => {}, onRetry = () => {} } = {}) {
  const breaks = /\r\n|\r|\n/g;
  let pending = ''; // the line begun and not ended yet
  let afterCR = false; // the last piece ended with CR: an LF first in the next ends nothing
  let type = '';
  let data = '';
  let id = lastEventId;

  function line(text) {
    if (!text) {
      const event = { type: type || 'message', data: data ? data.slice(0, -1) : null, lastEventId: id };
      type = data = '';
      onEvent(event);
      return;
    }
    if (text[0] === ':') return;
    const colon = text.indexOf(':');
    const name = colon < 0 ? text : text.slice(0, colon);
    let value = colon < 0 ? '' : text.slice(colon + 1);
    if (value[0] === ' ') value = value.slice(1);
    if (name === 'event') type = value;
    else if (name === 'data') data += `${value}\n`;
    else if (name === 'id' && !value.includes('\0')) id = value;
    else if (name === 'retry' && /^\d+$/.test(value)) onRetry(Number(value));
  }

  return {
    push(text) {
      if (afterCR && text) {
        afterCR = false;
        if (text[0] === '\n') text = text.slice(1);
      }
      let start = 0;
      breaks.lastIndex = 0;
      for (let found; (found = breaks.exec(text));) {
        line(pending + text.slice(start, found.index));
        pending = '';
        start = breaks.lastIndex;
        if (found[0] === '\r' && start === text.length) afterCR = true;
      }
      pending += text.slice(start);
    },
  };
}
