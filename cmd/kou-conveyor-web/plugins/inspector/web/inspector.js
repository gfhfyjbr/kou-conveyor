// inspector: a side panel (layout.panel "inspector") of sections. Every
// section is a contribution to "inspector.section" — { id, title, order,
// shown(view), render(view) → node or text } — its own (Run, Tokens, Tools,
// Session, Runner log) as much as a plugin's (cockpit.inspector.register).
// A section with the id of another takes its place.
const PANEL = '<svg viewBox="0 0 16 16" aria-hidden="true"><rect x="1.5" y="2.5" width="13" height="11" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M10 2.5v11" stroke="currentColor" stroke-width="1.4"/></svg>';
const API_LABEL = { responses: 'Responses API', messages: 'Messages API' };

export default function activate(cockpit) {
  const { h, svg, fmt, ui } = cockpit;
  const { kv } = ui;
  const session = cockpit.use('session');
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const view = () => session.view?.();
  const layout = () => service('layout');

  const sections = h('div', { class: 'ins-sections' });
  const node = h('aside', { class: 'inspector', id: 'inspector', 'aria-label': 'Inspector' },
    h('header', { class: 'ins-head' }, h('span', { class: 'label', text: 'Inspector' }),
      h('button', { class: 'icon', id: 'inspector-close', type: 'button', 'aria-label': 'Close inspector', onclick: () => toggle() }, '×')),
    sections);
  cockpit.contribute('layout.panel', { id: 'inspector', order: 20, width: 'var(--inspector)', node, opened: () => cockpit.render() });

  const button = h('button', { class: 'icon', id: 'inspector-toggle', type: 'button', title: 'Inspector (I)', 'aria-label': 'Toggle inspector', onclick: () => toggle() }, svg(PANEL));
  cockpit.ui.mount('bar.end', { id: 'inspector-toggle', order: 70, node: button });

  const open = () => !!layout()?.panelOpen?.('inspector');
  function toggle() {
    layout()?.togglePanel?.('inspector');
  }

  // Each section keeps its element, so its body is replaced in place.
  const boxes = new WeakMap();
  function render() {
    if (!open()) return;
    const v = view();
    if (!v) return;
    const summary = session.summary(v);
    const list = cockpit.contributions('inspector.section', { unique: 'id' })
      .filter((section) => !section.shown || cockpit.safely(() => section.shown(summary)));
    const nodes = list.map((section) => {
      let box = boxes.get(section);
      if (!box) {
        box = h('section', { class: 'ins', data: { plugin: section.plugin || null, section: section.id } });
        boxes.set(section, box);
      }
      const body = cockpit.safely(() => section.render(summary, v));
      const title = typeof section.title === 'function' ? section.title(summary, v) : section.title || section.plugin;
      if (section.raw && body instanceof Node) box.replaceChildren(body);
      else box.replaceChildren(title instanceof Node ? title : h('h2', { text: title }), body instanceof Node ? body : h('p', { class: 'none', text: body == null ? '' : String(body) }));
      return box;
    });
    sections.replaceChildren(...nodes);
  }
  cockpit.on('render', render);
  cockpit.on('point:inspector.section', () => cockpit.render());

  // ---------------------------------------------------------------- its own sections

  const kvs = (...rows) => rows;
  const tally = (label, n) => h('div', { data: { zero: n === 0 ? 'true' : null } }, h('b', { text: String(n) }), h('span', { text: label }));
  const meterFill = (percent) => {
    const fill = h('i');
    fill.style.width = `${percent}%`;
    return fill;
  };
  const host = (url) => {
    try {
      return new URL(url).host;
    } catch {
      return url;
    }
  };
  const shellQuote = (value) => (/^[\w./-]+$/.test(value) ? value : `'${value.replace(/'/g, `'\\''`)}'`);
  const frag = (...children) => {
    const f = document.createDocumentFragment();
    f.append(...children.flat().filter(Boolean));
    return f;
  };

  cockpit.contribute('inspector.section', {
    id: 'run', title: 'Run', order: 10,
    render: (summary, v) => {
      const state = session.state;
      const phase = session.runPhase(v);
      const models = service('models');
      const link = session.currentWorkspace()?.connection || state.config?.connection;
      const next = session.nextModel(v) || session.defaultModel();
      // Through the gateway, the API follows the model of the next prompt.
      const api = link?.source === 'gateway'
        ? models?.find?.(next)?.api || (/(^|\/)claude/i.test(next) ? 'messages' : 'responses')
        : link?.provider_type;
      const via = service('connection')?.viaGateway?.(link?.base_url, link);
      return frag(kvs(
        kv('State', session.PHASE_LABEL[phase] || phase, `state-${phase}`),
        kv('Elapsed', v.run ? fmt.timer(Date.now() - v.run.started) : '—'),
        kv('Activity', v.run?.activity || '—'),
        kv('Effort', service('effort')?.current?.() || state.config?.thinking || '—'),
        kv('Provider', link ? [link.source === 'gateway' ? 'gateway' : link.provider, API_LABEL[api]].filter(Boolean).join(' · ') : '—'),
        kv('Model', next || 'runner default'),
        kv('Default', session.defaultModel() || 'runner default'),
        kv('Endpoint', link?.source === 'gateway' ? `accounts gateway${state.config?.accounts?.url ? ` · ${host(state.config.accounts.url)}` : ''}`
          : link?.base_url ? (via ? `accounts gateway · ${host(link.base_url)}` : host(link.base_url)) : 'provider default'),
        cockpit.has('connection') ? h('div', { class: 'ins-actions' },
          h('button', { class: 'act', type: 'button', onclick: () => service('connection')?.open?.() }, 'Connection…')) : null));
    },
  });

  cockpit.contribute('inspector.section', {
    id: 'tokens', title: 'Tokens', order: 20,
    render: (summary, v) => {
      const usage = v.usage || {};
      const cachedShare = usage.input ? Math.round((usage.cached / usage.input) * 100) : 0;
      return frag(
        kv('Context', fmt.tokens(usage.context ?? NaN)),
        kv('Input', fmt.tokens(usage.input ?? NaN)),
        kv('Cached', usage.input ? `${fmt.tokens(usage.cached)} · ${cachedShare}%` : '—'),
        h('div', { class: 'meter-bar', title: `${cachedShare}% of input tokens were cached` }, meterFill(cachedShare)),
        kv('Output', fmt.tokens(usage.output ?? NaN)),
        kv('Reasoning', fmt.tokens(usage.reasoning ?? NaN)),
        kv('Turns', usage.turns ? String(usage.turns) : '—'),
        h('div', { class: 'ins-actions' },
          h('button', {
            class: 'act', type: 'button', disabled: v.fresh || !!session.runBlocked(v),
            title: 'Summarize the conversation to free context; the next prompt starts from the summary (/compact in the composer)',
            onclick: () => session.compact(),
          }, 'Compact')));
    },
  });

  const GLYPH = { done: '✓', failed: '✕', running: '■', queued: '□', canceled: '⊘', interrupted: '◌' };
  cockpit.contribute('inspector.section', {
    id: 'tools', order: 30,
    title: (summary, v) => h('h2', null, 'Tools ', h('span', { id: 'ins-tools-count', text: String([...v.entries.values()].filter((e) => e.kind === 'tool').length) })),
    render: (summary, v) => {
      const tools = [...v.entries.values()].filter((e) => e.kind === 'tool');
      const stateOf = (e) => service('timeline')?.toolState?.(e, !!v.run) || e.tool?.state || 'queued';
      const count = (s) => tools.filter((e) => stateOf(e) === s).length;
      const recent = tools.slice(-10).reverse();
      return frag(
        h('div', { class: 'tally' },
          tally('Done', count('done')), tally('Failed', count('failed')),
          tally('Live', count('running') + count('queued')), tally('Other', count('canceled') + count('interrupted'))),
        recent.length ? h('ol', { class: 'recent' }, recent.map((e) => {
          const s = stateOf(e);
          return h('li', null, h('button', { type: 'button', data: { state: s }, title: e.tool?.input || '', onclick: () => service('timeline')?.reveal?.(e.id) },
            h('span', { class: 'g', text: GLYPH[s] }),
            h('span', { class: 'n', text: e.tool?.name || 'Tool' }),
            h('span', { class: 'i', text: (e.tool?.input || '').split('\n')[0] })));
        })) : h('p', { class: 'none', text: 'No tool calls yet' }));
    },
  });

  cockpit.contribute('inspector.section', {
    id: 'session', title: 'Session', order: 40,
    render: (summary, v) => frag(
      kv('ID', v.fresh ? 'unsaved' : v.id.slice(0, 13)),
      kv('Entries', String(v.order.length)),
      kv('Started', v.order.length ? fmt.stamp(v.entries.get(v.order[0]).at) : '—'),
      h('div', { class: 'ins-actions' },
        h('button', {
          class: 'act', type: 'button', disabled: v.fresh, 'aria-haspopup': 'menu',
          onclick: (event) => service('menu')?.open?.(event.currentTarget, session.menuItems(v.ws, v.id)),
        }, 'Actions…'),
        h('button', { class: 'act', type: 'button', disabled: v.fresh, onclick: () => cockpit.copy(v.id, 'Session ID copied') }, 'Copy ID'),
        h('button', {
          class: 'act', type: 'button', disabled: v.fresh,
          onclick: () => cockpit.copy(`kou-conveyor-tui -workspace ${shellQuote(session.currentWorkspace()?.path || session.state.config?.workspace || '.')} -session ${v.id}`, 'Resume command copied'),
        }, 'Copy TUI command'))),
  });

  // The runner log keeps its element, so it stays open and scrolled.
  const logCount = h('span', { id: 'ins-log-count', text: '0' });
  const logText = h('pre', { id: 'ins-log' });
  const logBox = h('details', { class: 'log-box' }, h('summary', null, h('h2', null, 'Runner log ', logCount)), logText);
  cockpit.contribute('inspector.section', {
    id: 'log', order: 90, raw: true,
    render: (summary, v) => {
      logCount.textContent = String(v.logs.length);
      const text = v.logs.length ? v.logs.slice(-80).join('\n') : 'Runner diagnostics appear here.';
      if (logText.textContent !== text) logText.textContent = text;
      return logBox;
    },
  });

  cockpit.provide('inspector', { toggle, open, show: () => { if (!open()) toggle(); } });
  cockpit.commands.register({ name: 'inspector', help: 'Show or hide the inspector', order: 220, run: toggle });
  cockpit.keys.register({ key: 'i', views: ['sessions'], run: toggle });
  cockpit.contribute('help.keys', { keys: ['I'], text: 'Inspector', order: 220 });
  cockpit.contribute('palette.provider', {
    id: 'inspector', order: 160,
    items: () => [{ group: 'Actions', icon: '◧', label: open() ? 'Hide inspector' : 'Show inspector', hint: 'I', order: 160, run: toggle }],
  });
  cockpit.on('layout:panel', () => {
    button.setAttribute('aria-pressed', String(open()));
    cockpit.render();
  });
  cockpit.render();
}
