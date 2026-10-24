// Levels of detail: how much of a node the canvas draws at a zoom, and
// which terminals draw live. A live terminal is a restty and a socket: only
// the nodes in view get one, at a zoom where their text can be read, and
// no more than MAX_LIVE at once — those the user had to do with last.
// The others show a snapshot of their screen, or, further out, their
// header alone.

// Below LOW a node is its header: glyph, title and status.
export const LOW = 0.35;
// From LIVE on, terminals draw live.
export const LIVE = 0.6;
export const MAX_LIVE = 8;
// A kou agent node is its whole chat — the cockpit's own, in a frame —
// in view at a zoom where it can be read: every one of a canvas, up to so
// many at once, those used last; the others say they wait and open when
// used (nodes/agent.js). Each chat is a page of the cockpit, with the
// memory one takes and event streams of its own. Over HTTP/1.1 a browser
// holds no more than six connections to a host, for all its tabs
// together, and a stream read as an EventSource keeps one: there is room
// for one chat then. Read over sockets (kernel/events.js), or over HTTP/2
// (a relay), the streams keep none of them: CHATS chats show, COARSE_CHATS
// on a phone or a tablet, which has less memory to give them.
export const CHATS = 12;
export const COARSE_CHATS = 3;
// The protocol the page came over.
export const PROTOCOL = globalThis.performance?.getEntriesByType?.('navigation')?.[0]?.nextHopProtocol || '';

// chatsFor is how many chats show, by the protocol the page came over,
// whether its streams go over sockets, and whether its pointer is coarse.
export function chatsFor(protocol = '', sockets = false, coarse = false) {
  if (!sockets && !/^h[23]/.test(protocol || '')) return 1;
  return coarse ? COARSE_CHATS : CHATS;
}

export function levelOf(zoom) {
  if (zoom < LOW) return 'low';
  if (zoom < LIVE) return 'mid';
  return 'full';
}

// createBudget hands out the live terminals, and the chats: of the nodes
// that want one, those used most lately. A node never used counts from
// when it first wanted — or, with behind, as used before every node that
// was. What order says, the nodes used, the latest first, restore takes
// back on a page opened anew: as used before every node used since.
export function createBudget(max = MAX_LIVE) {
  const used = new Map(); // node → when it was last used, or first wanted (-1: behind every node used)

  return {
    grant(wanting, most = max, { behind = false } = {}) {
      const now = behind ? -1 : performance.now();
      for (const id of wanting) if (!used.has(id)) used.set(id, now);
      const ranked = [...wanting].sort((a, b) => used.get(b) - used.get(a));
      return new Set(ranked.slice(0, most));
    },
    touch(id) {
      used.set(id, performance.now());
    },
    forget(id) {
      used.delete(id);
    },
    order() {
      return [...used].filter(([, at]) => at > -1).sort((a, b) => b[1] - a[1]).map(([id]) => id);
    },
    restore(ids = []) {
      const list = [...new Set(Array.isArray(ids) ? ids : [])].filter((id) => typeof id === 'string' && !used.has(id));
      list.forEach((id, i) => used.set(id, -(i + 1) / (list.length + 1)));
    },
  };
}
