// inspector: the sections of the session in view — a tab of the sidebar
// ("sidebar.tab" inspector), or, without the sidebar plugin, a side panel
// of its own (layout.panel "inspector"). Every section is a contribution
// to "inspector.section" — { id, title, order, shown(view), render(view) →
// node or text, fold, open, meta(view) } — its own (Run, Tokens, Tools,
// Session, Runner log) as much as a plugin's (cockpit.inspector.register).
// A section with the id of another takes its place.
//
// A section with fold is a bar: its title, and what meta says of it —
// text, or parts { text, tone } — that opens, animated, onto its body. It
// starts closed (or open, with open), stays as the user leaves it, and its
// body is rendered only while it is open. The service's fold(spec) makes
// such a bar for a section to hold: the Plugins and Skills sections hold
// one for the project's and one for the system-wide ones.
const PANEL = '<svg viewBox="0 0 16 16" aria-hidden="true"><rect x="1.5" y="2.5" width="13" height="11" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M10 2.5v11" stroke="currentColor" stroke-width="1.4"/></svg>';
const CHEVRON = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M6 3.5 10.5 8 6 12.5" fill="none" stroke="currentColor" stroke-width="1.6"/></svg>';
const API_LABEL = { responses: 'Responses API', messages: 'Messages API' };

