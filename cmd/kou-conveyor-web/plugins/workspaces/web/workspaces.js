// workspaces: the folders the server runs agents in. The switcher heads the
// Sessions view's rail (sessions.tools): a menu of the workspaces, adding
// one — a dialog that completes folders as they are typed — and removing
// one from the list. W opens it, /workspace switches by name. It provides
// the workspaces service: openMenu(anchor), add(), command(query).
export default function activate(cockpit) {
  const { h } = cockpit;
  const session = cockpit.use('session');
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const toast = (text, kind = 'info') => cockpit.toast(text, kind);
  const state = () => session.state || {};

  const name = h('strong', { id: 'ws-name', text: '—' });
  const path = h('code', { id: 'ws-path' });
  const button = h('button', {
    class: 'ws-switch', id: 'ws-switch', type: 'button', 'aria-haspopup': 'menu', 'aria-expanded': 'false', title: 'Switch workspace (W)',
    onclick: (event) => openMenu(event.currentTarget),
  }, h('span', { class: 'label', text: 'Workspace' }), name, path, h('span', { class: 'ws-chev', 'aria-hidden': 'true' }));
  cockpit.ui.mount('sessions.tools', { id: 'ws-switch', order: 10, node: button });

  function render() {
    const w = session.currentWorkspace?.();
    const s = state();
    const label = w?.name || s.config?.workspace_name || 'workspace';
    name.textContent = label;
    path.textContent = w?.display || s.config?.workspace || '';
    path.title = w?.path || '';
    button.dataset.missing = w?.missing ? 'true' : '';
    // Runs elsewhere show on the switcher, so they are not forgotten.
    button.dataset.elsewhere = (s.workspaces || []).some((o) => o.id !== s.ws && o.running) ? 'true' : '';
    button.title = `${w?.path || label} · switch workspace (W)`;
  }
  cockpit.on('render', render);

  function menuItems() {
    const s = state();
    const items = (s.workspaces || []).map((w) => ({
      icon: w.id === s.ws ? '●' : w.running ? '■' : w.missing ? '◌' : '·',
      label: w.name,
      detail: [w.display, w.missing && 'folder missing', w.running && `${w.running} running`].filter(Boolean).join(' · '),
      run: () => session.switchWorkspace(w.id),
    }));
    items.push({ separator: true }, { icon: '+', label: 'Add workspace…', run: add });
    const current = session.currentWorkspace();
    if (current && !current.startup) {
      items.push({
        icon: '×', label: 'Remove from the list', danger: true, confirm: 'Click again · the folder stays',
        disabled: current.running > 0, run: () => session.removeWorkspace(current.id),
      });
    }
    return items;
  }

  function openMenu(anchor = button) {
    service('menu')?.toggle?.(anchor, menuItems);
  }

  function command(arg) {
    if (!arg) return openMenu(button);
    const s = state();
    const query = arg.toLowerCase();
    const found = s.workspaces.filter((w) => w.id === arg || w.name.toLowerCase() === query);
    const match = found.length ? found : s.workspaces.filter((w) => w.name.toLowerCase().includes(query) || (w.path || '').toLowerCase().includes(query));
    if (match.length !== 1) return toast(match.length ? `${match.length} workspaces match "${arg}"; pick one from the list.` : `No workspace matches "${arg}".`, 'error');
    if (match[0].id !== s.ws) session.switchWorkspace(match[0].id);
    return undefined;
  }

  // ---------------------------------------------------------------- adding one

  const input = h('input', {
    id: 'ws-path-input', type: 'text', spellcheck: 'false', autocomplete: 'off', autocapitalize: 'off',
    placeholder: '~/Projects/app or /absolute/path', role: 'combobox', 'aria-expanded': 'false', 'aria-controls': 'ws-suggest', 'aria-autocomplete': 'list',
  });
  const suggest = h('ul', { class: 'suggest', id: 'ws-suggest', role: 'listbox', hidden: true });
  const status = h('div', { class: 'settings-status', id: 'ws-status', role: 'status', 'aria-live': 'polite' });
  const save = h('button', { class: 'primary', id: 'ws-add-save', type: 'submit' }, 'Add');
  const form = h('form', { class: 'settings ticks', id: 'ws-form', role: 'dialog', 'aria-modal': 'true', 'aria-labelledby': 'ws-add-title', novalidate: true },
    h('header', null, h('span', { class: 'label', id: 'ws-add-title', text: 'Add workspace' }),
      h('button', { class: 'icon', id: 'ws-add-close', type: 'button', 'aria-label': 'Close', onclick: () => close() }, '×')),
    h('div', { class: 'settings-body' },
      h('p', { class: 'settings-note' }, 'A folder the agent works in. It keeps its own sessions in ', h('code', { text: '.harness/sessions' }),
        ' inside the folder, and runs use its ', h('code', { text: '.env' }), '.'),
      h('label', { class: 'field' }, h('span', { class: 'label', text: 'Folder' }), input),
      suggest, status),
    h('footer', null,
      h('span', { class: 'settings-path' }, h('kbd', { text: 'Tab' }), ' completes · ', h('kbd', { text: '↑' }), h('kbd', { text: '↓' }), ' choose'),
      h('button', { class: 'act', id: 'ws-add-cancel', type: 'button', onclick: () => close() }, 'Cancel'),
      save));
  const overlay = h('div', { class: 'overlay', id: 'ws-add', hidden: true, onmousedown: (event) => { if (event.target === overlay) close(); } }, form);
  cockpit.ui.mount('overlays', { id: 'ws-add', order: 40, node: overlay });
  cockpit.contribute('overlay', { id: 'ws-add', order: 40, modal: true, isOpen: () => !overlay.hidden, close: () => close() });
  const ui = { ticket: 0, suggestions: [], cursor: -1, timer: 0 };
  cockpit.onDispose(() => clearTimeout(ui.timer));

  function add() {
    service('overlays')?.closeTop?.();
    overlay.hidden = false;
    input.value = '';
    say('');
    show([]);
    input.focus();
    suggestFolders();
  }

  function close() {
    ui.ticket++;
    clearTimeout(ui.timer);
    overlay.hidden = true;
  }

  function say(text, kind = '') {
    status.textContent = text;
    status.dataset.kind = kind;
  }

  // suggestFolders lists the folders the typed path can continue with.
  async function suggestFolders() {
    const ticket = ++ui.ticket;
    const typed = input.value;
    const prefix = typed.trim() ? typed : '~/';
    let list = [];
    try {
      list = await cockpit.api(`/api/folders?prefix=${encodeURIComponent(prefix)}`);
    } catch {
      // Suggestions are a convenience; typing a path works without them.
    }
    if (ticket !== ui.ticket || overlay.hidden) return;
    show(list);
  }

  function show(list) {
    ui.suggestions = list;
    ui.cursor = -1;
    suggest.hidden = !list.length;
    input.setAttribute('aria-expanded', String(!!list.length));
    suggest.replaceChildren(...list.map((folder, n) => {
      // The folder's own name leads; long parents shorten, never the name.
      const cut = folder.lastIndexOf('/');
      return h('li', {
        role: 'option', id: `ws-suggest-${n}`, 'aria-selected': 'false', title: folder,
        onmousedown: (event) => {
          event.preventDefault(); // keep focus in the field
          accept(n);
        },
      }, h('b', { text: `${folder.slice(cut + 1)}/` }), h('small', { text: `\u200e${folder.slice(0, cut + 1)}\u200e` }));
    }));
  }

  function moveSuggestion(step) {
    const n = ui.suggestions.length;
    if (!n) return;
    // -1 is the text as typed; moving past either end of the list returns to it.
    let next = ui.cursor + step;
    if (next >= n) next = -1;
    else if (next < -1) next = n - 1;
    ui.cursor = next;
    suggest.querySelectorAll('li').forEach((li, i) => li.setAttribute('aria-selected', String(i === ui.cursor)));
    suggest.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' });
    input.setAttribute('aria-activedescendant', ui.cursor >= 0 ? `ws-suggest-${ui.cursor}` : '');
  }

  // accept takes a folder and lists what is inside it.
  function accept(n) {
    const folder = ui.suggestions[n];
    if (!folder) return;
    input.value = `${folder}/`;
    say('');
    suggestFolders();
  }

  async function submit() {
    const folder = input.value.trim();
    if (!folder) {
      say('Enter the folder\'s path', 'error');
      return;
    }
    const ticket = ++ui.ticket;
    save.disabled = true;
    say('Adding…');
    let added;
    try {
      added = await cockpit.api('/api/workspaces', { method: 'POST', body: { path: folder } });
    } catch (error) {
      if (ticket === ui.ticket) say(error.message, 'error');
      return;
    } finally {
      save.disabled = false;
    }
    if (ticket !== ui.ticket) return;
    close();
    toast(`Workspace ${added.name} added`);
    session.addedWorkspace(added);
  }

  cockpit.listen(form, 'submit', (event) => {
    event.preventDefault();
    submit();
  });
  cockpit.listen(input, 'input', () => {
    say('');
    clearTimeout(ui.timer);
    ui.timer = setTimeout(suggestFolders, 120);
  });
  cockpit.listen(input, 'keydown', (event) => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      moveSuggestion(event.key === 'ArrowDown' ? 1 : -1);
    } else if (event.key === 'Tab' && !event.shiftKey && ui.suggestions.length) {
      event.preventDefault();
      accept(ui.cursor >= 0 ? ui.cursor : 0);
    } else if (event.key === 'Enter' && ui.cursor >= 0) {
      event.preventDefault();
      accept(ui.cursor);
    }
  });

  cockpit.provide('workspaces', { openMenu, add, command, render });
  cockpit.commands.register({
    name: 'workspace', args: '<name>', help: 'Switch to another workspace', order: 170,
    shown: () => (state().workspaces || []).length > 1, run: (arg) => command(String(arg)),
    complete: () => (state().workspaces || []).map((w) => ({ value: w.id, label: w.name, detail: [w.id === state().ws ? 'current' : '', w.path].filter(Boolean).join(' · ') })),
  });
  cockpit.keys.register({ key: 'w', run: () => openMenu(button) });
  cockpit.contribute('help.keys', { keys: ['W'], text: 'Switch workspace', order: 130 });
  cockpit.contribute('palette.provider', {
    id: 'workspaces', order: 500,
    items: () => [
      ...(state().workspaces || []).map((w) => ({
        group: 'Workspaces', icon: w.id === state().ws ? '●' : w.running ? '■' : '·', label: w.name, hint: w.id === state().ws ? 'current' : '',
        detail: w.path, order: 500, run: () => session.switchWorkspace(w.id),
      })),
      { group: 'Workspaces', icon: '+', label: 'Add workspace…', order: 501, run: add },
    ],
  });
  cockpit.on('session:workspaces', () => cockpit.render());
  render();
}
