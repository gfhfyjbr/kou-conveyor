// effort: the effort of the next runs — the bars in composer.row, ⌥↑ ⌥↓,
// /effort and the palette. The server keeps it where the terminal cockpit
// reads it too, so a choice made in either holds in both. It provides the
// effort service: current(), levels(), set(level), cycle(step),
// command(arg), info().
const LEVELS = ['low', 'medium', 'high', 'xhigh', 'max'];

export default function activate(cockpit) {
  const { h, prefs } = cockpit;
  const session = cockpit.use('session');
  const hot = cockpit.hot.data;
  hot.level ??= null;
  hot.ticket ??= 0;

  const config = () => session.state?.config;
  const levels = () => config()?.thinking_levels || LEVELS;
  const current = () => hot.level || config()?.thinking || 'high';

  const bars = h('span', { class: 'think-bars', id: 'think-bars', role: 'radiogroup', 'aria-label': 'Effort' });
  const label = h('span', { class: 'think-label', id: 'think-label', text: 'high' });
  const node = h('div', { class: 'think', title: 'Effort, shared with the terminal cockpit (⌥↑ / ⌥↓)' }, h('span', { class: 'label', text: 'Effort' }), bars, label);
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
  }

  // set chooses the effort for the next runs, in this tab and in every
  // other cockpit: the server keeps it where the terminal one reads it.
  async function set(level) {
    const ticket = ++hot.ticket;
    hot.level = level;
    render();
    cockpit.render();
    try {
      await cockpit.api('/api/preferences', { method: 'PUT', body: { effort: level } });
    } catch (error) {
      if (ticket === hot.ticket) cockpit.toast(`The effort applies here only: ${error.message}`, 'error', 'effort');
    }
  }

  // refresh takes up an effort chosen in another tab or in the terminal.
  async function refresh() {
    const ticket = hot.ticket;
    let saved;
    try {
      saved = await cockpit.api('/api/preferences');
    } catch {
      return;
    }
    // A choice made here meanwhile is newer.
    if (ticket !== hot.ticket || !saved?.effort || saved.effort === hot.level) return;
    hot.level = saved.effort;
    render();
    cockpit.render();
  }

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
    return cockpit.toast(`Effort for the next runs: ${level}`, 'info', 'effort');
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

  cockpit.on('session:config', (config) => {
    if (!config) return;
    hot.level ??= config.thinking;
    // The effort used to be kept by each browser. The first time the server
    // has none saved, this browser's choice becomes the shared one.
    const local = prefs.get('thinking', null);
    prefs.set('thinking', null);
    if (!config.effort_saved && config.thinking_levels?.includes(local) && local !== current()) set(local);
    render();
  });

  // It follows the other cockpits while the page is in view.
  cockpit.interval(() => { if (document.visibilityState === 'visible') refresh(); }, 5000);
  cockpit.listen(document, 'visibilitychange', () => { if (document.visibilityState === 'visible') refresh(); });

  const info = () => ({ current: current(), levels: levels() });
  cockpit.provide('effort', { current, levels, set, cycle, command, refresh, info });

  cockpit.commands.register({
    name: 'effort', aliases: ['think'], args: '<level>', help: 'Effort for the next runs, shared with the terminal cockpit', order: 130, run: command,
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
