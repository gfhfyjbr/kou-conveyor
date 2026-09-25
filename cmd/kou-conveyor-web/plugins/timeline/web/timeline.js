// timeline: the transcript of the session in view, in the layout's
// stage.main. It draws the session's entries as they come and change, and
// takes contributions that change how they look:
//
//   timeline.renderer    { kind, render(entry, ctx) → node | null }: draws a
//                        kind of entry (user, assistant, reasoning, tool,
//                        notice, error, or a kind of a plugin's); the latest
//                        that returns a node wins, null leaves it to the next
//   timeline.action      { id, kinds, label, title, order, shown(entry, ctx), run(entry, ctx) }:
//                        a button in an entry's header (Edit, Copy, Reuse…)
//   timeline.decoration  { kinds, order, render(entry, ctx) → node | null }: drawn
//                        under an entry (a prompt's images)
//   timeline.editor      { editor(entry, ctx) → node | null }: a prompt being
//                        edited shows this node in place of its text
//   tool.view            { tool, summary(entry), render(entry, ui) }: how a
//                        tool's calls read (cockpit.tools.register)
//
// It provides the timeline service: schedule(id), flushNow(), reveal(id),
// scrollToBottom(), nearBottom(), promptInView(v), node(id), forget(ids),
// setAllTools(open), scroller().
import { fmt, h } from '/kernel/dom.js';

const TOOL_GLYPH = { queued: '□', running: '■', done: '✓', failed: '✕', canceled: '⊘', interrupted: '◌' };
const TOOL_WORD = { queued: 'Queued', running: 'Running', done: 'Done', failed: 'Failed', canceled: 'Stopped', interrupted: 'Interrupted' };

const STARTERS = [
  ['Survey', 'Map the architecture, entry points and how to build and test.', 'Map this repository: its architecture, entry points, and how to build and test it.'],
  ['Verify', 'Run the tests and fix the first real failure.', 'Run the test suite. If anything fails, find the root cause and fix it.'],
  ['Review', 'Audit uncommitted changes for bugs and risky edits.', 'Review the uncommitted changes for bugs, races and risky edits. Report findings by severity.'],
  ['Profile', 'Find the slowest step and propose a concrete fix.', 'Find the slowest part of the build or test run and propose a concrete fix.'],
];

// toolState maps a tool to what is shown: a tool that never finished in a
// session nobody is running was interrupted.
export function toolState(entry, live) {
  const state = entry.tool?.state || 'queued';
  if (!live && (state === 'queued' || state === 'running')) return 'interrupted';
  return state;
}