export default function activate(cockpit) {
  const { h, svg, fmt, ui } = cockpit;
  const { kv } = ui;
  const session = cockpit.use('session');
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const view = () => session.view?.();
  const layout = () => service('layout');

  const sections = h('div', { class: 'ins-sections', id: 'inspector-sections' });
  const header = h('header', { class: 'ins-head' }, h('span', { class: 'label', text: 'Inspector' }),
    h('button', { class: 'icon', id: 'inspector-close', type: 'button', 'aria-label': 'Close inspector', onclick: () => toggle() }, '×'));
  const node = h('aside', { class: 'inspector', id: 'inspector', 'aria-label': 'Inspector' }, header);
  const button = h('button', { class: 'icon', id: 'inspector-toggle', type: 'button', title: 'Inspector (I)', 'aria-label': 'Toggle inspector', onclick: () => toggle() }, svg(PANEL));

  // A tab of the sidebar, while there is one.
  let tab = null; // the tab that shows the sections
  cockpit.contribute('sidebar.tab', {
    id: 'inspector', title: 'Inspector', icon: PANEL, order: 10, multiple: false, key: 'I',
    description: 'The run, its tokens and tools, the session, the runner log and the plugins.',
    create(handle) {
      tab = handle;
      return {
        node: sections, title: 'Inspector',
        shown: () => cockpit.renderNow(),
        dispose: () => { if (tab === handle) tab = null; },
      };
    },
  });
  // Without it, a panel of its own, as the inspector was.
  let mode = '';
  let panelOff = null;
  let buttonOff = null;
  function placeIt() {
    const next = cockpit.has('sidebar') ? 'tab' : 'panel';
    if (next === mode) return;
    mode = next;
    if (mode === 'panel') {
      node.append(sections);
      panelOff = cockpit.contribute('layout.panel', { id: 'inspector', order: 20, width: 'var(--inspector)', node, opened: () => cockpit.render() });
      buttonOff = cockpit.ui.mount('bar.end', { id: 'inspector-toggle', order: 70, node: button });
    } else {
      panelOff?.();
      buttonOff?.();
      panelOff = buttonOff = null;
    }
    cockpit.render();
  }
  cockpit.on('service', (name) => { if (name === 'sidebar') placeIt(); });
  placeIt();

  const sidebar = () => (mode === 'tab' ? service('sidebar') : null);
  const open = () => (mode === 'tab' ? !!tab?.visible() : !!layout()?.panelOpen?.('inspector'));
  function toggle() {
    if (sidebar()) sidebar().toggle('inspector');
    else layout()?.togglePanel?.('inspector');
  }
  function show() {
    if (sidebar()) sidebar().open('inspector', { reuse: true, focus: false });
    else if (!open()) toggle();
  }

  // ---------------------------------------------------------------- bars

  // put gives parent those children, unless it holds them already: a node
  // put in again would lose the transition it runs.
  const same = (parent, nodes) => parent.childNodes.length === nodes.length && nodes.every((node, i) => parent.childNodes[i] === node);
  const put = (parent, nodes) => { if (!same(parent, nodes)) parent.replaceChildren(...nodes); };
  const setText = (el, text) => {
    text = text == null ? '' : String(text);
    if (el.textContent !== text) el.textContent = text;
  };
  const openKey = (id) => `inspector.open.${id}`;
  let bars = 0;

  // fold makes a bar that opens onto its body: { node, bar, body, isOpen(),
  // set(open), toggle(), title(text), hint(text), meta(parts) }. With an id
  // it stays as the user left it. beforeOpen fills the body before it
  // opens; toggled(open) hears it open and close.
  function fold({ id = '', title = '', hint = '', meta = '', open: opened = false, level = 1, beforeOpen = null, toggled = null } = {}) {
    let isOpen = id ? !!cockpit.prefs.get(openKey(id), opened) : !!opened;
    const n = ++bars;
    const titleNode = h('span', { class: 'ins-bar-title' });
    const hintNode = h('span', { class: 'ins-bar-hint' });
    const metaNode = h('span', { class: 'ins-bar-meta' });
    const bar = h('button', {
      type: 'button', class: 'ins-bar', id: `ins-bar-${n}`, 'aria-expanded': String(isOpen), 'aria-controls': `ins-fold-${n}`,
      onclick: () => set(!isOpen),
    }, titleNode, hintNode, metaNode, h('span', { class: 'ins-chev' }, svg(CHEVRON)));
    const body = h('div', { class: 'ins-fold-inner' });
    const region = h('div', { class: 'ins-fold-body', id: `ins-fold-${n}`, role: 'region', 'aria-labelledby': bar.id },
      h('div', { class: 'ins-fold-clip' }, body));
    const node = h('div', { class: `ins-fold level-${level}`, data: { open: String(isOpen), fold: id || null } }, bar, region);
    let shownMeta = null;
    function set(next) {
      next = !!next;
      if (next === isOpen) return;
      isOpen = next;
      if (id) cockpit.prefs.set(openKey(id), isOpen);
      bar.setAttribute('aria-expanded', String(isOpen));
      // What the bar opens onto is there before it opens.
      if (isOpen && beforeOpen) cockpit.safely(beforeOpen);
      node.dataset.open = String(isOpen);
      if (toggled) cockpit.safely(toggled, isOpen);
    }
    const handle = {
      node, bar, body, set,
      isOpen: () => isOpen,
      toggle: () => set(!isOpen),
      title(text) {
        setText(titleNode, text);
        return handle;
      },
      hint(text) {
        setText(hintNode, text);
        hintNode.hidden = !text;
        return handle;
      },
      // meta takes text, or parts: text or { text, tone, title }, tone one
      // of warn, err, ok, accent.
      meta(parts) {
        const list = (Array.isArray(parts) ? parts : [parts])
          .filter((part) => part != null && part !== '' && part !== false)
          .map((part) => (typeof part === 'object' ? part : { text: String(part) }));
        const key = JSON.stringify(list);
        if (key !== shownMeta) {
          shownMeta = key;
          metaNode.replaceChildren(...list.map((part) => h('span', { data: { tone: part.tone || null }, title: part.title || null, text: part.text })));
        }
        return handle;
      },
    };
    handle.title(title).hint(hint).meta(meta);
    return handle;
  }

  // Each section keeps its element, so its body is replaced in place.
  const boxes = new WeakMap(); // section → its element, or its bar
  const folded = new Map(); // id → the bar of the section shown with it
  function render() {
    if (!open()) return;
    const v = view();
    if (!v) return;
    const summary = session.summary(v);
    const list = cockpit.contributions('inspector.section', { unique: 'id' })
      .filter((section) => !section.shown || cockpit.safely(() => section.shown(summary)));
    const nodes = list.map((section) => (section.fold ? foldOf(section, summary, v) : boxOf(section, summary, v)));
    put(sections, nodes);
  }
  cockpit.on('render', render);

  function boxOf(section, summary, v) {
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
  }

  // A folded section is a bar, whose body is rendered while it is open.
  function foldOf(section, summary, v) {
    let bar = boxes.get(section);
    if (!bar) {
      bar = fold({ id: section.id, open: !!section.open, beforeOpen: () => fill(section, bar) });
      bar.node.classList.add('ins');
      bar.node.dataset.section = section.id;
      if (section.plugin) bar.node.dataset.plugin = section.plugin;
      boxes.set(section, bar);
    }
    folded.set(section.id, bar);
    const title = typeof section.title === 'function' ? cockpit.safely(() => section.title(summary, v)) : section.title || section.plugin;
    bar.title(title instanceof Node ? title.textContent : title);
    bar.meta(section.meta ? cockpit.safely(() => section.meta(summary, v)) ?? '' : '');
    if (bar.isOpen()) fill(section, bar, summary, v);
    return bar.node;
  }

  function fill(section, bar, summary, v) {
    if (!v) {
      v = view();
      if (!v) return;
      summary = session.summary(v);
    }
    const body = cockpit.safely(() => section.render(summary, v));
    put(bar.body, [body instanceof Node ? body : h('p', { class: 'none', text: body == null ? '' : String(body) })]);
  }

  // expand opens (or, with false, closes) the bar of a section, now or when
  // it shows.
  function expand(id, opened = true) {
    cockpit.prefs.set(openKey(id), !!opened);
    folded.get(id)?.set(!!opened);
    cockpit.render();
  }
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

  // The runner log keeps its element, so it stays scrolled.
  const logText = h('pre', { id: 'ins-log', class: 'log-text' });
  cockpit.contribute('inspector.section', {
    id: 'log', title: 'Runner log', order: 90, fold: true,
    meta: (summary, v) => String(v.logs.length),
    render: (summary, v) => {
      const text = v.logs.length ? v.logs.slice(-80).join('\n') : 'Runner diagnostics appear here.';
      if (logText.textContent !== text) logText.textContent = text;
      return logText;
    },
  });

  cockpit.provide('inspector', { toggle, open, show, fold, expand });
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
  cockpit.on('sidebar:tab', () => cockpit.render());
  cockpit.render();
}
