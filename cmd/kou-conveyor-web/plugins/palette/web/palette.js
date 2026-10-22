// palette: the command palette (⌘K). Its items come from plugins: fixed
// ones through "palette" (cockpit.palette.register: { group, icon, label,
// hint, detail, order, shown(view), run() }), lists that change through
// "palette.provider" ({ id, order, items(view) → items }), and every
// command a plugin (not the built-in ones) declares, under Plugin commands.
export default function activate(cockpit) {
  const { h } = cockpit;
  const session = cockpit.use('session');
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const ui = { items: [], shown: [], more: 0, cursor: 0 };

  const input = h('input', {
    id: 'palette-input', type: 'text', placeholder: 'Type a command or a session…', autocomplete: 'off', spellcheck: 'false',
    role: 'combobox', 'aria-expanded': 'true', 'aria-controls': 'palette-list',
  });
  const list = h('ul', { id: 'palette-list', role: 'listbox' });
  const overlay = h('div', { class: 'overlay', id: 'palette', hidden: true, onmousedown: (event) => { if (event.target === overlay) close(); } },
    h('div', { class: 'palette ticks', role: 'dialog', 'aria-label': 'Command palette' }, input, list,
      h('footer', null, h('span', null, h('kbd', { text: '↑' }), h('kbd', { text: '↓' }), ' move'), h('span', null, h('kbd', { text: '↵' }), ' run'), h('span', null, h('kbd', { text: 'Esc' }), ' close'))));
  cockpit.ui.mount('overlays', { id: 'palette', order: 60, node: overlay });
  cockpit.contribute('overlay', { id: 'palette', order: 60, modal: true, isOpen: () => !overlay.hidden, close: () => close() });

  const button = h('button', { class: 'icon wide', id: 'palette-open', type: 'button', title: 'Command palette (⌘K)', onclick: () => open() }, h('kbd', { text: '⌘K' }));
  cockpit.ui.mount('bar.end', { id: 'palette-open', order: 50, node: button });

  // items gathers what plugins offer now.
  function items() {
    const view = session.summary?.() || null;
    const out = [];
    for (const item of cockpit.contributions('palette')) {
      if (item.shown && !cockpit.safely(() => item.shown(view))) continue;
      out.push({ ...item, run: () => cockpit.safely(item.run) });
    }
    for (const provider of cockpit.contributions('palette.provider')) {
      const more = cockpit.safely(() => provider.items(view)) || [];
      for (const item of more) if (item) out.push({ order: provider.order ?? 0, ...item, run: () => cockpit.safely(item.run) });
    }
    // The commands plugins declare, besides the cockpit's own.
    const commands = service('commands');
    for (const c of commands?.list?.() || []) {
      if (c.order < 1000 || !commands.shown(c, view)) continue;
      out.push({
        group: 'Plugin commands', icon: '/', label: `/${c.name}${c.args ? ` ${c.args}` : ''}`, hint: c.plugin, detail: c.help, order: 450,
        run: () => {
          const composer = service('composer');
          composer?.set?.(`/${c.name}${c.args ? ' ' : ''}`, { focus: true });
          if (!c.args) composer?.submit?.();
        },
      });
    }
    // By order; items of one order stay in the order they came.
    return out.map((item, n) => ({ item, n })).sort((a, b) => (a.item.order ?? 0) - (b.item.order ?? 0) || a.n - b.n).map((x) => x.item);
  }

  function score(query, text) {
    if (!query) return 1;
    text = String(text || '').toLowerCase();
    const direct = text.indexOf(query);
    if (direct >= 0) return 100 - direct + (direct === 0 || text[direct - 1] === ' ' ? 50 : 0);
    let at = 0;
    let gaps = 0;
    for (const ch of query) {
      const next = text.indexOf(ch, at);
      if (next < 0) return 0;
      gaps += next - at;
      at = next + 1;
    }
    return Math.max(1, 40 - gaps);
  }

  // The palette gives the focus back to where it was when it closes.
  let before = null;

  function open() {
    // A menu or the model picker open would cover the palette.
    service('menu')?.close?.();
    service('models')?.closePicker?.();
    before = document.activeElement !== document.body ? document.activeElement : null;
    ui.items = items();
    input.value = '';
    overlay.hidden = false;
    filter();
    input.focus();
  }

  function close() {
    if (overlay.hidden) return;
    overlay.hidden = true;
    if (overlay.contains(document.activeElement)) {
      if (before?.isConnected) before.focus();
      else document.activeElement.blur();
    }
    before = null;
  }

  function filter() {
    const query = input.value.trim().toLowerCase();
    const found = ui.items
      .map((item) => ({ item, score: Math.max(score(query, item.label), score(query, `${item.group} ${item.detail || ''}`) * 0.5) }))
      .filter((x) => x.score > 0)
      .sort((a, b) => (query ? b.score - a.score : 0))
      .map((x) => x.item);
    ui.shown = found.slice(0, 60);
    ui.more = found.length - ui.shown.length;
    ui.cursor = 0;
    render();
  }

  function render() {
    const nodes = [];
    let group = '';
    ui.shown.forEach((item, n) => {
      if (item.group !== group && !input.value.trim()) {
        group = item.group;
        nodes.push(h('li', { class: 'group', role: 'presentation', text: group }));
      }
      nodes.push(h('li', {
        role: 'option', id: `palette-${n}`, 'aria-selected': String(n === ui.cursor),
        onmousemove: () => { if (ui.cursor !== n) select(n, false); },
        onclick: () => choose(n),
      }, h('span', { class: 'i', text: item.icon || '' }), h('span', { class: 'l', text: item.label }), item.hint ? h('kbd', { text: item.hint }) : h('span')));
    });
    if (!ui.shown.length) nodes.push(h('li', { class: 'none', role: 'presentation', text: 'Nothing matches' }));
    // What the list leaves out, it says: the query finds it.
    else if (ui.more) nodes.push(h('li', { class: 'none', role: 'presentation', text: `…${ui.more} more — refine the query` }));
    list.replaceChildren(...nodes);
    select(ui.cursor, true);
  }

  // select puts the cursor on an item, in place: the pointer moves it to
  // the item under it, and only the arrows scroll the list. The field names
  // the item the cursor is on, and none when nothing matches.
  function select(n, scroll) {
    list.querySelector(`#palette-${ui.cursor}`)?.setAttribute('aria-selected', 'false');
    ui.cursor = n;
    const li = list.querySelector(`#palette-${n}`);
    if (!li) {
      input.removeAttribute('aria-activedescendant');
      return;
    }
    li.setAttribute('aria-selected', 'true');
    input.setAttribute('aria-activedescendant', li.id);
    if (scroll) li.scrollIntoView({ block: 'nearest' });
  }

  function choose(n) {
    const item = ui.shown[n];
    close();
    item?.run();
  }

  cockpit.listen(input, 'input', filter);
  cockpit.listen(input, 'keydown', (event) => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      const n = ui.shown.length;
      if (n) select((ui.cursor + (event.key === 'ArrowDown' ? 1 : n - 1)) % n, true);
    } else if (event.key === 'Enter') {
      event.preventDefault();
      choose(ui.cursor);
    } else if (event.key === 'Tab') {
      // The palette has one field: Tab would take the focus out of it, under
      // the dimmed page.
      event.preventDefault();
    }
  });

  cockpit.keys.register({ key: 'Mod+k', global: true, priority: 90, run: () => (overlay.hidden ? open() : close()) });
  cockpit.contribute('help.keys', { keys: ['⌘', 'K'], text: 'Command palette', order: 90 });
  cockpit.provide('palette', { open, close, items });
}
