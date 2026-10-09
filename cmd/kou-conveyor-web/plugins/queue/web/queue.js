// queue: what is written while the agent works waits in the session's
// queue, which the server keeps (every tab sees it, and it outlives the
// tab). Enter queues a prompt: it runs as a prompt of its own once the run
// ends, however long that takes. ⌘Enter forces it: the running agent reads
// it once the tool calls it is making finish, after their results, never
// cutting a response short. A run that is stopped or fails pauses the queue
// until it is resumed. The tray sits on the composer (dock); ↑ from an
// empty composer takes the keys to it.
export default function activate(cockpit) {
  const { h } = cockpit;
  const session = cockpit.use('session');
  const composer = cockpit.use('composer');
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const view = () => session.view?.();
  const toast = (text, kind = 'info', key = '') => cockpit.toast(text, kind, key);
  const hot = cockpit.hot.data;
  const ui = hot.ui ??= { sig: '', editing: null, drag: null, seen: new Set() };
  ui.sig = '';

  const queuePath = (v, rest = '') => cockpit.wsPath(v.ws, `/sessions/${encodeURIComponent(v.id)}/queue${rest}`);

  // ---------------------------------------------------------------- the tray

  const count = h('span', { class: 'queue-count', id: 'queue-count', text: '0' });
  const stateText = h('span', { class: 'queue-state', id: 'queue-state' });
  const resumeButton = h('button', { class: 'act strong', id: 'queue-resume', type: 'button', title: 'Run the queued messages, one after another', hidden: true, onclick: () => act('POST', '/resume') }, '▶ Resume');
  const pauseButton = h('button', { class: 'act', id: 'queue-pause', type: 'button', title: 'Keep the queued messages from running when the agent finishes', hidden: true, onclick: () => act('POST', '/pause') }, 'Pause');
  const clearButton = h('button', { class: 'act', id: 'queue-clear', type: 'button', title: 'Drop the queued messages', onclick: () => act('DELETE') }, 'Clear');
  const list = h('ol', { class: 'queue-list', id: 'queue-list' });
  const box = h('section', { class: 'queue', id: 'queue', 'aria-label': 'Queued messages', hidden: true },
    h('header', { class: 'queue-head' }, h('span', { class: 'label', text: 'Queue' }), count, stateText,
      h('span', { class: 'queue-tools' }, resumeButton, pauseButton, clearButton)),
    list);
  cockpit.ui.mount('dock', { id: 'queue', order: 30, node: box });

  // forcedWhen says when the running agent reads a forced prompt.
  function forcedWhen(v) {
    for (const id of v.order) {
      const entry = v.entries.get(id);
      if (entry.kind === 'tool' && ['queued', 'running'].includes(entry.tool?.state)) {
        return { text: 'after', code: (entry.tool.input || entry.tool.name || 'its tool calls').split('\n')[0] };
      }
    }
    return { text: v.run ? 'after the step it is on' : 'when it runs', code: '' };
  }

  async function enqueue(text, { force = false } = {}) {
    const v = view();
    const pictures = service('images');
    const sent = pictures?.take?.(text) || [];
    composer.clear?.();
    // A run that is starting names its session first.
    if (v.run?.ready) await v.run.ready;
    try {
      const res = await cockpit.api(queuePath(v), {
        method: 'POST',
        body: { text, force, model: session.nextModel(v) || undefined, images: sent.length ? await pictures.encode(sent) : undefined },
      });
      // The prompt runs under the queued item's ID; its images show from here.
      if (res.item?.id) pictures?.remember?.(res.item.id, sent);
      if (v !== view()) return;
      if (res.queue) session.setQueue(v, res.queue);
      if (res.run) return session.followQueued(v, res.run);
      const when = forcedWhen(v);
      if (res.note) toast(res.note, 'warn', 'queue');
      else if (force) toast(`⚡ Forced in: the agent reads it ${when.text}${when.code ? ` ${when.code}` : ''}`, 'info', 'queue');
      else toast(v.queue.paused ? 'Queued. The queue is paused: resume it to run it.' : 'Queued: it runs when the agent finishes', 'info', 'queue');
    } catch (error) {
      if (v !== view()) return;
      composer.restore?.(text, sent);
      toast(error.message, 'error');
    }
  }

  // act asks the server to change the queue, and shows what it is now.
  async function act(method, rest = '', body) {
    const v = view();
    try {
      const res = await cockpit.api(queuePath(v, rest), { method, body });
      if (v !== view()) return null;
      if (res.queue) session.setQueue(v, res.queue);
      if (res.run) session.followQueued(v, res.run);
      if (res.note) toast(res.note, 'warn', 'queue');
      return res;
    } catch (error) {
      if (v !== view()) return null;
      if (error.body?.code === 'queue_gone') {
        toast('That message is no longer in the queue: the agent has it, or another window changed the queue.');
        cockpit.api(queuePath(v)).then((res) => { if (v === view()) session.setQueue(v, res.queue); }).catch(() => {});
      } else {
        toast(error.message, 'error');
      }
      return null;
    }
  }

  const forceQueued = (id) => act('PATCH', `/${encodeURIComponent(id)}`, { force: true });
  const dropQueued = (id) => act('DELETE', `/${encodeURIComponent(id)}`);
  const moveQueued = (id, position) => act('PATCH', `/${encodeURIComponent(id)}`, { position: Math.max(0, position) });

  // position is a prompt's place among the queued ones (forced ones come
  // first and do not count).
  function position(v, id) {
    return v.queue.items.filter((item) => !item.forced).findIndex((item) => item.id === id);
  }

  function focusQueued(index) {
    const items = [...list.querySelectorAll('.queue-item')];
    if (!items.length) return;
    items[Math.max(0, Math.min(index, items.length - 1))].focus();
  }

  function render() {
    const v = view();
    if (!v) return;
    const q = v.queue;
    box.hidden = !q.items.length || v.external;
    if (box.hidden) {
      ui.sig = '';
      ui.editing = null;
      return;
    }
    const forced = q.items.filter((item) => item.forced).length;
    const queued = q.items.length - forced;
    const waiting = q.paused || !v.run;
    box.dataset.paused = waiting ? 'true' : 'false';
    count.textContent = String(q.items.length);
    stateText.textContent = waiting
      ? (q.paused ? 'Paused · the queued messages wait for you' : 'Waiting · the next runs when you resume')
      : forced && queued ? `⚡ ${forced} forced in after the running tools · ${queued} when the agent finishes`
        : forced ? '⚡ Forced in: the agent reads it after its running tools'
          : 'Runs when the agent finishes, one after another';
    resumeButton.hidden = !waiting || !queued;
    pauseButton.hidden = waiting || !queued;
    clearButton.hidden = !queued;

    // The list is rebuilt only when it changes, so its animations and focus
    // live on; the forced prompts' readout follows the run.
    const sig = JSON.stringify([q.items, waiting, ui.editing, !!v.run]);
    if (sig !== ui.sig) {
      ui.sig = sig;
      const focused = document.activeElement?.closest?.('.queue-item')?.dataset.id;
      const editor = list.querySelector('.queue-editor');
      const draft = editor ? [editor.value, editor.selectionStart, editor.selectionEnd] : null;
      let number = 0;
      const nodes = q.items.map((item) => {
        if (!item.forced) number++;
        const node = row(v, item, item.forced ? 0 : number, waiting);
        if (!ui.seen.has(item.id)) node.dataset.new = 'true';
        return node;
      });
      ui.seen = new Set(q.items.map((item) => item.id));
      list.replaceChildren(...nodes);
      if (focused) list.querySelector(`.queue-item[data-id="${CSS.escape(focused)}"]`)?.focus();
      const input = list.querySelector('.queue-editor');
      if (input) {
        if (draft) {
          input.value = draft[0];
          input.setSelectionRange(draft[1], draft[2]);
        }
        size(input);
        if (!draft) {
          input.focus();
          input.setSelectionRange(input.value.length, input.value.length);
        }
      }
    }
    const when = forcedWhen(v);
    for (const el of list.querySelectorAll('.queue-when')) {
      el.replaceChildren(when.text, when.code ? ' ' : '', when.code ? h('code', { text: when.code.length > 48 ? `${when.code.slice(0, 47)}…` : when.code }) : '');
    }
  }
  cockpit.on('render', render);
  cockpit.on('session:queue', () => cockpit.render());

  function row(v, item, number, waiting) {
    const editing = ui.editing === item.id && !item.forced;
    const li = h('li', {
      class: 'queue-item', tabindex: '0', draggable: !item.forced && !editing ? 'true' : null,
      data: { id: item.id, forced: item.forced ? 'true' : null, editing: editing ? 'true' : null },
      title: item.forced ? 'The agent has this message: it reads it after its running tools' : null,
    });
    const mark = item.forced
      ? h('span', { class: 'queue-mark', 'aria-label': 'Forced' }, h('span', { class: 'queue-bolt', text: '⚡' }))
      : h('span', { class: 'queue-mark', text: String(number).padStart(2, '0') });
    let text;
    if (editing) {
      text = h('textarea', { class: 'queue-editor', rows: '1', 'aria-label': 'Edit the queued message', spellcheck: 'true' });
      text.value = item.text;
    } else {
      text = h('span', { class: 'queue-text', text: item.text });
      if (item.images?.length) {
        const pictures = service('images');
        text = h('span', { class: 'queue-body' },
          h('span', { class: 'queue-images' }, item.images.map((image, n) => h('img', { src: pictures?.queuedImageURL?.(v, item, n) || '', alt: image.label, title: image.label }))),
          text);
      }
    }
    // A queued prompt runs with the model it was queued with; a forced one
    // with the running agent's.
    const model = !item.forced && item.model
      ? h('span', { class: 'queue-model', text: item.model, title: service('models')?.title?.(item.model, 'The model it runs with') || item.model })
      : null;
    const meta = item.forced
      ? h('span', { class: 'queue-meta' }, h('span', { class: 'queue-flow', 'aria-hidden': 'true' }, h('i'), h('i'), h('i')), h('span', { class: 'queue-when' }))
      : number === 1 && !waiting ? h('span', { class: 'queue-meta' }, model, h('span', { class: 'queue-next', text: 'Next' }))
        : model ? h('span', { class: 'queue-meta' }, model) : null;
    const tools = item.forced ? null : editing
      ? h('span', { class: 'queue-actions' },
        h('button', { class: 'act strong', type: 'button', text: 'Save', data: { act: 'save' }, title: 'Save (↵)' }),
        h('button', { class: 'act', type: 'button', text: 'Cancel', data: { act: 'cancel' }, title: 'Keep it as it was (Esc)' }))
      : h('span', { class: 'queue-actions' },
        v.run && h('button', { class: 'act force-act', type: 'button', text: '⚡ Force', data: { act: 'force' }, title: 'Force it in: the agent reads it after its running tools (⌘↵)' }),
        h('button', { class: 'act', type: 'button', text: 'Edit', data: { act: 'edit' }, title: 'Edit (↵)' }),
        h('button', { class: 'icon small', type: 'button', text: '×', data: { act: 'drop' }, title: 'Drop it from the queue (⌫)', 'aria-label': 'Drop' }));
    // DOM append would write null out; the meta column keeps its place.
    li.append(mark, text, meta || h('span'), ...(tools ? [tools] : []));
    return li;
  }

  function size(input) {
    input.style.height = 'auto';
    input.style.height = `${Math.min(input.scrollHeight, 200)}px`;
  }

  function edit(id) {
    ui.editing = id;
    ui.sig = '';
    render();
  }

  async function save(input) {
    const id = input.closest('.queue-item')?.dataset.id;
    const item = view().queue.items.find((it) => it.id === id);
    const text = input.value.trim();
    ui.editing = null;
    ui.sig = '';
    if (!item || !text || text === item.text) {
      render();
      if (item && !text) await dropQueued(id);
      focusItem(id);
      return;
    }
    // The edit shows at once; the server's answer settles it.
    item.text = text;
    render();
    focusItem(id);
    await act('PATCH', `/${encodeURIComponent(id)}`, { text });
  }

  function cancel(id) {
    ui.editing = null;
    ui.sig = '';
    render();
    focusItem(id);
  }

  function focusItem(id) {
    list.querySelector(`.queue-item[data-id="${CSS.escape(id || '')}"]`)?.focus();
  }

  cockpit.listen(list, 'click', (event) => {
    const button = event.target.closest('button[data-act]');
    const li = event.target.closest('.queue-item');
    if (!li) return;
    const id = li.dataset.id;
    switch (button?.dataset.act) {
      case 'force': forceQueued(id); break;
      case 'edit': edit(id); break;
      case 'drop': dropQueued(id); break;
      case 'save': save(li.querySelector('.queue-editor')); break;
      case 'cancel': cancel(id); break;
      default:
        if (event.detail === 2 && !li.dataset.forced) edit(id);
    }
  });
  cockpit.listen(list, 'input', (event) => {
    if (event.target.classList.contains('queue-editor')) size(event.target);
  });
  cockpit.listen(list, 'focusout', (event) => {
    // An edit left for elsewhere is saved, unless its own buttons took focus.
    const input = event.target;
    if (!input.classList?.contains('queue-editor')) return;
    if (event.relatedTarget && input.closest('.queue-item')?.contains(event.relatedTarget)) return;
    if (ui.editing) save(input);
  });
  cockpit.listen(list, 'keydown', (event) => {
    const li = event.target.closest('.queue-item');
    if (!li) return;
    const id = li.dataset.id;
    if (event.target.classList.contains('queue-editor')) {
      if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
        event.preventDefault();
        save(event.target);
      } else if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        cancel(id);
      }
      return;
    }
    if (event.target !== li) return; // its buttons keep their keys
    const items = [...list.querySelectorAll('.queue-item')];
    const at = items.indexOf(li);
    const v = view();
    switch (event.key) {
      case 'ArrowUp':
      case 'ArrowDown': {
        event.preventDefault();
        const step = event.key === 'ArrowUp' ? -1 : 1;
        if (event.altKey || event.shiftKey) {
          if (!li.dataset.forced) moveQueued(id, position(v, id) + step);
        } else if (at + step >= items.length) {
          composer.focus?.();
        } else {
          items[Math.max(0, at + step)].focus();
        }
        break;
      }
      case 'Enter':
        event.preventDefault();
        if (li.dataset.forced) break;
        if (event.metaKey || event.ctrlKey) forceQueued(id);
        else edit(id);
        break;
      case 'Backspace':
      case 'Delete':
        event.preventDefault();
        if (li.dataset.forced) toast('The agent has this message already: it cannot be taken back.');
        else {
          const neighbour = items[at + 1] || items[at - 1];
          if (neighbour) neighbour.focus();
          else composer.focus?.();
          dropQueued(id);
        }
        break;
      case 'Escape':
        event.preventDefault();
        event.stopPropagation(); // not the first Esc of an Esc Esc
        composer.focus?.();
        break;
      default:
        // Typing goes back to the composer.
        if (event.key.length === 1 && !event.metaKey && !event.ctrlKey && !event.altKey) composer.focus?.();
    }
  });
  // Queued prompts are dragged into another order.
  cockpit.listen(list, 'dragstart', (event) => {
    const li = event.target.closest?.('.queue-item');
    if (!li || li.dataset.forced) return;
    ui.drag = li.dataset.id;
    li.dataset.dragging = 'true';
    event.dataTransfer.effectAllowed = 'move';
    event.dataTransfer.setData('text/plain', li.querySelector('.queue-text')?.textContent || '');
  });
  cockpit.listen(list, 'dragover', (event) => {
    const li = event.target.closest('.queue-item');
    if (!ui.drag || !li || li.dataset.forced) return;
    event.preventDefault();
    const rect = li.getBoundingClientRect();
    for (const other of list.querySelectorAll('[data-drop]')) if (other !== li) delete other.dataset.drop;
    li.dataset.drop = event.clientY < rect.top + rect.height / 2 ? 'before' : 'after';
  });
  cockpit.listen(list, 'drop', (event) => {
    const li = event.target.closest('.queue-item');
    const id = ui.drag;
    if (!id || !li) return;
    event.preventDefault();
    const v = view();
    const from = position(v, id);
    let to = position(v, li.dataset.id) + (li.dataset.drop === 'after' ? 1 : 0);
    if (from < to) to--;
    delete li.dataset.drop;
    if (from !== to && to >= 0) moveQueued(id, to);
  });
  cockpit.listen(list, 'dragend', () => {
    ui.drag = null;
    for (const el of list.querySelectorAll('[data-drop], [data-dragging]')) {
      delete el.dataset.drop;
      delete el.dataset.dragging;
    }
  });

  // ---------------------------------------------------------------- the composer

  // While the agent works, what is written waits for it.
  cockpit.hooks.tap('composer.submit', ({ text, force, view: v }) => {
    if (!v.run || v.external || v.gone || v.edit) return false;
    enqueue(text, { force });
    return true;
  }, { order: 10 });
  // Over an empty composer, ↑ goes to what waits for the agent.
  cockpit.hooks.tap('composer.key', (event) => {
    const v = view();
    if (event.key !== 'ArrowUp' || event.altKey || composer.value?.() || !v?.queue.items.length) return false;
    event.preventDefault();
    focusQueued(v.queue.items.length - 1);
    return true;
  }, { order: 20 });

  // command is /queue: the queue takes the keys, and resume, pause and
  // clear act on it.
  function command(arg) {
    const v = view();
    if (v.fresh && !v.run) return toast('Nothing waits yet: while the agent works, Enter queues a message and ⌘Enter forces one in.');
    switch (arg) {
      case '':
        if (!v.queue.items.length) return toast('The queue is empty: while the agent works, Enter queues a message and ⌘Enter forces one in.');
        return focusQueued(v.queue.items.length - 1);
      case 'resume': return act('POST', '/resume');
      case 'pause': return act('POST', '/pause');
      case 'clear': return act('DELETE');
      default: return toast('/queue takes resume, pause or clear', 'error');
    }
  }

  cockpit.provide('queue', { enqueue, act, command, focus: focusQueued });
  cockpit.commands.register({
    name: 'queue', args: '[resume|pause|clear]', help: 'What waits for the agent: select it (↑), run it, hold it or drop it', order: 40,
    shown: (v) => v.queued > 0 || v.running,
    complete: (v) => [
      v.paused && { value: 'resume', label: 'resume', detail: 'run the queued messages, one after another' },
      !v.paused && v.queued > 0 && { value: 'pause', label: 'pause', detail: 'keep them from running when the agent finishes' },
      v.queued > 0 && { value: 'clear', label: 'clear', detail: 'drop the queued messages' },
    ].filter(Boolean),
    run: (arg) => command(String(arg).trim()),
  });
  cockpit.contribute('help.keys', { keys: ['⌘', '↵'], text: 'Force it in: the agent reads it after its running tools', order: 20 });
  cockpit.contribute('help.keys', { keys: ['↑'], text: 'Queued messages, from an empty composer: ↵ edit · ⌘↵ force · ⌫ drop · ⌥↑↓ move', order: 30 });
  cockpit.render();
}
