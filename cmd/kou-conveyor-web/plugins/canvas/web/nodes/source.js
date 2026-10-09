// The body of an event source node: what it watches, the events it gave
// lately, and what its program last said. A manual source has its button
// here, a webhook its address.

const KEEP = 30;

export function createSourceBody(node, ctx) {
  const { h, fmt, model } = ctx;
  let current = node;
  let disposed = false;

  const what = h('div', { class: 'cv-src-what' });
  const text = h('input', { type: 'text', spellcheck: 'false', autocomplete: 'off', 'aria-label': 'What it fires' });
  const fire = h('form', {
    class: 'cv-fire', hidden: true,
    onsubmit: async (event) => {
      event.preventDefault();
      try {
        await model.fire(current.id, text.value);
        text.value = '';
      } catch (error) {
        ctx.toast(error.message, 'error');
      }
    },
  }, text, h('button', { class: 'act strong', type: 'submit' }, 'Fire'));
  const list = h('ol', { class: 'cv-events', 'aria-label': 'Its latest events' });
  const log = h('div', { class: 'cv-src-log' });
  const box = h('div', { class: 'cv-source' }, what, fire, list, log);
  list.dataset.scroll = 'true';

  const def = () => ctx.kinds.source(current);
  const config = () => current.config || {};

  // The events it gave: those the page heard of, then what comes.
  const events = [];
  const seen = new Set();
  for (const m of model.messages) {
    if (m.from?.node !== current.id || seen.has(m.chain || m.id)) continue;
    seen.add(m.chain || m.id);
    events.push({ at: m.at, port: m.from.port, title: m.title || '', text: m.text || '' });
  }
  events.splice(0, Math.max(0, events.length - KEEP));

  const offOutput = model.on('output', (event) => {
    if (event.node !== current.id) return;
    if (event.log) {
      drawLog();
      return;
    }
    events.push({ at: event.at, port: event.port, title: event.title || '', text: event.preview || '' });
    if (events.length > KEEP) events.shift();
    drawEvents();
  });

  function describe() {
    const d = def();
    const c = config();
    switch (current.plugin ? '' : current.preset) {
      case 'manual':
        return 'Fires what you write, or its text.';
      case 'timer':
        return c.at ? `Every day at ${c.at}` : `Every ${c.every || '15m'}`;
      case 'files':
        return c.pattern ? `When ${c.pattern} changes` : 'Set the files it watches in the inspector';
      case 'webhook':
        return '';
      default:
        return [d?.description || '', d?.mode === 'stream' ? 'streams' : d?.interval ? `every ${d.interval}` : ''].filter(Boolean).join(' · ');
    }
  }

  function drawWhat() {
    const hook = model.hooks.get(current.id);
    if (!current.plugin && current.preset === 'webhook') {
      const url = hook ? `${location.origin}${hook}` : '';
      if (!hook && !current.proposed) model.refreshHooks();
      what.replaceChildren(
        h('span', { class: 'cv-src-label', text: 'POST to' }),
        url ? h('code', { class: 'cv-hook', text: url, title: url }) : h('span', { text: 'its address comes once it runs' }),
        url ? h('button', { class: 'act', type: 'button', onclick: () => ctx.cockpit.copy(url, 'Address copied') }, 'Copy') : null);
      return;
    }
    const line = describe();
    what.replaceChildren(h('span', { text: line || '' }));
    if (!def()) what.replaceChildren(h('span', { class: 'cv-warn', text: `No source ${current.plugin ? `${current.plugin}/` : ''}${current.preset}: is its plugin on?` }));
  }

  function drawEvents() {
    const items = events.slice().reverse().map((e) => h('li', null,
      h('time', { text: fmt.short(e.at), title: fmt.stamp(e.at) }),
      e.port && e.port !== 'out' ? h('span', { class: 'cv-port-tag', text: e.port }) : null,
      h('span', { class: 'cv-ev-text', text: (e.title || e.text || '—').replace(/\s+/g, ' ').trim(), title: e.text || e.title || '' })));
    if (!items.length) items.push(h('li', { class: 'cv-none', text: 'No events yet.' }));
    list.replaceChildren(...items);
  }

  function drawLog() {
    const lines = model.logs.get(current.id) || [];
    const last = lines[lines.length - 1];
    const st = model.status.get(current.id);
    const textOf = st?.state === 'error' && st.detail ? st.detail : last?.text || '';
    log.textContent = textOf;
    log.hidden = !textOf;
    log.dataset.level = st?.state === 'error' ? 'error' : '';
  }

  function draw() {
    const manual = !current.plugin && current.preset === 'manual';
    fire.hidden = !manual;
    text.placeholder = config().text ? `Fire: ${config().text}` : 'What it fires';
    text.disabled = !!current.proposed;
    drawWhat();
    drawLog();
  }

  draw();
  drawEvents();
  return {
    node: box,
    update(next) {
      current = next;
      draw();
    },
    status: draw,
    lod() {},
    shown() {},
    hidden() {},
    focus() {
      if (!fire.hidden) {
        text.focus();
        return true;
      }
      return false;
    },
    runtime: drawWhat,
    contains: (target) => box.contains(target),
    menu() {
      return [!current.plugin && current.preset === 'manual' && {
        icon: '⚡', label: 'Fire', detail: config().text || 'with no text',
        run: () => model.fire(current.id, '').catch((error) => ctx.toast(error.message, 'error')),
      }].filter(Boolean);
    },
    dispose() {
      disposed = true;
      offOutput();
    },
  };
}
