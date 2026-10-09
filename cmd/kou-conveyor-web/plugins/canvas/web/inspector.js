// The canvas's inspector: a panel over the right of the board with what
// is selected — a node's settings (a form made from its preset's schema),
// what it may do with the canvas, where it works and what waits for it; an
// edge's template, with a preview, its mode and its latest messages — or
// the canvas's own settings. Plugins add sections to a node's
// (canvas.inspector). A change applies as it is made, as an operation the
// user can undo. What is being typed is not drawn over: the panel catches
// up once the field is left.
import { form } from './forms.js';
import { glyphOf } from './geometry.js';
import { shownState, stateWord } from './status.js';

const ACCESS = [
  ['none', 'None', 'It is not told of the canvas'],
  ['observe', 'Observe', 'It reads the canvas and its nodes'],
  ['talk', 'Talk', 'It reads, and messages the nodes'],
  ['build', 'Build', 'It makes, wires and moves nodes too'],
  ['admin', 'Admin', 'It deletes any node, and sets the canvas'],
];
const STATES = { pending: 'waits', awaiting_approval: 'to approve', delivered: 'delivered', dropped: 'dropped' };
// The line a message to an agent starts with (the engine's headerOf):
// [canvas] from «Title» (n_id):, or [canvas] reply from ….
const HEADER = /^\[canvas\] (?:reply )?from «[^»]*»(?: \([^)\n]*\))?:\n/;

// renderTemplate is the engine's template (template.go) as far as a
// preview needs it.
export function renderTemplate(template, vars) {
  const source = template && template.trim() ? template : '{{text}}';
  return source.replace(/\{\{\s*([A-Za-z0-9_.-]+)\s*\}\}/g, (_, key) => {
    const value = key.split('.').reduce((at, part) => (at == null ? undefined : at[part]), vars);
    if (value == null) return '';
    return typeof value === 'object' ? JSON.stringify(value) : String(value);
  });
}

