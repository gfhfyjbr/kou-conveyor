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

export function levelOf(zoom) {
  if (zoom < LOW) return 'low';
  if (zoom < LIVE) return 'mid';
  return 'full';
}

// createBudget hands out the live terminals: of the nodes that want one,
// those used most lately.
export function createBudget(max = MAX_LIVE) {
  const used = new Map(); // node → when it was last used, or first wanted

  return {
    grant(wanting) {
      const now = performance.now();
      for (const id of wanting) if (!used.has(id)) used.set(id, now);
      const ranked = [...wanting].sort((a, b) => used.get(b) - used.get(a));
      return new Set(ranked.slice(0, max));
    },
    touch(id) {
      used.set(id, performance.now());
    },
    forget(id) {
      used.delete(id);
    },
  };
}