export default function activate(cockpit) {
  const session = cockpit.use('session');
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const view = () => session.view?.();

  // ---------------------------------------------------------------- the log

  const empty = h('section', { class: 'empty', id: 'empty', hidden: true },
    h('div', { class: 'empty-card ticks' },
      h('div', { class: 'empty-head' }, h('span', { class: 'label', text: 'Session · New' }), h('span', { class: 'label', id: 'empty-path' })),
      h('h1', null, 'Ready', h('span', { class: 'caret', 'aria-hidden': 'true' })),
      h('p', { text: 'The agent works in this workspace with a shell. Describe the outcome you want — it plans, runs commands and reports back as it goes.' }),
      h('ol', { class: 'starters' }, STARTERS.map(([title, text, prompt], n) => h('li', null, h('button', {
        type: 'button', data: { starter: prompt },
        onclick: () => service('composer')?.set?.(prompt, { focus: true, end: true }),
      }, h('span', { class: 'n', text: String(n + 1).padStart(2, '0') }), h('b', { text: title }), h('span', { text }))))),
      h('div', { class: 'empty-keys' },
        h('span', null, h('kbd', { text: '⌘K' }), ' commands'), h('span', null, h('kbd', { text: '/' }), ' sessions'), h('span', null, h('kbd', { text: '?' }), ' shortcuts'))));
  const emptyPath = empty.querySelector('#empty-path');
  const loading = h('div', { class: 'state-line', id: 'loading', hidden: true },
    h('span', { class: 'meter', 'aria-hidden': 'true' }, Array.from({ length: 8 }, () => h('i'))), 'Loading session');
  const errorText = h('span', { id: 'load-error-text' });
  const loadError = h('div', { class: 'load-error', id: 'load-error', hidden: true },
    h('b', { text: 'Could not load this session' }), errorText,
    h('button', { class: 'act', id: 'load-retry', type: 'button', onclick: () => session.openSession(view().id, { push: false }) }, 'Retry'));
  const list = h('ol', { class: 'entries', id: 'entries', 'aria-live': 'polite' });
  const log = h('div', { class: 'log' }, empty, loading, loadError, list);
  cockpit.ui.mount('stage.main', { id: 'timeline', order: 0, node: log });

  const jumpCount = h('span', { id: 'jump-count', text: '0' });
  const jump = h('button', { class: 'jump', id: 'jump', type: 'button', hidden: true, onclick: () => scrollToBottom() }, jumpCount, ' new ↓');
  cockpit.ui.mount('dock.float', { id: 'jump', order: 0, node: jump });

  const scroller = () => service('layout')?.scroller?.() || log.parentElement;
  const nodes = new Map();
  const pending = { ids: new Set(), all: false, frame: 0 };
  let unseen = 0;

  function schedule(id) {
    if (id === undefined) pending.all = true;
    else pending.ids.add(id);
    if (!pending.frame) pending.frame = requestAnimationFrame(flush);
  }
  cockpit.onDispose(() => cancelAnimationFrame(pending.frame));

  function nearBottom() {
    const el = scroller();
    return !el || el.scrollHeight - el.scrollTop - el.clientHeight < 96;
  }

  function scrollToBottom() {
    const el = scroller();
    if (el) el.scrollTop = el.scrollHeight;
    unseen = 0;
    jump.hidden = true;
  }

  function flush() {
    pending.frame = 0;
    const v = view();
    if (!v) return;
    const stick = nearBottom();
    const ctx = context(v);
    // Drawing an entry again moves what has the focus in it (an editor),
    // which loses the focus; it gets it back.
    const active = list.contains(document.activeElement) ? document.activeElement : null;
    const selection = active && 'selectionStart' in active ? [active.selectionStart, active.selectionEnd, active.selectionDirection] : null;
    let added = 0;
    if (pending.all) {
      const el = scroller();
      const top = el?.scrollTop || 0;
      pending.all = false;
      pending.ids.clear();
      nodes.clear();
      const frag = document.createDocumentFragment();
      for (const id of v.order) {
        const node = draw(v.entries.get(id), ctx);
        nodes.set(id, node);
        frag.append(node);
      }
      list.replaceChildren(frag);
      if (!stick && el) el.scrollTop = top;
    } else {
      for (const id of pending.ids) {
        const entry = v.entries.get(id);
        if (!entry) continue;
        const node = draw(entry, ctx);
        const old = nodes.get(id);
        if (old) old.replaceWith(node);
        else {
          list.append(node);
          added++;
        }
        nodes.set(id, node);
      }
      pending.ids.clear();
    }
    if (active && active.isConnected && document.activeElement !== active) {
      active.focus({ preventScroll: true });
      if (selection) active.setSelectionRange(...selection);
    }
    renderState(v);
    cockpit.render();
    if (stick) scrollToBottom();
    else if (added) {
      unseen += added;
      jumpCount.textContent = String(unseen);
      jump.hidden = false;
    }
    cockpit.emit('timeline:flush', v);
  }

  // flushNow draws what is pending right away, for code that needs the DOM.
  function flushNow() {
    if (pending.frame) cancelAnimationFrame(pending.frame);
    flush();
  }

  function renderState(v) {
    empty.hidden = !(v.order.length === 0 && !v.loading && !v.failed);
    loading.hidden = !v.loading;
    loadError.hidden = !v.failed;
    errorText.textContent = v.failed;
    emptyPath.textContent = session.currentWorkspace?.()?.path || session.state?.config?.workspace || '';
  }

  // ---------------------------------------------------------------- drawing entries

  // context is what renderers, actions and decorations get to draw with.
  function context(v) {
    const models = service('models');
    const ctx = {
      h, fmt, view: v, summary: session.summary(v),
      live: !!v.run || v.external,
      index: (id) => v.users.get(id) || 0,
      expanded: (entry) => {
        if (v.expanded.has(entry.id)) return v.expanded.get(entry.id);
        return entry.kind === 'tool' && entry.tool?.state === 'failed' && !!entry.tool?.error;
      },
      toggle: (id, open) => {
        v.expanded.set(id, open);
        schedule(id);
      },
      copy: (text, label) => cockpit.copy(text, label),
      markdown: (text, options) => (cockpit.has('markdown') ? cockpit.use('markdown').render(text, options) : h('p', { text })),
      button,
      actions: (entry) => actionsFor(entry, ctx),
      stream: (label, text, kind = 'out') => stream(label, text, kind),
      // The prompt's model, and whether it differs from the prompt before.
      modelTitle: (id) => models?.title?.(id, 'The model that answered this prompt') || id,
      modelProvider: (id) => models?.provider?.(id) || '',
      switched: (entry) => {
        const before = session.previousModel(v, entry.id);
        return !!before && !!entry.model && before !== entry.model;
      },
      arrived: (id) => v.arrived.delete(id),
      editable: cockpit.has('edit') && !session.runBlocked(v),
    };
    return ctx;
  }

  function button(label, onclick, title) {
    return h('button', { class: 'act', type: 'button', title, onclick: (event) => { event.stopPropagation(); onclick(); } }, label);
  }

  function actionsFor(entry, ctx) {
    const buttons = cockpit.contributions('timeline.action', { unique: 'id' })
      .filter((a) => (!a.kinds || a.kinds.includes(entry.kind)) && (!a.shown || cockpit.safely(() => a.shown(entry, ctx))))
      .map((a) => button(typeof a.label === 'function' ? a.label(entry) : a.label, () => cockpit.safely(() => a.run(entry, ctx)), typeof a.title === 'function' ? a.title(entry) : a.title));
    return h('span', { class: 'actions' }, buttons);
  }

  function decorations(entry, ctx) {
    return cockpit.contributions('timeline.decoration')
      .filter((d) => !d.kinds || d.kinds.includes(entry.kind))
      .map((d) => cockpit.safely(() => d.render(entry, ctx)))
      .filter((node) => node instanceof Node);
  }

  // draw builds one entry: the latest renderer of its kind that answers
  // (by order: the built-in ones come last).
  function draw(entry, ctx) {
    const renderers = cockpit.contributions('timeline.renderer').filter((r) => r.kind === entry.kind || r.kind === '*').reverse();
    for (const renderer of renderers) {
      const node = cockpit.safely(() => renderer.render(entry, ctx));
      if (node instanceof Node) return node;
    }
    return h('li', { class: 'entry', data: { kind: entry.kind, id: entry.id } }, h('div', { class: 'gutter' }), h('div', { class: 'body' }, h('div', { class: 'text', text: entry.text || '' })));
  }

  // shell is an entry's frame: its item, gutter and body.
  function shell(entry) {
    const li = h('li', { class: 'entry', id: `entry-${entry.id}`, data: { kind: entry.kind, id: entry.id } });
    const gutter = h('div', { class: 'gutter' });
    const body = h('div', { class: 'body' });
    li.append(gutter, body);
    const time = entry.at ? h('time', { datetime: entry.at, title: fmt.stamp(entry.at), text: fmt.clock(entry.at) }) : null;
    return { li, gutter, body, time };
  }

  function activateKeys(fn) {
    return (event) => {
      if (event.target !== event.currentTarget) return;
      if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); fn(); }
    };
  }

  const builtin = {
    user(entry, ctx) {
      const { li, gutter, body, time } = shell(entry);
      const n = ctx.index(entry.id);
      gutter.append(h('span', { class: 'idx', text: String(n).padStart(2, '0') }));
      if (entry.state) li.dataset.state = entry.state;
      // A plugin's editor takes the prompt's place while it is edited.
      for (const editor of cockpit.contributions('timeline.editor')) {
        const node = cockpit.safely(() => editor.editor(entry, ctx));
        if (node instanceof Node) {
          li.dataset.editing = 'true';
          body.append(h('header', { class: 'meta' }, h('span', { class: 'who', text: 'You' }), time, h('span', { class: 'flag editing', text: 'Editing' })), node);
          return li;
        }
      }
      const flag = entry.state === 'pending' ? h('span', { class: 'flag pending', text: 'Sending' })
        : entry.state === 'undelivered' ? h('span', { class: 'flag undelivered', text: 'Not delivered' }) : null;
      // Sent while the agent worked, which read it after its tool calls.
      const forced = entry.forced && h('span', {
        class: 'flag forced', text: '⚡ Forced in',
        title: 'Sent while the agent worked: it read this after the results of the tool calls it was making',
      });
      if (entry.forced) li.dataset.forced = 'true';
      if (ctx.arrived(entry.id)) li.dataset.arrived = 'true';
      // The model that answered the prompt; a session may use another for
      // every prompt, and one that differs from the prompt before is marked.
      const model = entry.model && h('span', {
        class: 'model-tag', title: ctx.modelTitle(entry.model) || entry.model,
        data: { provider: ctx.modelProvider(entry.model) || null, switched: ctx.switched(entry) ? 'true' : null },
      }, h('i', { class: 'model-dot', 'aria-hidden': 'true' }), entry.model);
      body.append(
        h('header', { class: 'meta' }, h('span', { class: 'who', text: 'You' }), time, model, flag, forced, ctx.actions(entry)),
        h('div', { class: 'text', text: entry.text }),
        ...decorations(entry, ctx));
      return li;
    },
    assistant(entry, ctx) {
      const { li, gutter, body, time } = shell(entry);
      gutter.append(h('span', { class: 'tick', text: fmt.short(entry.at) }));
      if (entry.phase) li.dataset.phase = entry.phase;
      body.append(
        h('header', { class: 'meta' }, h('span', { class: 'who', text: 'Agent' }),
          entry.phase === 'commentary' && h('span', { class: 'tag', text: 'Note' }), time, ctx.actions(entry)),
        h('div', { class: 'prose' }, ctx.markdown(entry.text, { onCopy: (text) => ctx.copy(text, 'Code copied') })),
        ...decorations(entry, ctx));
      return li;
    },
    reasoning(entry, ctx) {
      const { li, gutter, body } = shell(entry);
      gutter.append(h('span', { class: 'tick', text: fmt.short(entry.at) }));
      const open = ctx.expanded(entry);
      const gist = entry.text.split('\n').find((line) => line.trim()) || '';
      const toggle = () => ctx.toggle(entry.id, !open);
      body.append(h('div', { class: 'fold', role: 'button', tabindex: 0, 'aria-expanded': String(open), onclick: toggle, onkeydown: activateKeys(toggle) },
        h('span', { class: 'who', text: 'Thinking' }),
        h('span', { class: 'gist', text: gist.replace(/\*\*/g, '') }),
        h('span', { class: 'chev', 'aria-hidden': 'true' })));
      if (open) body.append(h('div', { class: 'prose thought' }, ctx.markdown(entry.text)));
      return li;
    },
    tool(entry, ctx) {
      const { li, gutter, body } = shell(entry);
      gutter.append(h('span', { class: 'tick', text: fmt.short(entry.at) }));
      body.append(renderTool(entry, ctx), ...decorations(entry, ctx));
      return li;
    },
    notice(entry, ctx) {
      const { li, body, time } = shell(entry);
      const open = entry.detail && ctx.expanded(entry);
      const toggle = () => ctx.toggle(entry.id, !open);
      body.append(h('div', entry.detail
        ? { class: 'notice', role: 'button', tabindex: 0, 'aria-expanded': String(!!open), onclick: toggle, onkeydown: activateKeys(toggle) }
        : { class: 'notice' }, h('span', { text: entry.text }), time));
      if (open) body.append(h('div', { class: 'prose thought' }, ctx.markdown(entry.detail)));
      return li;
    },
    error(entry, ctx) {
      const { li, gutter, body, time } = shell(entry);
      gutter.append(h('span', { class: 'tick', text: fmt.short(entry.at) }));
      body.append(
        h('header', { class: 'meta' }, h('span', { class: 'who', text: 'Error' }), time, ctx.actions(entry)),
        h('div', { class: 'errbox', text: entry.text }));
      return li;
    },
  };
  for (const [kind, render] of Object.entries(builtin)) cockpit.contribute('timeline.renderer', { kind, render, builtin: true, order: -1000 });

  // toolView is how a tool's calls read: as a plugin says, if one does.
  function toolView(name) {
    const custom = cockpit.contributions('tool.view').filter((t) => t.tool === name).pop();
    if (!custom) return null;
    const copy = (entry) => entry && { ...entry, tool: entry.tool && { ...entry.tool } };
    return {
      summary: custom.summary ? (entry) => cockpit.safely(() => custom.summary(copy(entry))) : null,
      render: custom.render ? (entry, helpers) => cockpit.safely(() => custom.render(copy(entry), helpers)) : null,
    };
  }

  function renderTool(entry, ctx) {
    const tool = entry.tool || {};
    const state = toolState(entry, ctx.live);
    // A plugin may say how its tool's calls read: a line, and the open call.
    const custom = toolView(tool.name);
    const summary = custom?.summary?.(entry);
    const open = ctx.expanded(entry);
    const shellTool = (tool.name || '').toLowerCase() === 'bash';
    const failed = state === 'failed';
    const exit = tool.exit_code;

    const meta = h('span', { class: 'tmeta' });
    if (state === 'running' && tool.started) {
      meta.append(h('span', { class: 'live', data: { since: tool.started }, text: fmt.duration(Date.now() - Date.parse(tool.started)) }));
    } else if (tool.started && tool.finished) {
      meta.append(h('span', { text: fmt.duration(Date.parse(tool.finished) - Date.parse(tool.started)) }));
    }
    if (exit != null && exit !== 0) meta.append(h('span', { class: 'exit', text: `Exit ${exit}` }));
    if (state !== 'done' && state !== 'running') meta.append(h('span', { class: `word ${state}`, text: TOOL_WORD[state] }));

    const toggle = () => ctx.toggle(entry.id, !open);
    const card = h('div', { class: 'tool', data: { state, exit: exit != null && exit !== 0 ? 'nonzero' : null } },
      h('div', { class: 'tool-head', role: 'button', tabindex: 0, 'aria-expanded': String(open), onclick: toggle, onkeydown: activateKeys(toggle) },
        h('span', { class: 'tstate', 'aria-label': TOOL_WORD[state], text: TOOL_GLYPH[state] }),
        h('span', { class: 'tname', text: tool.name || 'Tool' }),
        h('span', { class: `tinput${shellTool ? ' cmd' : ''}`, title: tool.input || '', text: typeof summary === 'string' ? summary : (tool.input || '').split('\n')[0] }),
        meta,
        h('span', { class: 'actions' }, tool.input && button('Copy', () => ctx.copy(tool.input, shellTool ? 'Command copied' : 'Copied'), 'Copy the command')),
        h('span', { class: 'chev', 'aria-hidden': 'true' })));

    const rendered = open && custom?.render ? custom.render(entry, { h, fmt, stream: (label, text) => stream(label, text, 'out'), copy: ctx.copy }) : null;
    if (rendered instanceof Node) {
      card.append(h('div', { class: 'tool-body plugin-view' }, rendered));
    } else if (open) {
      const panel = h('div', { class: 'tool-body' });
      if (tool.input && (tool.input.includes('\n') || tool.input.length > 90)) panel.append(stream('Input', tool.input, 'input'));
      if (tool.error) panel.append(h('div', { class: 'tool-error', text: tool.error }));
      if (tool.output) panel.append(stream('Output', tool.output, 'out'));
      if (tool.stderr) panel.append(stream('Stderr', tool.stderr, 'err'));
      if (!tool.output && !tool.stderr && !tool.error) {
        panel.append(h('div', { class: 'tool-empty', text: state === 'running' || state === 'queued' ? 'Waiting for output…' : 'No output' }));
      }
      card.append(panel);
    } else if (failed && tool.error) {
      card.append(h('div', { class: 'tool-peek', text: tool.error.split('\n')[0] }));
    }
    return card;
  }

  function stream(label, text, kind) {
    const count = fmt.lines(text);
    return h('section', { class: `stream ${kind}` },
      h('header', null,
        h('span', { text: `${label} · ${count} ${count === 1 ? 'line' : 'lines'}` }),
        button('Copy', () => cockpit.copy(text, `${label} copied`))),
      h('pre', { text }));
  }

  // The actions of the timeline's own: copying what an entry says.
  cockpit.contribute('timeline.action', { id: 'copy', kinds: ['user'], label: 'Copy', order: 20, run: (entry) => cockpit.copy(entry.text, 'Prompt copied') });
  cockpit.contribute('timeline.action', { id: 'copy-answer', kinds: ['assistant'], label: 'Copy', order: 20, run: (entry) => cockpit.copy(entry.text, 'Answer copied') });
  cockpit.contribute('timeline.action', { id: 'copy-error', kinds: ['error'], label: 'Copy', order: 20, run: (entry) => cockpit.copy(entry.text, 'Error copied') });

  // ---------------------------------------------------------------- following the session

  cockpit.on('session:view', (v, { reload } = {}) => {
    if (!reload) {
      unseen = 0;
      jump.hidden = true;
    }
    schedule();
  });
  cockpit.on('session:dirty', (v, id) => { if (v === view()) schedule(id); });
  // A plugin that changes how entries look has them drawn again.
  for (const point of ['timeline.renderer', 'timeline.action', 'timeline.decoration', 'timeline.editor', 'tool.view']) {
    cockpit.on(`point:${point}`, () => schedule());
  }
  cockpit.on('service', (name) => { if (name === 'markdown' || name === 'models' || name === 'edit') schedule(); });

  // Scrolling does not bubble; the capture sees it, whichever element the
  // layout's scroller is now.
  cockpit.listen(document, 'scroll', (event) => {
    if (event.target !== scroller()) return;
    if (unseen && nearBottom()) scrollToBottom();
    cockpit.emit('timeline:scroll');
  }, { capture: true, passive: true });

  // One clock drives every elapsed-time readout.
  cockpit.interval(() => {
    for (const el of list.querySelectorAll('[data-since]')) el.textContent = fmt.duration(Date.now() - Date.parse(el.dataset.since));
  }, 1000);

  function reveal(id) {
    const node = nodes.get(id);
    if (!node) return;
    node.scrollIntoView({ block: 'center', behavior: 'smooth' });
    node.classList.remove('flash');
    void node.offsetWidth;
    node.classList.add('flash');
  }

  function setAllTools(open) {
    const v = view();
    for (const entry of v.entries.values()) {
      if (entry.kind === 'tool' || entry.kind === 'reasoning') v.expanded.set(entry.id, open);
    }
    schedule();
  }

  // promptInView is the prompt whose part of the transcript is in view: at
  // the bottom, the latest; above it, the last one that starts above a third
  // of the way down.
  function promptInView(v) {
    const prompts = v.order.map((id) => v.entries.get(id)).filter((e) => e?.kind === 'user' && nodes.has(e.id));
    if (!prompts.length) return null;
    if (nearBottom()) return prompts[prompts.length - 1];
    const el = scroller();
    const probe = el.getBoundingClientRect().top + el.clientHeight / 3;
    let found = prompts[0];
    for (const entry of prompts) {
      if (nodes.get(entry.id).getBoundingClientRect().top > probe) break;
      found = entry;
    }
    return found;
  }

  // forget takes entries out that a rewind removed.
  function forget(ids) {
    for (const id of ids) {
      pending.ids.delete(id);
      nodes.get(id)?.remove();
      nodes.delete(id);
    }
  }

  cockpit.provide('timeline', {
    schedule, flushNow, reveal, scrollToBottom, nearBottom, promptInView, forget, setAllTools,
    node: (id) => nodes.get(id), scroller, toolState,
  });

  // ---------------------------------------------------------------- commands, keys, palette

  cockpit.commands.register({ name: 'expand', help: 'Expand tool output and thinking', order: 190, run: () => setAllTools(true) });
  cockpit.commands.register({ name: 'collapse', help: 'Collapse tool output and thinking', order: 200, run: () => setAllTools(false) });
  cockpit.keys.register({ key: 'e', views: ['sessions'], run: () => setAllTools(true) });
  cockpit.keys.register({ key: 'E', views: ['sessions'], run: () => setAllTools(false) });
  cockpit.keys.register({ key: 'g', views: ['sessions'], run: () => scrollToBottom() });
  cockpit.contribute('help.keys', { keys: ['E', ' ', '⇧', 'E'], text: 'Expand / collapse tool output', order: 170 });
  cockpit.contribute('help.keys', { keys: ['G'], text: 'Jump to latest', order: 180 });
  cockpit.palette.register({ group: 'Actions', icon: '↓', label: 'Jump to latest', hint: 'G', order: 110, run: scrollToBottom });
  cockpit.palette.register({ group: 'Actions', icon: '▸', label: 'Expand all tool output', hint: 'E', order: 120, run: () => setAllTools(true) });
  cockpit.palette.register({ group: 'Actions', icon: '▾', label: 'Collapse all tool output', hint: 'Shift E', order: 130, run: () => setAllTools(false) });

  if (view()) schedule();
}
