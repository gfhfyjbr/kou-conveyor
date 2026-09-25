// help: the keyboard sheet (?, /help, the button at the foot of the rail).
// It lists what plugins say of their keys — contributions to "help.keys":
// { keys: ['⌘', 'K'], text, order } — and the slash commands in effect, so
// it always tells the cockpit as its plugins make it.
export default function activate(cockpit) {
  const { h } = cockpit;
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);

  const keys = h('dl', { id: 'sheet-keys' });
  const commands = h('dl', { class: 'sheet-commands', id: 'sheet-commands' });
  const close = h('button', { class: 'icon', id: 'sheet-close', type: 'button', 'aria-label': 'Close', onclick: () => hide() }, '×');
  const overlay = h('div', { class: 'overlay', id: 'sheet', hidden: true, onmousedown: (event) => { if (event.target === overlay) hide(); } },
    h('div', { class: 'sheet ticks', role: 'dialog', 'aria-label': 'Keyboard shortcuts' },
      h('header', null, h('span', { class: 'label', text: 'Keyboard' }), close),
      keys,
      h('div', { class: 'sheet-sub' }, h('span', { class: 'label', text: 'Commands' }), h('span', null, 'type ', h('kbd', { text: '/' }), ' in the composer · ', h('kbd', { text: 'Tab' }), ' completes')),
      commands));
  cockpit.ui.mount('overlays', { id: 'sheet', order: 70, node: overlay });
  cockpit.contribute('overlay', { id: 'sheet', order: 70, modal: false, isOpen: () => !overlay.hidden, close: () => hide() });

  // A key of a row: its keys as caps; a lone space separates two.
  const caps = (list) => list.map((key) => (key === ' ' ? ' ' : h('kbd', { text: key })));

  function render() {
    keys.replaceChildren(...cockpit.contributions('help.keys').flatMap((row) => [h('dt', null, caps(row.keys || [])), h('dd', { text: row.text })]));
    commands.replaceChildren(...(service('commands')?.list?.() || []).flatMap((c) => [
      h('dt', null, h('code', { text: `/${c.name}` }), c.args ? h('span', { class: 'args', text: ` ${c.args}` }) : null),
      h('dd', { text: c.help }),
    ]));
  }

  function open() {
    render();
    overlay.hidden = false;
    close.focus();
  }
  function hide() {
    overlay.hidden = true;
  }

  const button = h('button', { class: 'icon', id: 'help-open', type: 'button', title: 'Keyboard shortcuts (?)', 'aria-label': 'Keyboard shortcuts', onclick: open }, '?');
  cockpit.ui.mount('rail.actions', { id: 'help-open', order: 30, node: button });
  cockpit.keys.register({ key: '?', run: open });
  cockpit.commands.register({ name: 'help', aliases: ['?'], help: 'Keyboard shortcuts and commands', order: 250, run: open });
  cockpit.palette.register({ group: 'Actions', icon: '?', label: 'Keyboard shortcuts', hint: '?', order: 190, run: open });
  cockpit.contribute('help.keys', { keys: ['?'], text: 'This sheet', order: 240 });
  cockpit.on('point:help.keys', () => { if (!overlay.hidden) render(); });
  cockpit.provide('help', { open, close: hide });
}
