// session-list: the Sessions view (layout.view "sessions"): in the rail, the
// workspace's sessions with a filter and a button for a new session; the
// stage shows the session in view. The pinned sessions come first, in the
// order the user drags them into (or moves them with ⌥↑ ⌥↓); the others by
// when the user last wrote to them — the server's order, which the agent at
// work does not change. Rows glide to their new places. Its rail's tools
// are the slot sessions.tools (the workspace switcher heads them). It
// provides the session-list service: rename(ws, id, place), which edits a
// title in place, in the rail or in the header, move(step), and render().
//
// Other plugins put rows of their own among the sessions not pinned, by
// when they changed, through the point session-list.rows:
//   { id, order, rows(ws) → [{ id, glyph, title, meta, at, href, current,
//     running, open(), menu() → menu items }] }
// (the canvases do). The sessions of the agents of canvas nodes stay out
// of the list until it is asked to show them, or a filter finds them.
const SEARCH = '<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="7" cy="7" r="4.5" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M10.5 10.5 14 14" stroke="currentColor" stroke-width="1.4"/></svg>';

export default function activate(cockpit) {
  const { h, svg, fmt } = cockpit;
  const session = cockpit.use('session');
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const view = () => session.view?.();
  let renaming = null; // the session whose title is edited in place

  const tools = h('div', { class: 'rail-tools' });
  const newButton = h('button', { class: 'new', id: 'new-session', type: 'button', onclick: () => session.newSession() }, h('span', { text: 'New session' }), h('kbd', { text: 'N' }));
  const filter = h('input', { id: 'session-filter', type: 'search', placeholder: 'Filter sessions', autocomplete: 'off', spellcheck: 'false', 'aria-label': 'Filter sessions' });
  const search = h('label', { class: 'search' }, svg(SEARCH), filter, h('kbd', { text: '/' }));
  const count = h('span', { id: 'session-count', text: '0' });
  const list = h('nav', { class: 'sessions', id: 'sessions' });
  const rail = h('div', { class: 'rail-view', data: { view: 'sessions' } },
    tools, h('div', { class: 'rail-label' }, h('span', { text: 'Sessions' }), count), list);
  cockpit.ui.slot('sessions.tools', tools);
  cockpit.ui.mount('sessions.tools', { id: 'new-session', order: 20, node: newButton });
  cockpit.ui.mount('sessions.tools', { id: 'session-filter', order: 30, node: search });

  // The Sessions view: shown from the start, and when its tab is picked it
  // goes back to the session it showed.
  cockpit.contribute('layout.view', {
    id: 'sessions', title: 'Sessions', order: 10, rail,
    select: () => {
      const v = view();
      const state = session.state;
      const hash = v && !v.fresh ? session.sessionHash(v.ws, v.id) : session.workspaceHash(state.ws);
      if (location.hash !== hash) history.pushState(null, '', hash);
      service('layout')?.show?.('sessions');
      if (v && v.ws !== state.ws) session.routeSessions();
    },
  });

  let drag = null; // the pinned session being dragged, by ID
  let lastQuery = '';

  // The sessions of the agents of canvas nodes stay out of the list unless
  // asked for (or searched for): their canvas shows them.
  const AGENTS = 'session-list.canvas-agents';
  const showAgents = () => cockpit.prefs.get(AGENTS, false) === true;
  const at = (value) => Date.parse(value || '') || 0;

  // extraRows are the rows plugins put among the sessions
  // (session-list.rows) for the workspace, as the filter leaves them.
  function extraRows(ws, query) {
    const out = [];
    for (const source of cockpit.contributions('session-list.rows', { unique: 'id' })) {
      const rows = cockpit.safely(() => source.rows?.(ws));
      for (const r of Array.isArray(rows) ? rows : []) {
        if (!r?.id) continue;
        if (query && !String(r.title || '').toLowerCase().includes(query) && !String(r.id).includes(query)) continue;
        out.push({ ...r, key: `${source.id}:${r.id}` });
      }
    }
    return out.sort((a, b) => at(b.at) - at(a.at));
  }

  function render() {
    // An edit in place survives until it is committed, and a drag until it
    // drops; the list catches up then.
    if (renaming || drag) return;
    const v = view();
    const state = session.state;
    const sessions = state.sessions || [];
    const query = filter.value.trim().toLowerCase();
    // Rows that moved glide to their new places — unless the filter moved
    // them, as it does with every letter typed.
    const glide = query === lastQuery ? positions() : null;
    lastQuery = query;
    const inView = (s) => !!v && s.id === v.id && v.ws === state.ws;
    const agents = sessions.filter((s) => s.canvas && !inView(s)).length;
    const hideAgents = !query && !showAgents();
    const shown = sessions.filter((s) => (!query || (s.title || '').toLowerCase().includes(query) || s.id.includes(query)) && !(hideAgents && s.canvas && !inView(s)));
    // The other rows go among the sessions that are not pinned, by when
    // they changed.
    const extras = extraRows(state.ws, query);
    const items = [];
    let next = 0;
    for (const s of shown) {
      if (!s.pinned) while (next < extras.length && at(extras[next].at) >= at(s.updated_at)) items.push({ row: extras[next++] });
      items.push({ session: s });
    }
    while (next < extras.length) items.push({ row: extras[next++] });
    const total = sessions.length - (hideAgents ? agents : 0) + (query ? extraRows(state.ws, '').length : extras.length);
    count.textContent = query ? `${items.length}/${total}` : String(items.length);
    const pinned = (item) => !!item.session?.pinned;
    const grouped = !query && items.some(pinned) && items.some((item) => !pinned(item));
    const nodes = [];
    let n = -1;
    items.forEach((item, i) => {
      if (grouped && (i === 0 || pinned(item) !== pinned(items[i - 1]))) {
        nodes.push(h('div', { class: 'session-group', text: pinned(item) ? 'Pinned' : 'Recent' }));
      }
      if (item.row) {
        nodes.push(extraRow(item.row));
        return;
      }
      const s = item.session;
      n++;
      const title = s.title || 'Untitled session';
      // Pinned sessions are dragged into another order, while the whole
      // list shows.
      const movable = s.pinned && !query;
      nodes.push(h('div', {
        class: 'session-row', draggable: movable ? 'true' : null,
        data: { id: s.id, pinned: s.pinned ? 'true' : null },
      },
        h('a', {
          class: 'session', href: session.sessionHash(state.ws, s.id), 'aria-current': v && s.id === v.id && v.ws === state.ws ? 'true' : null,
          data: { running: s.run_id ? 'true' : null, id: s.id }, title, draggable: movable ? 'false' : null,
          'aria-keyshortcuts': movable ? 'Alt+ArrowUp Alt+ArrowDown' : null,
        },
        h('span', { class: 'n', text: s.pinned ? '◆' : String(n + 1).padStart(2, '0') }),
        h('span', { class: 't', text: title }),
        h('span', { class: 'm' },
          [s.canvas ? '◧' : '', s.run_id ? 'running' : fmt.ago(s.updated_at), s.size ? fmt.bytes(s.size) : '', s.id.slice(0, 8)].filter(Boolean).join(' · '),
          // What waits for the session's agent.
          s.queued && h('span', { class: 'queued', data: { paused: s.queue_paused ? 'true' : null }, text: s.queue_paused ? ` · ${s.queued} paused` : ` · +${s.queued} queued` }))),
        h('button', {
          class: 'more', type: 'button', title: 'Session actions', 'aria-label': `Actions for ${title}`,
          'aria-haspopup': 'menu', 'aria-expanded': 'false',
          onclick: (event) => {
            event.preventDefault();
            event.stopPropagation();
            service('menu')?.open?.(event.currentTarget, session.menuItems(state.ws, s.id, 'rail'));
          },
        }, h('span', { 'aria-hidden': 'true', text: '⋯' }))));
    });
    if (!nodes.length) {
      nodes.push(h('p', { class: 'sessions-empty', text: query ? 'No sessions match.' : 'Sessions you run appear here.' }));
    }
    if (!query && agents) {
      const on = showAgents();
      nodes.push(h('button', {
        class: 'sessions-toggle', type: 'button',
        title: on ? 'Their canvases show them' : 'The sessions of the agents on the canvases',
        onclick: () => {
          cockpit.prefs.set(AGENTS, on ? null : true);
          render();
        },
      }, on ? 'Hide the canvas agents' : `Show ${agents} canvas agent${agents === 1 ? '' : 's'}`));
    }
    const focused = document.activeElement?.closest?.('a.session')?.getAttribute('href');
    list.replaceChildren(...nodes);
    if (focused) list.querySelector(`a.session[href="${CSS.escape(focused)}"]`)?.focus();
    if (glide) animate(glide);
  }

  // extraRow draws a row a plugin put in the list: its link opens what it
  // stands for, its ⋯ the plugin's menu for it.
  function extraRow(r) {
    const title = r.title || 'Untitled';
    return h('div', { class: 'session-row', data: { id: r.key, row: 'true' } },
      h('a', {
        class: 'session', href: r.href || '#', title, 'aria-current': r.current ? 'true' : null,
        data: { running: r.running ? 'true' : null, row: 'true' },
        onclick: (event) => {
          if (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
          event.preventDefault();
          service('layout')?.rail?.(false);
          cockpit.safely(() => r.open?.());
        },
      },
      h('span', { class: 'n', text: r.glyph || '◇' }),
      h('span', { class: 't', text: title }),
      h('span', { class: 'm', text: r.meta || '' })),
      typeof r.menu === 'function' ? h('button', {
        class: 'more', type: 'button', title: 'Actions', 'aria-label': `Actions for ${title}`,
        'aria-haspopup': 'menu', 'aria-expanded': 'false',
        onclick: (event) => {
          event.preventDefault();
          event.stopPropagation();
          const items = cockpit.safely(() => r.menu()) || [];
          if (items.length) service('menu')?.open?.(event.currentTarget, items);
        },
      }, h('span', { 'aria-hidden': 'true', text: '⋯' })) : null);
  }

  // positions are where the rows are now, by session.
  function positions() {
    const out = new Map();
    for (const row of list.querySelectorAll('.session-row')) out.set(row.dataset.id, row.getBoundingClientRect().top);
    return out;
  }

  // animate has rows that moved glide from where they were.
  function animate(before) {
    if (matchMedia('(prefers-reduced-motion: reduce)').matches) return;
    for (const row of list.querySelectorAll('.session-row')) {
      const was = before.get(row.dataset.id);
      if (was === undefined) continue;
      const by = was - row.getBoundingClientRect().top;
      if (Math.abs(by) < 1) continue;
      row.animate([{ transform: `translateY(${by}px)` }, { transform: 'none' }], { duration: 260, easing: 'cubic-bezier(0.22, 1, 0.36, 1)' });
    }
  }

  // ---------------------------------------------------------------- the pinned order

  const pinnedIDs = () => (session.state.sessions || []).filter((s) => s.pinned).map((s) => s.id);

  // reorder puts a pinned session at an index among the pinned ones: at
  // once here, then on the server, whose list follows.
  async function reorder(id, index) {
    const state = session.state;
    const order = pinnedIDs();
    const from = order.indexOf(id);
    if (from < 0) return;
    order.splice(from, 1);
    order.splice(Math.max(0, Math.min(index, order.length)), 0, id);
    if (order.join() === pinnedIDs().join()) return;
    const byID = new Map(state.sessions.map((s) => [s.id, s]));
    state.sessions = [...order.map((pid) => byID.get(pid)), ...state.sessions.filter((s) => !s.pinned)];
    render();
    try {
      await cockpit.api('/pins', { method: 'PUT', body: { order } });
    } catch (error) {
      // A page read live from a checkout can be newer than the server
      // running: one that does not know the order yet says so, once.
      cockpit.toast(error.status === 404
        ? 'The order was not saved: the server running is older than this page. Restart kou-conveyor-web to keep it.'
        : `The order was not saved: ${error.message}`, 'error', 'pins');
    }
    session.refreshSessions();
  }

  const clearDrop = () => {
    for (const row of list.querySelectorAll('[data-drop]')) delete row.dataset.drop;
  };

  cockpit.listen(list, 'dragstart', (event) => {
    const row = event.target.closest?.('.session-row[draggable="true"]');
    if (!row) return;
    drag = row.dataset.id;
    row.dataset.dragging = 'true';
    event.dataTransfer.effectAllowed = 'move';
    event.dataTransfer.setData('text/plain', row.querySelector('.t')?.textContent || '');
    service('menu')?.close?.();
  });
  // Over a pinned row, the line shows where the session goes: before it or
  // after, by the half the pointer is on. Both events say so, as a drop
  // target must.
  const over = (event) => {
    const row = event.target.closest?.('.session-row[data-pinned]');
    if (!drag || !row) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = 'move';
    const box = row.getBoundingClientRect();
    const where = event.clientY < box.top + box.height / 2 ? 'before' : 'after';
    if (row.dataset.drop === where) return;
    clearDrop();
    if (row.dataset.id !== drag) row.dataset.drop = where;
  };
  cockpit.listen(list, 'dragenter', over);
  cockpit.listen(list, 'dragover', over);
  // Leaving a row for its own title fires dragleave too, without saying
  // where to: the line goes only once the pointer is out of the list.
  cockpit.listen(list, 'dragleave', (event) => {
    const box = list.getBoundingClientRect();
    const out = event.clientX <= box.left || event.clientX >= box.right || event.clientY <= box.top || event.clientY >= box.bottom;
    if (out) clearDrop();
  });
  cockpit.listen(list, 'drop', (event) => {
    const row = event.target.closest('.session-row[data-pinned]');
    const id = drag;
    if (!id || !row) return;
    event.preventDefault();
    const order = pinnedIDs().filter((pid) => pid !== id);
    let to = order.indexOf(row.dataset.id);
    if (row.dataset.drop === 'after') to++;
    clearDrop();
    drag = null;
    list.querySelector('[data-dragging]')?.removeAttribute('data-dragging');
    if (to >= 0) reorder(id, to);
    else render();
  });
  cockpit.listen(list, 'dragend', () => {
    const was = drag;
    drag = null;
    clearDrop();
    for (const row of list.querySelectorAll('[data-dragging]')) delete row.dataset.dragging;
    if (was) render();
  });
  // ⌥↑ ⌥↓ move the pinned session that has the focus.
  cockpit.listen(list, 'keydown', (event) => {
    if (!event.altKey || (event.key !== 'ArrowUp' && event.key !== 'ArrowDown')) return;
    const row = event.target.closest?.('.session-row[draggable="true"]');
    if (!row) return;
    event.preventDefault();
    const at = pinnedIDs().indexOf(row.dataset.id);
    reorder(row.dataset.id, at + (event.key === 'ArrowUp' ? -1 : 1));
  });
  cockpit.on('session:sessions', render);
  cockpit.on('point:session-list.rows', render);
  cockpit.on('session:view', render);
  cockpit.on('session:workspace', () => { filter.value = ''; });

  cockpit.listen(filter, 'input', render);
  cockpit.listen(filter, 'keydown', (event) => {
    if (event.key === 'Enter') {
      event.preventDefault();
      list.querySelector('.session')?.click();
    } else if (event.key === 'Escape') {
      event.stopPropagation(); // clearing the filter is not the first Esc of an Esc Esc
      event.target.value = '';
      render();
      event.target.blur();
    }
  });
  cockpit.listen(list, 'click', (event) => {
    const link = event.target.closest('a.session');
    // A plugin's row opens what it stands for itself.
    if (!link || link.dataset.row || event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
    event.preventDefault();
    service('layout')?.rail?.(false);
    const id = link.dataset.id;
    if (id !== view().id || view().ws !== session.state.ws) session.openSession(id);
  });
  cockpit.listen(list, 'scroll', () => service('menu')?.close?.(), { passive: true });

  // ---------------------------------------------------------------- renaming in place

  // rename edits a session's title in place: in the header (place "crumb")
  // for the session in view, else in its row of the rail.
  function rename(ws, id, place = 'crumb') {
    const v = view();
    const state = session.state;
    if (renaming || ws !== state.ws || (v.id === id && v.fresh)) return;
    // The header shows only the open session; the rail row may be filtered
    // out or, on a narrow screen, off-canvas.
    const row = list.querySelector(`.session-row[data-id="${CSS.escape(id)}"] .t`);
    const crumb = document.getElementById('crumb-title');
    const target = v.id === id && (place === 'crumb' || !row?.offsetParent) && crumb ? crumb : row;
    if (!target) return;
    const current = v.id === id ? (v.title || session.firstPrompt(v)) : (session.sessionInfo(ws, id)?.title || '');
    const input = h('input', {
      class: 'rename', type: 'text', maxlength: 120, value: current, spellcheck: 'false',
      'aria-label': 'Session title', placeholder: 'Title (empty: the first prompt)',
    });
    let done = false;
    const finish = (save) => {
      if (done) return;
      done = true;
      renaming = null;
      const title = input.value.trim();
      input.remove();
      target.hidden = false;
      if (save && title !== current) session.rename(ws, id, title);
      render();
      cockpit.render();
    };
    input.addEventListener('keydown', (event) => {
      event.stopPropagation();
      if (event.key === 'Enter') {
        event.preventDefault();
        finish(true);
      } else if (event.key === 'Escape') {
        event.preventDefault();
        finish(false);
      }
    });
    input.addEventListener('blur', () => finish(true));
    // The rail's title sits inside a link; typing into it must not follow it.
    input.addEventListener('click', (event) => {
      event.preventDefault();
      event.stopPropagation();
    });
    renaming = id;
    // The title stays in the page, hidden, so everything that updates it by
    // ID keeps working while the edit is open.
    target.hidden = true;
    target.after(input);
    input.focus();
    input.select();
  }

  // moveSession opens the next or previous session of the list, as it
  // shows them.
  function move(step) {
    const sessions = (session.state.sessions || []).filter((s) => !s.canvas || showAgents() || s.id === view().id);
    if (!sessions.length) return;
    const at = sessions.findIndex((s) => s.id === view().id);
    const next = sessions[Math.max(0, Math.min(sessions.length - 1, at < 0 ? 0 : at + step))];
    if (next && next.id !== view().id) session.openSession(next.id);
  }

  cockpit.provide('session-list', { rename, move, render });
  cockpit.keys.register({ key: '/', views: ['sessions'], run: () => filter.focus() });
  cockpit.keys.register({ key: 'j', run: () => move(1) });
  cockpit.keys.register({ key: 'k', run: () => move(-1) });
  cockpit.contribute('help.keys', { keys: ['J', ' ', 'K'], text: 'Next / previous session', order: 150 });
  cockpit.contribute('help.keys', { keys: ['/'], text: 'Filter sessions', order: 160 });
  cockpit.contribute('help.keys', { keys: ['⌥', '↑', ' ', '⌥', '↓'], text: 'Move a pinned session up or down, on its row (or drag it)', order: 165 });
  render();
}
