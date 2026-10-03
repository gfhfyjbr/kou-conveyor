// The body of a kou agent node: a session of the cockpit's own agent, kept
// small — a compact feed of its transcript (its tail read once, then what
// the canvas's events add) and a field to write to it. Enter sends (a run
// starts, or the message waits for the one running), ⌘Enter sends now,
// into the run, and Esc Esc stops it. The whole session, with everything
// the transcript can do, is a click away (Open session).

// The line a message from the canvas starts with (the engine's headerOf):
// [canvas] from «Title» (n_id):, or [canvas] reply from ….
const ORIGIN = /^\[canvas\] ((?:reply )?from) «([^»]*)»(?: \([^)\n]*\))?:\s*/;
const FAILED = new Set(['failed', 'error']);
const RUNNING = new Set(['queued', 'running']);

export function createAgentBody(node, ctx) {
  const { h, fmt, model } = ctx;
  let current = node;
  let level = 'full';
  let visible = false;
  let loaded = false;
  let lastEscape = 0;
  let disposed = false;
  const rows = new Map(); // entry → its row

  const feed = h('div', { class: 'cv-feed', role: 'log', 'aria-label': 'What the agent did' });
  const empty = h('p', { class: 'cv-feed-empty', text: 'Nothing yet: write to it, or wire something into it.' });
  feed.append(empty);
  const input = h('textarea', {
    class: 'cv-input', rows: '2', spellcheck: 'false',
    'aria-label': 'Message the agent', placeholder: 'Message it… ⏎ send · ⌘⏎ now · Esc Esc stop',
  });
  const working = h('span', { class: 'cv-working', hidden: true }, h('span', { class: 'meter', 'aria-hidden': 'true' }, h('i'), h('i'), h('i'), h('i'), h('i')), h('span', { class: 'cv-working-text' }));
  const model_ = h('button', { class: 'cv-chip', type: 'button', title: 'Its model: change it in the inspector', onclick: () => ctx.inspect() });
  const stop = h('button', { class: 'act', type: 'button', hidden: true, title: 'Stop the run (Esc Esc)', onclick: () => stopRun() }, 'Stop');
  const send = h('button', { class: 'act strong', type: 'button', title: 'Send (⏎); ⌘⏎ sends into the run now', onclick: () => submit(false) }, 'Send');
  const bar = h('div', { class: 'cv-agent-bar' }, working, model_, h('span', { class: 'cv-gap' }), stop, send);
  const box = h('div', { class: 'cv-agent' }, feed, h('div', { class: 'cv-agent-input' }, input, bar));

  const status = () => model.status.get(current.id) || { state: 'idle' };

  // ---------------------------------------------------------------- the feed

  async function load() {
    if (loaded) return;
    loaded = true;
    const list = await model.feed(current.id);
    if (disposed) return;
    // The list holds what the events brought meanwhile too, in order.
    for (const row of rows.values()) row.remove();
    rows.clear();
    for (const entry of list) put(entry);
    toEnd(true);
  }

  // put draws an entry, or draws it anew where it was.
  function put(entry) {
    if (!entry?.id) return;
    const row = rowOf(entry);
    const was = rows.get(entry.id);
    if (was) was.replaceWith(row);
    else feed.append(row);
    rows.set(entry.id, row);
    empty.remove();
    while (rows.size > 200) {
      const [first, el] = rows.entries().next().value;
      rows.delete(first);
      el.remove();
    }
  }

  function rowOf(entry) {
    switch (entry.kind) {
      case 'user': {
        const text = entry.text || '';
        const origin = ORIGIN.exec(text);
        return h('div', { class: 'cv-e cv-e-user', title: text },
          origin ? h('span', { class: 'cv-origin', text: `${origin[1]} ${origin[2]}` }) : h('span', { class: 'cv-origin you', text: 'you' }),
          h('span', { class: 'cv-e-text', text: (origin ? text.slice(origin[0].length) : text).replace(/\s+/g, ' ').trim() }));
      }
      case 'assistant': {
        const body = ctx.markdown(entry.text || '');
        return h('div', { class: 'cv-e cv-e-answer', data: { phase: entry.phase || null } }, body,
          entry.truncated ? h('small', { class: 'cv-e-more', text: 'Cut short: Open session shows it whole.' }) : null);
      }
      case 'reasoning':
        return h('div', { class: 'cv-e cv-e-thought', title: entry.text || '' }, h('span', { text: '· thinking' }));
      case 'tool': {
        const tool = entry.tool || {};
        const failed = FAILED.has(tool.state);
        const runningNow = RUNNING.has(tool.state);
        const took = tool.started && tool.finished ? fmt.duration(Date.parse(tool.finished) - Date.parse(tool.started)) : '';
        const mark = runningNow ? '…' : failed ? `✕${tool.exit_code ? ` ${tool.exit_code}` : ''}` : '✓';
        return h('div', { class: 'cv-e cv-e-tool', data: { state: failed ? 'failed' : runningNow ? 'running' : 'done' }, title: tool.summary || '' },
          h('span', { class: 'cv-e-name', text: `▸ ${tool.name || 'tool'}` }),
          tool.summary ? h('span', { class: 'cv-e-text', text: tool.summary.replace(/\s+/g, ' ') }) : null,
          h('span', { class: 'cv-e-mark', text: [mark, took].filter(Boolean).join(' ') }));
      }
      case 'error':
        return h('div', { class: 'cv-e cv-e-error', text: entry.text || 'It failed' });
      default:
        return h('div', { class: 'cv-e cv-e-notice', text: entry.text || entry.kind });
    }
  }

  const nearEnd = () => feed.scrollHeight - feed.scrollTop - feed.clientHeight < 48;
  function toEnd(force = false) {
    if (force || nearEnd()) feed.scrollTop = feed.scrollHeight;
  }

  const offAgent = model.on('agent', (event) => {
    if (event.node !== current.id || !event.entry) return;
    const end = nearEnd();
    put(event.entry);
    if (end) toEnd(true);
  });

  // ---------------------------------------------------------------- writing

  async function submit(now) {
    const text = input.value.trim();
    if (!text) return;
    input.value = '';
    try {
      await model.send(current.id, { text, deliver: now ? 'now' : '' });
    } catch (error) {
      input.value = text;
      ctx.toast(error.message, 'error');
    }
  }

  async function stopRun() {
    try {
      await model.stop(current.id);
    } catch (error) {
      ctx.toast(error.message, 'error');
    }
  }

  input.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      event.stopPropagation();
      submit(event.metaKey || event.ctrlKey);
    } else if (event.key === 'Escape') {
      const now = Date.now();
      if (now - lastEscape < 600 && status().state === 'busy') {
        event.preventDefault();
        event.stopPropagation();
        lastEscape = 0;
        stopRun();
        return;
      }
      lastEscape = now;
    }
  });
  // A wheel over the feed scrolls it, not the canvas, when it can.
  feed.dataset.scroll = 'true';

  // ---------------------------------------------------------------- state

  function draw() {
    const st = status();
    const busy = st.state === 'busy' || st.state === 'starting';
    working.hidden = !busy;
    stop.hidden = !busy;
    const text = st.activity || (st.state === 'starting' ? 'starting' : 'working');
    const label = working.querySelector('.cv-working-text');
    if (label.textContent !== text) label.textContent = text;
    const config = current.config || {};
    const name = config.model ? ctx.modelTitle(config.model) : 'default model';
    const chip = [name, config.sandbox === 'worktree' ? 'worktree' : ''].filter(Boolean).join(' · ');
    if (model_.textContent !== chip) model_.textContent = chip;
    input.disabled = !!current.proposed;
  }

  function lastAnswer() {
    const list = model.feeds.get(current.id) || [];
    for (let i = list.length - 1; i >= 0; i--) if (list[i].kind === 'assistant' && list[i].text) return list[i].text;
    return '';
  }

  function menu() {
    const sessionID = current.runtime?.session || '';
    return [
      sessionID && { icon: '↗', label: 'Open session', detail: 'The whole transcript: edits, branches, changes', run: () => ctx.openSession(sessionID) },
      status().state === 'busy' && { icon: '■', label: 'Stop the run', hint: 'Esc Esc', run: stopRun },
      { icon: '⧉', label: 'Copy the last answer', disabled: !lastAnswer(), run: () => ctx.cockpit.copy(lastAnswer(), 'Answer copied') },
    ].filter(Boolean);
  }

  draw();
  return {
    node: box,
    update(next) {
      current = next;
      draw();
    },
    status: draw,
    lod(nextLevel) {
      level = nextLevel;
      if (level !== 'low' && visible) load();
    },
    shown() {
      visible = true;
      if (level !== 'low') load();
    },
    hidden() {
      visible = false;
    },
    focus() {
      input.focus();
      return true;
    },
    contains: (target) => box.contains(target),
    menu,
    dispose() {
      disposed = true;
      offAgent();
    },
  };
}
