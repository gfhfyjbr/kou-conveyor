// The body of a note: Markdown, shown rendered. A double-click (or Enter)
// edits it; leaving the field, or ⌘Enter, keeps what was written, and Esc
// leaves it as it was.

export function createNoteBody(node, ctx) {
  const { h } = ctx;
  let current = node;
  let editing = null;

  const view = h('div', { class: 'cv-note-view' });
  const box = h('div', { class: 'cv-note' }, view);
  view.dataset.scroll = 'true';

  const textOf = (n) => (typeof n.config?.text === 'string' ? n.config.text : '');

  function draw() {
    if (editing) return;
    const text = textOf(current);
    view.replaceChildren(text.trim() ? ctx.markdown(text) : h('p', { class: 'cv-note-empty', text: 'Double-click to write.' }));
  }

  function edit() {
    if (editing || current.proposed) return;
    const area = h('textarea', { class: 'cv-note-edit', spellcheck: 'true', 'aria-label': 'The note, in Markdown' });
    area.value = textOf(current);
    let done = false;
    const finish = (save) => {
      if (done) return;
      done = true;
      const text = area.value;
      editing = null;
      area.remove();
      view.hidden = false;
      if (save && text !== textOf(current)) {
        current = { ...current, config: { ...(current.config || {}), text } };
        ctx.update({ config: { text } }, 'Edit the note');
      }
      draw();
    };
    area.addEventListener('keydown', (event) => {
      event.stopPropagation();
      if (event.key === 'Escape') {
        event.preventDefault();
        finish(false);
      } else if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
        event.preventDefault();
        finish(true);
      }
    });
    area.addEventListener('blur', () => finish(true));
    editing = area;
    view.hidden = true;
    box.append(area);
    area.focus();
    area.setSelectionRange(area.value.length, area.value.length);
  }

  box.addEventListener('dblclick', (event) => {
    if (event.target.closest('a')) return;
    event.stopPropagation();
    edit();
  });

  draw();
  return {
    node: box,
    update(next) {
      current = next;
      draw();
    },
    lod() {},
    shown() {},
    hidden() {},
    focus() {
      edit();
      return true;
    },
    contains: (target) => box.contains(target),
    menu: () => [{ icon: '✎', label: 'Edit the note', hint: 'Enter', run: edit }],
    dispose() {
      editing?.remove();
    },
  };
}