export function createInspector(env) {
  const { cockpit, h, fmt } = env;
  const titleEl = h('h2', { class: 'cv-ins-title' });
  const close = h('button', { class: 'icon small', type: 'button', title: 'Close (Esc)', 'aria-label': 'Close the inspector', onclick: () => hide() }, '×');
  const body = h('div', { class: 'cv-ins-body' });
  const element = h('aside', { class: 'cv-inspector', hidden: true, 'aria-label': 'Inspector' },
    h('header', { class: 'cv-ins-head' }, titleEl, close), body);
  body.dataset.scroll = 'true';

  let target = null; // { node, pending } | { edge } | { canvas: true }
  let dirty = false;
  let drawn = '';

  const model = () => env.model();
  const failed = (what) => (error) => cockpit.toast(`${what}: ${error.message}`, 'error');
  const editing = () => {
    const active = document.activeElement;
    return element.contains(active) && /^(INPUT|TEXTAREA|SELECT)$/.test(active?.tagName || '');
  };

  function show(next) {
    target = next || null;
    element.hidden = !target;
    drawn = '';
    dirty = false;
    draw();
    env.onToggle?.(!!target);
  }

  function hide() {
    if (!target) return false;
    show(null);
    env.onClose?.();
    return true;
  }

  // refresh draws the panel anew, unless a field of it is being typed in.
  function refresh() {
    if (!target) return;
    if (editing()) {
      dirty = true;
      return;
    }
    draw();
  }
  element.addEventListener('focusout', () => {
    setTimeout(() => {
      if (dirty && !editing()) {
        dirty = false;
        draw();
      }
    }, 0);
  });

  function draw() {
    const m = model();
    if (!target || !m) {
      body.replaceChildren();
      return;
    }
    let parts = null;
    if (target.node) parts = drawNode(m, m.node(target.node));
    else if (target.edge) parts = drawEdge(m, m.edge(target.edge));
    else if (target.canvas) parts = drawCanvas(m);
    if (!parts) {
      // What it showed is gone.
      show(null);
      return;
    }
    const key = target.node || target.edge || 'canvas';
    const scroll = drawn === key ? body.scrollTop : 0;
    body.replaceChildren(...parts.filter(Boolean));
    body.scrollTop = scroll;
    drawn = key;
    if (target.pending) {
      target.pending = false;
      body.querySelector('.cv-ins-pending')?.scrollIntoView({ block: 'nearest' });
    }
  }

  const section = (title, ...children) => h('section', { class: 'cv-ins-section' }, title ? h('h3', { class: 'label', text: title }) : null, ...children);
  const row = (label, value) => h('div', { class: 'cv-ins-row' }, h('span', { text: label }), value instanceof Node ? value : h('b', { text: String(value ?? '') }));
  const button = (label, run, { strong = false, danger = false, title = '' } = {}) =>
    h('button', { class: `act${strong ? ' strong' : ''}${danger ? ' danger' : ''}`, type: 'button', title: title || null, onclick: () => cockpit.safely(run) }, label);

  // ---------------------------------------------------------------- a node

  function drawNode(m, n) {
    if (!n) return null;
    const readOnly = m.readOnly;
    titleEl.replaceChildren(h('span', { class: 'cv-glyph', text: glyphOf(n, env.kinds) }), h('span', { text: n.title || n.id }));
    const st = m.status.get(n.id) || { state: n.kind === 'agent' ? 'idle' : 'stopped' };
    const name = h('input', { type: 'text', maxlength: '120', spellcheck: 'false', 'aria-label': 'Title', disabled: readOnly });
    name.value = n.title || '';
    name.addEventListener('change', () => {
      const value = name.value.trim();
      if (!value || value === n.title) {
        name.value = n.title || '';
        return;
      }
      m.apply([{ op: 'node.update', id: n.id, set: { title: value } }], { label: 'Rename' }).catch(failed('It was not renamed'));
    });
    const kind = describeKind(n);
    const head = section('',
      h('label', { class: 'cv-field' }, h('span', { class: 'cv-field-title', text: 'Title' }), name),
      h('p', { class: 'cv-ins-kind' }, h('span', { text: kind }), h('code', { text: n.id, title: 'Its ID: what agents name it by' })),
      h('p', { class: 'cv-ins-state', data: { state: shownState(st) } }, h('i', { 'aria-hidden': 'true' }),
        h('b', { text: stateWord(shownState(st)) }),
        st.agent?.detected ? h('em', { text: `${st.agent.title || st.agent.id} in its shell`, title: 'What is sent to the node goes to it as a prompt' }) : null,
        st.since ? h('time', { text: fmt.ago(st.since), title: fmt.stamp(st.since) }) : null,
        st.detail ? h('span', { text: st.detail }) : null));
    const parts = [head];
    if (n.proposed) {
      parts.push(section('Proposed', h('p', { class: 'cv-ins-note', text: `«${m.titleOf(n.created_by)}» proposed it: nothing runs for it until it is approved.` }),
        h('div', { class: 'ins-actions' },
          button('Approve', () => m.apply([{ op: 'node.update', id: n.id, set: { proposed: false } }], { label: 'Approve the node' }).catch(failed('It was not approved')), { strong: true }),
          button('Discard', () => m.apply([{ op: 'node.remove', id: n.id }], { label: 'Discard the node' }).catch(failed('It was not discarded'))))));
    }
    parts.push(settingsOf(m, n));
    if (n.kind === 'terminal' || n.kind === 'agent') parts.push(accessOf(m, n));
    parts.push(runtimeOf(m, n));
    parts.push(pendingOf(m, n));
    for (const item of cockpit.contributions('canvas.inspector').sort((a, b) => (a.order ?? 0) - (b.order ?? 0))) {
      const kinds = item.kinds;
      if (kinds?.length && !kinds.some((k) => k === n.kind || k === n.preset || k === `${n.plugin}/${n.preset}`)) continue;
      const node = cockpit.safely(() => item.render(n, { cockpit, h, fmt, model: m, kinds: env.kinds }));
      if (node instanceof Node) parts.push(section(item.title || '', node));
    }
    if (!readOnly) parts.push(actionsOf(m, n, st));
    return parts;
  }

  function describeKind(n) {
    if (n.kind === 'terminal') {
      const def = env.kinds.harness(n);
      return [def?.title?.replace(/…$/, '') || n.preset, n.plugin ? `from ${n.plugin}` : 'terminal'].filter(Boolean).join(' · ');
    }
    if (n.kind === 'agent') return n.preset === 'foreman' ? 'kou agent · the canvas\'s foreman' : 'kou agent';
    if (n.kind === 'source') {
      const def = env.kinds.source(n);
      return [def?.title || n.preset, 'event source', n.plugin ? `from ${n.plugin}` : ''].filter(Boolean).join(' · ');
    }
    return n.kind;
  }

  // schemaOf is the schema of a node's config: its preset's, or the agent's.
  function schemaOf(n) {
    if (n.kind === 'terminal') return env.kinds.harness(n)?.config || null;
    if (n.kind === 'source') return env.kinds.source(n)?.config || null;
    if (n.kind === 'agent') return env.kinds.agent()?.config || null;
    return null;
  }

  function settingsOf(m, n) {
    const config = n.config || {};
    const schema = schemaOf(n);
    const set = (key, value, label = 'Change a setting') =>
      m.apply([{ op: 'node.update', id: n.id, set: { config: { [key]: value } } }], { label }).catch(failed('It was not saved'));
    const children = [];
    if (schema && Object.keys(schema.properties || {}).length) {
      const f = form(h, schema, config, { idPrefix: `cv-ins-${n.id}`, onChange: (key, value) => set(key, value) });
      if (m.readOnly) for (const input of f.node.querySelectorAll('input, select, textarea')) input.disabled = true;
      // The agent's model: the models the connection reaches, offered.
      const modelInput = n.kind === 'agent' ? f.node.querySelector(`#cv-ins-${CSS.escape(n.id)}-model`) : null;
      if (modelInput) {
        const list = env.models?.() || [];
        if (list.length) {
          const id = `cv-models-${n.id}`;
          modelInput.setAttribute('list', id);
          modelInput.placeholder = 'The connection\'s default';
          f.node.append(h('datalist', { id }, ...list.map((x) => h('option', { value: x.id, text: x.name && x.name !== x.id ? x.name : null }))));
        }
      }
      children.push(f.node);
    }
    if (n.kind === 'terminal') {
      // A worktree of its own: true, or { name, branch, base }.
      const wt = config.worktree;
      const on = !!wt;
      const toggle = h('input', { type: 'checkbox', disabled: m.readOnly });
      toggle.checked = on;
      const wtName = h('input', { type: 'text', spellcheck: 'false', placeholder: 'From its title', disabled: m.readOnly || !on, 'aria-label': 'The worktree\'s name' });
      wtName.value = typeof wt === 'object' && wt ? wt.name || '' : '';
      const save = () => {
        const value = !toggle.checked ? null : wtName.value.trim() ? { ...(typeof wt === 'object' && wt ? wt : {}), name: wtName.value.trim() } : true;
        set('worktree', value, toggle.checked ? 'Give it a worktree' : 'Take its worktree away');
      };
      toggle.addEventListener('change', save);
      wtName.addEventListener('change', save);
      children.push(h('label', { class: 'cv-field cv-check' }, toggle, h('span', { class: 'cv-field-title', text: 'A git worktree of its own' })),
        h('label', { class: 'cv-field' }, h('span', { class: 'cv-field-title', text: 'Worktree' }), wtName,
          h('small', { text: 'Under .harness/worktrees, on the branch kou/<name>.' })));
      children.push(h('p', { class: 'cv-ins-note', text: 'Changes apply when it starts again.' }));
    }
    if (!children.length) return null;
    return section('Settings', ...children);
  }

  function accessOf(m, n) {
    const current = n.access || (n.kind === 'terminal' && (!n.preset || n.preset === 'shell' || n.preset === 'command') ? 'talk' : 'build');
    const select = h('select', { 'aria-label': 'Its access to the canvas', disabled: m.readOnly },
      ...ACCESS.map(([value, title]) => h('option', { value, text: title, selected: value === current ? true : null })));
    const note = h('small', { text: ACCESS.find(([value]) => value === current)?.[2] || '' });
    select.addEventListener('change', () => {
      note.textContent = ACCESS.find(([value]) => value === select.value)?.[2] || '';
      m.apply([{ op: 'node.update', id: n.id, set: { access: select.value } }], { label: 'Change its access' }).catch(failed('It was not saved'));
    });
    return section('Access', h('label', { class: 'cv-field' }, h('span', { class: 'cv-field-title', text: 'What its program may do' }), select, note));
  }

  function runtimeOf(m, n) {
    const rt = n.runtime || {};
    const rows = [];
    const terminal = m.terminals.get(n.id);
    if (rt.session) {
      rows.push(row('Session', h('span', { class: 'cv-ins-inline' }, h('code', { text: rt.session.slice(0, 8) }),
        button('Open', () => env.openSession?.(rt.session)))));
    }
    if (rt.agent_session) rows.push(row('Its program\'s session', h('code', { text: rt.agent_session, title: rt.agent_session })));
    if (terminal?.cwd || rt.worktree) rows.push(row('Folder', h('code', { text: env.short(terminal?.cwd || rt.worktree), title: terminal?.cwd || rt.worktree })));
    if (terminal?.running) rows.push(row('Running', h('code', { text: terminal.running })));
    if (rt.branch) {
      rows.push(row('Branch', h('span', { class: 'cv-ins-inline' }, h('code', { text: rt.branch }),
        m.readOnly ? null : button('Remove the worktree', async () => {
          try {
            await m.removeWorktree(n.id);
            cockpit.toast('Worktree removed');
          } catch (error) {
            cockpit.toast(error.status === 409 ? `Not removed: ${error.message}` : error.message, 'error');
          }
        }, { title: 'git worktree remove — only when it has nothing uncommitted' }))));
    }
    if ((n.kind === 'terminal' || n.kind === 'agent') && !m.readOnly) {
      rows.push(row('Token', h('span', { class: 'cv-ins-inline' }, h('span', { text: `#${rt.epoch || 0}` }),
        button('Rotate', async () => {
          try {
            await m.rotate(n.id);
            cockpit.toast('Its old tokens no longer work: it gets a new one when it starts again');
          } catch (error) {
            cockpit.toast(error.message, 'error');
          }
        }, { title: 'Void the tokens its program holds' }))));
    }
    if (n.created_by && n.created_by !== 'user') rows.push(row('Made by', m.titleOf(n.created_by)));
    if (!rows.length) return null;
    return section('Runtime', ...rows);
  }

  function pendingOf(m, n) {
    const list = m.queue.filter((q) => q.to?.node === n.id);
    if (!list.length) return null;
    const items = list.map((q) => {
      const text = (q.text || '').replace(HEADER, '');
      const preview = h('p', { class: 'cv-msg-text', text });
      const actions = h('div', { class: 'ins-actions' });
      const approve = (edited) => m.approve(q.id, edited).catch(failed('It was not let through'));
      if (!m.readOnly) {
        actions.append(
          button(q.state === 'awaiting_approval' ? 'Approve' : 'Send now', () => approve(), { strong: true }),
          button('Edit', async () => {
            let full = q.text || '';
            if (q.truncated) {
              try {
                full = (await m.message(q.id))?.text || full;
              } catch { /* the preview, then */ }
            }
            const area = h('textarea', { class: 'cv-msg-edit', rows: '6', spellcheck: 'false' });
            area.value = full;
            actions.replaceChildren(
              button('Approve as edited', () => approve(area.value), { strong: true }),
              button('Cancel', () => refresh()));
            preview.replaceWith(area);
            area.focus();
          }),
          button('Drop', () => m.drop(q.id).catch(failed('It was not dropped')), { danger: true }));
      }
      return h('li', { class: 'cv-msg', data: { state: q.state } },
        h('div', { class: 'cv-msg-head' },
          h('b', { text: q.from?.node ? `from «${m.titleOf(q.from.node)}»` : 'sent to it' }),
          h('span', { text: [STATES[q.state] || q.state, q.reason].filter(Boolean).join(' · ') }),
          h('time', { text: fmt.short(q.at), title: fmt.stamp(q.at) })),
        preview, actions);
    });
    return h('section', { class: 'cv-ins-section cv-ins-pending' }, h('h3', { class: 'label', text: `Waiting for it · ${list.length}` }), h('ol', { class: 'cv-msgs' }, ...items));
  }

  function actionsOf(m, n, st) {
    const running = ['starting', 'idle', 'busy', 'waiting'].includes(st.state);
    const list = [];
    if (n.kind === 'terminal') {
      list.push(button(running ? 'Restart' : 'Start', () => m.restart(n.id, false).catch(failed('It did not start'))));
      if (running) list.push(button('Stop', () => m.stop(n.id).catch(failed('It did not stop'))));
    } else if (n.kind === 'agent' && st.state === 'busy') {
      list.push(button('Stop the run', () => m.stop(n.id).catch(failed('It did not stop'))));
    } else if (n.kind === 'source' && !n.plugin && n.preset === 'manual') {
      list.push(button('Fire', () => m.fire(n.id, '').catch(failed('It did not fire')), { strong: true }));
    }
    list.push(button('Delete the node', () => env.remove?.(n.id), { danger: true }));
    return section('', h('div', { class: 'ins-actions' }, ...list));
  }

  // ---------------------------------------------------------------- an edge

  function drawEdge(m, e) {
    if (!e) return null;
    const from = m.node(e.from.node);
    const to = m.node(e.to.node);
    titleEl.replaceChildren(h('span', { class: 'cv-glyph', text: '→' }), h('span', { text: 'Edge' }));
    const readOnly = m.readOnly;
    const update = (set, label) => m.apply([{ op: 'edge.update', id: e.id, set }], { label }).catch(failed('The edge was not changed'));
    const ends = section('',
      h('p', { class: 'cv-ins-ends' },
        h('b', { text: from?.title || e.from.node }), h('code', { text: e.from.port }),
        h('span', { text: '→' }),
        h('b', { text: to?.title || e.to.node }), h('code', { text: e.to.port })),
      h('p', { class: 'cv-ins-kind' }, h('code', { text: e.id })));

    const choice = (name, value, options, onPick) => h('div', { class: 'cv-seg', role: 'radiogroup', 'aria-label': name },
      ...options.map(([v, label, title]) => h('button', {
        type: 'button', role: 'radio', 'aria-checked': String(v === value), title, disabled: readOnly,
        onclick: () => { if (v !== value) onPick(v); },
      }, label)));
    const mode = e.mode || 'auto';
    const deliver = e.deliver || 'queue';
    const headerDefault = !!to && (to.kind === 'agent' || (to.kind === 'terminal' && to.preset && to.preset !== 'shell' && to.preset !== 'command'));
    const header = e.header ?? null;
    const headerSelect = h('select', { disabled: readOnly, 'aria-label': 'Where it comes from' },
      h('option', { value: '', text: `As the target wants (${headerDefault ? 'on' : 'off'})`, selected: header === null ? true : null }),
      h('option', { value: 'on', text: 'On', selected: header === true ? true : null }),
      h('option', { value: 'off', text: 'Off', selected: header === false ? true : null }));
    headerSelect.addEventListener('change', () => update({ header: headerSelect.value === '' ? null : headerSelect.value === 'on' }, 'Change the edge\'s header'));

    // The template, and what it makes of the last output of the node it
    // comes from.
    const template = h('textarea', { rows: '4', spellcheck: 'false', placeholder: '{{text}}', disabled: readOnly, 'aria-label': 'Template' });
    template.value = e.template || '';
    const preview = h('pre', { class: 'cv-preview' });
    const out = m.outputs.get(e.from.node);
    const sample = {
      text: out?.preview || 'its output',
      title: out?.title || '',
      data: {},
      from: { title: from?.title || '', id: e.from.node, kind: from?.kind || '' },
      edge: { id: e.id },
      now: new Date().toISOString(),
    };
    const drawPreview = () => {
      const on = header === null ? headerDefault : header;
      const text = renderTemplate(template.value, sample);
      preview.textContent = on ? `[canvas] from «${sample.from.title}» (${sample.from.id}):\n${text}` : text;
    };
    template.addEventListener('input', drawPreview);
    template.addEventListener('change', () => {
      if (template.value !== (e.template || '')) update({ template: template.value }, 'Change the template');
    });
    drawPreview();

    const messages = m.messages.filter((x) => x.edge === e.id).slice(-20).reverse();
    const list = messages.length
      ? h('ol', { class: 'cv-msgs' }, ...messages.map((x) => {
        const text = h('p', { class: 'cv-msg-text', text: (x.text || '').replace(HEADER, '') });
        const more = x.truncated ? h('button', {
          class: 'act', type: 'button',
          onclick: async (event) => {
            event.currentTarget.remove();
            try {
              text.textContent = ((await m.message(x.id))?.text || '').replace(HEADER, '');
            } catch (error) {
              cockpit.toast(error.message, 'error');
            }
          },
        }, 'All of it') : null;
        return h('li', { class: 'cv-msg', data: { state: x.state } },
          h('div', { class: 'cv-msg-head' },
            h('b', { text: STATES[x.state] || x.state }),
            x.reason ? h('span', { text: x.reason }) : null,
            h('time', { text: fmt.short(x.delivered_at || x.at), title: fmt.stamp(x.delivered_at || x.at) })),
          text, more);
      }))
      : h('p', { class: 'none', text: 'Nothing went along it since the page opened.' });

    return [
      ends,
      section('Mode', choice('Mode', mode, [
        ['auto', 'Auto', 'Messages go as they come'],
        ['approve', 'Approve', 'Each message waits for you on its target'],
        ['off', 'Off', 'Nothing goes along it'],
      ], (v) => update({ mode: v }, `Edge: ${v}`))),
      section('When the target is busy', choice('Delivery', deliver, [
        ['queue', 'Wait', 'The message waits until its target is idle'],
        ['now', 'Now', 'The message goes at once, into what runs'],
      ], (v) => update({ deliver: v }, 'Change the delivery'))),
      section('Where it comes from', h('label', { class: 'cv-field' }, h('span', { class: 'cv-field-title', text: 'A line naming the node it comes from' }), headerSelect)),
      section('Template',
        h('label', { class: 'cv-field' }, template,
          h('small', { text: '{{text}} {{title}} {{data.…}} {{from.title}} {{from.id}} {{from.kind}} {{edge.id}} {{now}}' })),
        h('span', { class: 'cv-field-title', text: out ? 'Its last output makes' : 'An output makes' }), preview),
      section('Latest messages', list),
      readOnly ? null : section('', h('div', { class: 'ins-actions' }, button('Delete the edge', () => env.removeEdge?.(e.id), { danger: true }))),
    ];
  }

  // ---------------------------------------------------------------- the canvas

  function drawCanvas(m) {
    const doc = m.doc;
    titleEl.replaceChildren(h('span', { class: 'cv-glyph', text: '◧' }), h('span', { text: 'Canvas' }));
    const readOnly = m.readOnly || !m.exists;
    const name = h('input', { type: 'text', maxlength: '120', spellcheck: 'false', disabled: m.readOnly, 'aria-label': 'Title' });
    name.value = doc.title || '';
    name.addEventListener('change', () => {
      const value = name.value.trim();
      if (!value || value === doc.title) return;
      env.rename?.(value);
    });
    const live = h('input', { type: 'checkbox', disabled: readOnly });
    live.checked = doc.live !== false;
    live.addEventListener('change', () => env.setLive?.(live.checked));
    const settings = structuredClone(doc.settings || {});
    settings.autonomy ||= {};
    settings.routing ||= {};
    const save = () => m.apply([{ op: 'canvas.update', set: { settings } }], { label: 'Change the canvas\'s settings' }).catch(failed('Not saved'));
    const number = (group, key, title, note) => {
      const input = h('input', { type: 'number', min: '1', step: '1', disabled: readOnly });
      input.value = settings[group][key] ?? '';
      input.addEventListener('change', () => {
        const value = Math.round(Number(input.value));
        if (!Number.isFinite(value) || value < 1) {
          input.value = settings[group][key] ?? '';
          return;
        }
        settings[group][key] = value;
        save();
      });
      return h('label', { class: 'cv-field' }, h('span', { class: 'cv-field-title', text: title }), input, note ? h('small', { text: note }) : null);
    };
    const spawn = h('select', { disabled: readOnly, 'aria-label': 'Agents make nodes' },
      ...[['allow', 'Yes'], ['ask', 'Ask me first'], ['deny', 'No']].map(([value, text]) => h('option', { value, text, selected: (settings.autonomy.spawn || 'allow') === value ? true : null })));
    spawn.addEventListener('change', () => {
      settings.autonomy.spawn = spawn.value;
      save();
    });
    return [
      section('',
        h('label', { class: 'cv-field' }, h('span', { class: 'cv-field-title', text: 'Title' }), name),
        h('label', { class: 'cv-field cv-check' }, live, h('span', { class: 'cv-field-title', text: 'Live' })),
        h('small', { class: 'cv-ins-note', text: 'A paused canvas runs no sources, and its messages wait.' })),
      section('Agents',
        h('label', { class: 'cv-field' }, h('span', { class: 'cv-field-title', text: 'Agents make nodes' }), spawn,
          h('small', { text: 'Ask: what they make waits for your approval.' })),
        number('autonomy', 'max_nodes', 'Nodes at most'),
        number('autonomy', 'max_terminals', 'Terminals at most'),
        number('autonomy', 'spawns_per_minute', 'Nodes made a minute'),
        number('autonomy', 'max_depth', 'Depth', 'How many agents deep a node an agent makes can be.')),
      section('Messages',
        number('routing', 'max_hops', 'Hops at most', 'A chain of messages longer than this is a loop: it stops.'),
        number('routing', 'edge_rate_per_minute', 'A minute along an edge'),
        number('routing', 'canvas_rate_per_minute', 'A minute on the canvas')),
      m.exists ? section('', h('p', { class: 'cv-ins-kind' }, h('span', { text: `${doc.nodes.length} nodes · ${doc.edges.length} edges` }), h('code', { text: doc.id }))) : null,
    ];
  }

  return {
    element,
    show,
    hide,
    refresh,
    isOpen: () => !!target,
    target: () => target,
    contains: (el) => element.contains(el),
  };
}
