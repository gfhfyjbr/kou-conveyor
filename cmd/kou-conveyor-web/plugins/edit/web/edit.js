// edit: editing a prompt rewinds the session to before it and runs the
// edited prompt from there. The editor takes the prompt's place in the
// timeline (timeline.editor) and lives in the view, so drawing the prompt
// again keeps what was typed. Esc Esc (the session's) edits the last
// prompt, the Edit button any other, and /edit n prompt n. It provides the
// edit service: begin(id), editLast(), close(v), drop(v, message), command(n).
export default function activate(cockpit) {
  const { h } = cockpit;
  const session = cockpit.use('session');
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const view = () => session.view?.();
  const toast = (text, kind = 'info') => cockpit.toast(text, kind);

  function editLast() {
    const entry = session.lastPrompt(view());
    if (entry) begin(entry.id);
  }

  function begin(id) {
    const v = view();
    const entry = v.entries.get(id);
    if (entry?.kind !== 'user') return;
    const blocked = session.runBlocked(v);
    if (blocked) return toast(blocked);
    if (v.edit?.id !== id) {
      if (v.edit) close(v);
      v.edit = createEditor(entry, v.users.get(id));
      session.touch(v, id);
      service('timeline')?.flushNow?.();
    }
    const { input } = v.edit;
    size(input);
    service('timeline')?.node?.(id)?.scrollIntoView({ block: 'nearest' });
    input.focus({ preventScroll: true });
    input.setSelectionRange(input.value.length, input.value.length);
    return undefined;
  }

  function createEditor(entry, n) {
    const input = h('textarea', { class: 'edit-input', rows: 1, spellcheck: 'true', 'aria-label': n ? `Edit prompt ${n}` : 'Edit the prompt' });
    input.value = entry.text;
    const note = h('span', { class: 'edit-note' });
    const cancel = h('button', { class: 'act', type: 'button', title: 'Keep the prompt as it was' }, h('span', { text: 'Cancel' }), h('kbd', { text: 'Esc' }));
    const run = h('button', { class: 'run', type: 'button' }, h('span', { text: 'Run' }), h('kbd', { text: '↵' }));
    const edit = { id: entry.id, input, note, run, node: h('div', { class: 'edit' }, input, h('div', { class: 'edit-row' }, note, h('span', { class: 'edit-buttons' }, cancel, run))) };
    // A reload hands the editor to a new view; handlers act on whichever
    // view holds it now.
    const holder = () => (view()?.edit === edit ? view() : null);
    // The prompt's images come with it, and more can be pasted.
    const pictures = service('images');
    if (pictures?.attach) {
      edit.images = pictures.attach({
        input,
        onChange: () => {
          const v = holder();
          if (v) renderEdit(v);
        },
      });
      edit.node.insertBefore(edit.images.strip, input);
      edit.images.load(entry.images, (i) => pictures.imageURL(view(), entry, i));
    }
    input.addEventListener('input', () => {
      size(input);
      const v = holder();
      if (v) renderEdit(v);
    });
    input.addEventListener('keydown', (event) => {
      if (event.key === 'Escape' && !event.isComposing) {
        event.preventDefault();
        event.stopPropagation(); // not the first Esc of an Esc Esc
        const v = holder();
        if (v) close(v, { focusComposer: true });
      } else if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
        event.preventDefault();
        if (holder()) submit();
      } else if (event.altKey && (event.key === 'ArrowUp' || event.key === 'ArrowDown')) {
        event.preventDefault();
        service('effort')?.cycle?.(event.key === 'ArrowUp' ? 1 : -1);
      }
    });
    cancel.addEventListener('click', () => {
      const v = holder();
      if (v) close(v, { focusComposer: true });
    });
    run.addEventListener('click', () => { if (holder()) submit(); });
    return edit;
  }

  function size(input) {
    input.style.height = 'auto';
    input.style.height = `${Math.min(input.scrollHeight, Math.round(window.innerHeight * 0.5))}px`;
  }

  // renderEdit says what running the edit replaces and whether it can run.
  function renderEdit(v) {
    const edit = v.edit;
    if (!edit) return;
    const at = v.order.indexOf(edit.id);
    const after = at < 0 ? 0 : v.order.length - 1 - at;
    edit.note.textContent = after ? `Replaces the ${after === 1 ? 'entry' : `${after} entries`} below · changes to files stay` : '';
    edit.note.title = 'The session goes back to before this prompt, and the edited prompt runs from there. Files the agent changed keep their changes.';
    const blocked = session.runBlocked(v);
    edit.run.disabled = !edit.input.value.trim() || !!blocked;
    edit.run.title = blocked || 'Rewind the session to this prompt and run the edited one';
  }
  cockpit.on('render', () => { const v = view(); if (v?.edit) renderEdit(v); });

  function close(v, { focusComposer = false } = {}) {
    const edit = v.edit;
    if (!edit) return;
    v.edit = null;
    edit.images?.destroy?.();
    if (v !== view()) return;
    session.touch(v, edit.id);
    if (focusComposer) service('composer')?.input?.focus({ preventScroll: true });
  }

  // drop closes an edit the session moved past. What was typed goes to the
  // composer when that is empty, so it is not lost; with always, even when
  // it is the prompt as it was.
  function drop(v, message, { always = false } = {}) {
    const edit = v.edit;
    if (!edit) return;
    v.edit = null;
    edit.images?.destroy?.();
    if (v === view()) session.touch(v, edit.id);
    const text = edit.input.value.trim();
    const composer = service('composer');
    const kept = text && (always || text !== v.entries.get(edit.id)?.text) && composer && !composer.value().trim();
    if (kept) composer.set(text);
    if (message) toast(kept ? `${message} What you typed is in the composer.` : message, 'error');
  }

  async function submit() {
    const v = view();
    const edit = v.edit;
    if (!edit) return;
    const text = edit.input.value.trim();
    if (!text) return;
    const blocked = session.runBlocked(v);
    if (blocked) return toast(blocked);
    const at = v.order.indexOf(edit.id);
    if (at < 0) return close(v);
    const removed = v.order.slice(at);
    // The session goes back to before the first of these prompts the runner
    // has; one it never got exists only on this page.
    const persisted = removed.map((id) => v.entries.get(id)).find((e) => e.kind === 'user' && !e.state);
    const first = v.order.find((id) => v.entries.get(id).kind === 'user') === edit.id;
    // The edited prompt runs again with the model it ran with, unless one was
    // chosen for the session.
    const model = v.model || v.entries.get(edit.id)?.model || session.nextModel(v);
    const before = { order: v.order, users: new Map(v.users), entries: new Map(v.entries), title: v.title, interrupted: v.interrupted };
    v.edit = null;
    for (const id of removed) {
      v.entries.delete(id);
      v.users.delete(id);
    }
    service('timeline')?.forget?.(removed);
    v.order = v.order.slice(0, at);
    // A title that was the first prompt follows the edited one.
    if (first && !v.renamed) v.title = '';
    await session.submit(text, {
      images: edit.images?.referenced(text) || [],
      model,
      rewind: persisted ? persisted.id.replace(/^input:/, '') : '',
      undo: () => {
        Object.assign(v, before);
        v.edit = edit;
        session.touch(v);
        service('timeline')?.flushNow?.();
        edit.input.focus({ preventScroll: true });
      },
    });
    return undefined;
  }

  // command is /edit: prompt n, or the last.
  function command(arg = '') {
    const v = view();
    if (!arg) return session.lastPrompt(v) ? editLast() : toast('There is no prompt to edit yet.');
    const { entry, error } = session.promptNumbered(v, arg);
    if (error) return toast(error, 'error');
    return begin(entry.id);
  }

  // ---------------------------------------------------------------- following the session

  cockpit.contribute('timeline.editor', { editor: (entry, ctx) => (ctx.view.edit?.id === entry.id ? ctx.view.edit.node : null) });
  cockpit.contribute('timeline.action', {
    id: 'edit', kinds: ['user'], order: 10, label: 'Edit', title: 'Edit this prompt and run it again from here (Esc Esc edits the last one)',
    shown: (entry, ctx) => !session.runBlocked(ctx.view), run: (entry) => begin(entry.id),
  });
  // A prompt at the end leaves an edit further up behind.
  cockpit.on('session:before-run', (v) => close(v));
  cockpit.on('session:gone', (v) => drop(v, '', { always: true }));
  // An edit outlives a reload only while its prompt is there and nothing runs.
  cockpit.on('session:loaded', (v, data) => {
    if (v.edit && (data.run || v.external || !v.entries.has(v.edit.id))) drop(v, 'The session changed in another window, so the edit was closed.');
  });
  cockpit.on('session:finish', (v) => {
    if (!v.editAfterStop) return;
    v.editAfterStop = false;
    if (v === view()) editLast();
  });
  cockpit.listen(window, 'resize', () => { if (view()?.edit) size(view().edit.input); });
  cockpit.on('layout:resize', () => { if (view()?.edit) size(view().edit.input); });
  // An Esc pressed outside the editor cancels it too.
  cockpit.keys.register({
    key: 'Escape', global: true, priority: 50,
    when: () => !!view()?.edit,
    run: () => close(view(), { focusComposer: true }),
  });
  // Loaded anew, the editor of the version before goes: its handlers were
  // that version's.
  cockpit.onDispose(() => { const v = view(); if (v?.edit) drop(v, ''); });

  cockpit.provide('edit', { begin, editLast, close, drop, command });
  cockpit.commands.register({ name: 'edit', args: '[n]', help: 'Edit prompt n, by default the last, and run it again from there', order: 50, shown: (v) => v.prompts > 0, run: (arg) => command(String(arg)) });
  cockpit.contribute('palette.provider', {
    id: 'edit', order: 301,
    items: () => {
      const v = view();
      return v && session.lastPrompt(v) && !session.runBlocked(v)
        ? [{ group: 'Session', icon: '↶', label: 'Edit the last prompt', hint: 'Esc Esc', order: 301, run: editLast }] : [];
    },
  });
}
