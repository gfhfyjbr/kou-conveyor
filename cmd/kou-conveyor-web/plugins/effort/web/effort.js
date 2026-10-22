// effort: the effort of the session's next runs — the bars in composer.row,
// ⌥↑ ⌥↓, /effort and the palette. Each session keeps its own; a new one
// starts with the effort last chosen in a composer of either cockpit, which
// the server keeps where the terminal one reads it too. A choice in one
// session changes no other. It provides the effort service: current(v),
// levels(), set(level), cycle(step), command(arg), refresh(), info().
// refresh also tells the other plugins what the preferences hold, with the
// "preferences" event.
const LEVELS = ['low', 'medium', 'high', 'xhigh', 'max'];

export default function activate(cockpit) {
  const { h, prefs } = cockpit;
  const session = cockpit.use('session');
  const hot = cockpit.hot.data;
  hot.last ??= null; // the effort last chosen, which new sessions start with
  hot.chosen ??= 0; // when it was last chosen here (performance.now())

  const config = () => session.state?.config;
  const view = () => session.view?.();
  const levels = () => config()?.thinking_levels || LEVELS;
  const valid = (level) => !!level && levels().includes(level);
  const effortKey = (v) => `effort.${v?.ws || 'default'}.${v?.id || 'new'}`;
  const last = () => (valid(hot.last) && hot.last) || (valid(config()?.thinking) && config().thinking) || 'high';

  // current is the effort of v's next runs: its own, else, for a session
  // still to start, the effort last chosen. A session that ran before
  // sessions kept their own takes the last chosen once, and keeps it; so
  // does a new one once its first prompt is on its way (v.run).
  const current = (v = view()) => {
    if (!v) return last();
    if (!valid(v.effort)) {
      const saved = prefs.get(effortKey(v), '');
      if (valid(saved)) v.effort = saved;
      else if ((!v.fresh || v.run) && !v.loading && config()) v.effort = last();
    }
    return valid(v.effort) ? v.effort : last();
  };

  const bars = h('span', { class: 'think-bars', id: 'think-bars', role: 'radiogroup', 'aria-label': 'Effort' });
  const label = h('span', { class: 'think-label', id: 'think-label', text: 'high' });
  // On a touch screen the bars are too narrow to hit: this select lies over
  // the control (effort.css) and opens the system's list of the levels. The
  // bars stay what a screen reader reads.
  const select = h('select', { class: 'think-select', id: 'think-select', tabindex: '-1', 'aria-hidden': 'true', onchange: () => set(select.value) });
  const node = h('div', { class: 'think', title: 'Effort for this session; new sessions start with the last chosen (⌥↑ / ⌥↓)' }, h('span', { class: 'label', text: 'Effort' }), bars, label, select);
  cockpit.ui.mount('composer.row', { id: 'effort', order: 20, node });

  function render() {
    const level = current();
    const all = levels();
    const at = all.indexOf(level);
    bars.replaceChildren(...all.map((name, n) => h('button', {
      type: 'button', role: 'radio', 'aria-checked': String(name === level), 'aria-label': `Effort ${name}`,
      title: `Effort: ${name}`, data: { on: n <= at ? 'true' : null, level: name },
      tabindex: name === level ? 0 : -1,
      onclick: () => set(name),
    })));
    label.textContent = level;
    select.replaceChildren(...all.map((name) => h('option', { value: name, text: `Effort: ${name}` })));
    select.value = level;
  }

  // set chooses the effort of this session's next runs, and of new sessions
  // in every cockpit; the other sessions keep theirs. The session endpoint
  // keeps it with a session that has a file; the local key covers a session
  // still to start, until its first prompt takes the effort along.
  async function set(level) {
    if (!valid(level)) return;
    const v = view();
    const ticket = hot.chosen = performance.now();
    hot.last = level;
    if (v) {
      v.effort = level;
      prefs.set(effortKey(v), level);
    }
    render();
    cockpit.render();
    const own = v && !v.fresh
      ? cockpit.api(cockpit.wsPath(v.ws, `/sessions/${encodeURIComponent(v.id)}`), { method: 'PATCH', body: { effort: level } })
      : null;
    const [shared, kept] = await Promise.allSettled([cockpit.api('/api/preferences', { method: 'PUT', body: { effort: level } }), own]);
    if (kept.status === 'rejected') cockpit.toast(`The effort is kept in this browser only: ${kept.reason?.message}`, 'error', 'effort');
    else if (shared.status === 'rejected' && ticket === hot.chosen) cockpit.toast(`New sessions do not start with this effort: ${shared.reason?.message}`, 'warn', 'effort');
  }

  // refresh asks for the preferences: what was chosen in another window or
  // the terminal is what new sessions start with here too. asked tells the
  // listeners when they were asked for, to drop an answer older than a
  // choice made here.
  async function refresh() {
    const asked = performance.now();
    let saved;
    try {
      saved = await cockpit.api('/api/preferences');
    } catch {
      return;
    }
    cockpit.emit('preferences', saved, asked);
  }
  cockpit.on('preferences', (saved, asked) => {
    if (!valid(saved?.effort) || !(asked > hot.chosen) || saved.effort === hot.last) return;
    hot.last = saved.effort;
    // Only a session still to start follows it.
    const v = view();
    if (v && !v.fresh) return;
    render();
    cockpit.render();
  });

  function cycle(step) {
    const all = levels();
    const at = all.indexOf(current());
    set(all[Math.max(0, Math.min(all.length - 1, at + step))]);
  }

  function command(arg) {
    if (!arg) return cockpit.toast(`Effort: ${current()}. Levels: ${levels().join(', ')}.`);
    const level = arg.toLowerCase();
    if (!levels().includes(level)) return cockpit.toast(`Effort levels: ${levels().join(', ')}.`, 'error');
    set(level);
    return cockpit.toast(`Effort for this session and new ones: ${level}`, 'info', 'effort');
  }

  cockpit.listen(bars, 'keydown', (event) => {
    if (event.key === 'ArrowRight' || event.key === 'ArrowUp') { event.preventDefault(); cycle(1); }
    if (event.key === 'ArrowLeft' || event.key === 'ArrowDown') { event.preventDefault(); cycle(-1); }
    bars.querySelector('[aria-checked="true"]')?.focus();
  });
  // ⌥↑ ⌥↓ in the composer (and in a prompt being edited, which asks).
  cockpit.hooks.tap('composer.key', (event) => {
    if (!event.altKey || (event.key !== 'ArrowUp' && event.key !== 'ArrowDown')) return false;
    event.preventDefault();
    cycle(event.key === 'ArrowUp' ? 1 : -1);
    return true;
  }, { order: 30 });

  cockpit.on('session:config', (c) => {
    if (c && !valid(hot.last)) hot.last = c.thinking;
    render();
  });
  cockpit.on('session:view', (v) => {
    if (v) current(v);
    render();
  });
  cockpit.on('render', () => {
    // A session's own effort arrives with it.
    if (label.textContent !== current()) render();
  });

  // The preferences are asked for while the page is in view.
  cockpit.interval(() => { if (document.visibilityState === 'visible') refresh(); }, 5000);
  cockpit.listen(document, 'visibilitychange', () => { if (document.visibilityState === 'visible') refresh(); });

  const info = () => ({ current: current(), levels: levels() });
  cockpit.provide('effort', { current, levels, set, cycle, command, refresh, info });

  cockpit.commands.register({
    name: 'effort', aliases: ['think'], args: '<level>', help: 'Effort for the next runs of this session, and of new sessions', order: 130, run: command,
    complete: () => levels().map((level) => ({ value: level, label: level, detail: level === current() ? 'current' : '', current: level === current() })),
  });
  cockpit.contribute('palette.provider', {
    id: 'effort', order: 600,
    items: () => levels().map((level) => ({
      group: 'Effort', icon: level === current() ? '●' : '○', label: `Effort: ${level}`, order: 600, run: () => set(level),
    })),
  });
  cockpit.contribute('help.keys', { keys: ['⌥', '↑', ' ', '⌥', '↓'], text: 'Effort', order: 60 });
  render();
}
