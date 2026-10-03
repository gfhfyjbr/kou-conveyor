// How a node's status shows on the page. A command at work that has shown
// nothing for a while — a server, a watcher, a program that waits for its
// input — runs on quietly (the engine's Status.Quiet): it is running, not
// busy, and nothing blinks or scans for it.

const WORDS = { paused: 'proposed' };

// shownState is the state a node's status shows as; fallback, the one a
// node without a status has.
export function shownState(status, fallback = 'stopped') {
  const state = status?.state || fallback;
  return state === 'busy' && status?.quiet ? 'running' : state;
}

// stateWord is what a state shown is called.
export const stateWord = (state) => WORDS[state] || state;

// atWork says a state shown is one whose time counts.
export const atWork = (state) => state === 'busy' || state === 'starting' || state === 'running';
