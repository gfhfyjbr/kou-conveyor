// scratchpad: a view of the cockpit's own, as a plugin adds one. It shows
// how far a plugin goes:
//
//   - a view (layout.view): a Notes tab in the rail, the notes listed there,
//     and a page in the stage in place of the session;
//   - an address (#/notes), a command (/note), a palette item;
//   - a hook (run.request) that changes every prompt on its way to the
//     agent: a note pinned to prompts goes with each;
//   - state kept across its own versions (hot.data): edit this file while
//     the cockpit is open and the note being edited stays.
export default function activate(cockpit) {
  const { h, prefs } = cockpit;
  const hot = cockpit.hot.data;
  const notes = hot.notes ??= prefs.get('scratchpad.notes', [{ id: cockpit.uuid(), text: '', pinned: false }]);
  hot.selected ??= notes[0]?.id;
  const save = () => prefs.set('scratchpad.notes', notes);
  const selected = () => notes.find((n) => n.id === hot.selected) || notes[0];

  // ---------------------------------------------------------------- the view

  const list = h('nav', { class: 'filters scratch-list', 'aria-label': 'Notes' });
  const rail = h('div', { class: 'scratch-rail' },
    h('div', { class: 'rail-tools' }, h('button', { class: 'new', type: 'button', onclick: () => add() }, h('span', { text: 'New note' }))),
    h('div', { class: 'rail-label' }, h('span', { text: 'Notes' })),
    list);
  const editor = h('textarea', { class: 'scratch-editor', spellcheck: 'true', placeholder: 'Write here. A note pinned to prompts goes with every prompt you run.' });
  const pin = h('label', { class: 'scratch-pin' }, h('input', { type: 'checkbox' }), ' Pin to prompts');
  const page = h('section', { class: 'scratch-page', 'aria-label': 'Notes' },
    h('header', { class: 'bar' },
      h('div', { class: 'crumbs' }, h('strong', { text: 'Notes' })),
      h('div', { class: 'bar-right' }, pin, cockpit.ui.button('Back to sessions', () => back()))),
    h('div', { class: 'scratch-body' }, editor));

  function render() {
    list.replaceChildren(...notes.map((note) => h('button', {
      type: 'button', class: 'filter', 'aria-pressed': String(note.id === selected()?.id),
      onclick: () => { hot.selected = note.id; render(); editor.focus(); },
    }, h('span', { class: 'dot', data: { health: note.pinned ? 'ok' : null } }), h('span', { class: 't', text: note.text.split('\n')[0] || 'Empty note' }), h('span', { class: 'n' }))));
    const note = selected();
    if (editor.value !== note.text && document.activeElement !== editor) editor.value = note.text;
    pin.querySelector('input').checked = !!note.pinned;
  }

  function add(text = '') {
    const note = { id: cockpit.uuid(), text, pinned: false };
    notes.unshift(note);
    hot.selected = note.id;
    save();
    render();
    return note;
  }

  const show = () => {
    if (location.hash !== '#/notes') history.pushState(null, '', '#/notes');
    cockpit.use('session').ensureView?.();
    cockpit.use('layout').show('scratchpad');
  };
  const back = () => cockpit.contributions('layout.view', { unique: 'id' }).find((v) => v.id === 'sessions')?.select?.();

  cockpit.listen(editor, 'input', () => {
    selected().text = editor.value;
    save();
    render();
  });
  cockpit.listen(pin.querySelector('input'), 'change', (event) => {
    selected().pinned = event.target.checked;
    save();
    render();
  });

  cockpit.contribute('layout.view', { id: 'scratchpad', title: 'Notes', order: 30, rail, page, select: show, shown: render });
  cockpit.routes.register({ id: 'scratchpad', priority: 10, match: (hash) => hash === '#/notes', enter: show });
  cockpit.commands.register({
    name: 'note', args: '[text]', help: 'Write a note beside the sessions; alone, open the notes',
    run: (arg) => {
      if (!arg) return show();
      add(arg);
      return cockpit.toast('Noted');
    },
  });
  cockpit.palette.register({ group: 'Notes', icon: '✎', label: 'Notes', detail: 'The scratchpad beside the sessions', run: show });

  // ---------------------------------------------------------------- the hook

  // Pinned notes go with every prompt that runs.
  cockpit.hooks.tap('run.request', (body) => {
    const pinned = notes.filter((n) => n.pinned && n.text.trim()).map((n) => n.text.trim());
    if (!pinned.length || !body.prompt) return undefined;
    return { ...body, prompt: `${body.prompt}\n\nKeep in mind (my notes):\n${pinned.map((t) => `- ${t}`).join('\n')}` };
  });

  render();
}
