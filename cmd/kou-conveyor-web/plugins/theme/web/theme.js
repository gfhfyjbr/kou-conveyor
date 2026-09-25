// theme: the design tokens and the components every plugin shares
// (theme.css), and light or dark — T, /theme, the palette, a button at the
// foot of the rail. A plugin that only restyles the cockpit can redefine
// the tokens (--bg, --fg, --accent…) in its own style sheet.
const ICON = '<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="5.5" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M8 2.5v11a5.5 5.5 0 0 0 0-11z" fill="currentColor"/></svg>';

export default function activate(cockpit) {
  const { h, svg, prefs } = cockpit;
  const root = document.documentElement;

  const current = () => root.dataset.theme || (matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark');
  function set(theme) {
    if (theme !== 'light' && theme !== 'dark') return;
    root.dataset.theme = theme;
    prefs.set('theme', theme);
    cockpit.emit('theme', theme);
  }
  const toggle = () => set(current() === 'light' ? 'dark' : 'light');

  cockpit.provide('theme', { current, set, toggle });
  cockpit.ui.mount('rail.actions', {
    id: 'theme-toggle', order: 20,
    node: h('button', { class: 'icon', id: 'theme-toggle', type: 'button', title: 'Toggle light / dark (T)', 'aria-label': 'Toggle theme', onclick: toggle }, svg(ICON)),
  });
  cockpit.keys.register({ key: 't', run: toggle });
  cockpit.commands.register({ name: 'theme', help: 'Switch between light and dark', order: 230, run: toggle });
  cockpit.palette.register({ group: 'Actions', icon: '◐', label: 'Toggle light / dark', hint: 'T', order: 170, run: toggle });
  cockpit.contribute('help.keys', { keys: ['T'], text: 'Light / dark', order: 230 });
}
