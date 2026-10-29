// inspector: the sections of the session in view — a tab of the sidebar
// ("sidebar.tab" inspector), or, without the sidebar plugin, a side panel
// of its own (layout.panel "inspector"). Every section is a contribution
// to "inspector.section" — { id, title, order, shown(view), render(view) →
// node, nodes or text, fold, open, meta(view) } — its own (Run, Tokens,
// Cost, Tools, Session, Runner log) as much as a plugin's
// (cockpit.inspector.register). A section with the id of another takes its
// place. A node a section renders again stays where it is, so a select the
// section keeps does not close the menu it has open.
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

  // put gives parent those children, leaving where they are the ones it
  // holds already: a node put in again would lose the transition it runs,
  // and a select the menu it has open.
  const same = (parent, nodes) => parent.childNodes.length === nodes.length && nodes.every((node, i) => parent.childNodes[i] === node);
  const put = (parent, nodes) => {
    if (same(parent, nodes)) return;
    const keep = new Set(nodes);
    for (const child of [...parent.childNodes]) if (!keep.has(child)) child.remove();
    let at = parent.firstChild;
    for (const node of nodes) {
      if (node === at) at = node.nextSibling;
      else parent.insertBefore(node, at);
    }
  };
  // nodesOf is what a section rendered, as the nodes to put: a node, the
  // children of a fragment, a list of them, or text.
  const nodesOf = (body) => {
    if (Array.isArray(body)) {
      return body.flat(Infinity).filter((part) => part != null && part !== false)
        .flatMap((part) => (part instanceof Node ? nodesOf(part) : [document.createTextNode(String(part))]));
    }
    if (body instanceof DocumentFragment) return [...body.childNodes];
    if (body instanceof Node) return [body];
    return [h('p', { class: 'none', text: body == null ? '' : String(body) })];
  };
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

  // Each section keeps its element and its heading, so its body is replaced
  // in place.
  const boxes = new WeakMap(); // section → its element, or its bar
  const heads = new WeakMap(); // section → its heading
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
    if (section.raw && (body instanceof Node || Array.isArray(body))) put(box, nodesOf(body));
    else put(box, [headOf(section, title), ...nodesOf(body)]);
    return box;
  }

  function headOf(section, title) {
    if (title instanceof Node) return title;
    let head = heads.get(section);
    if (!head) heads.set(section, (head = h('h2')));
    setText(head, title);
    return head;
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
    put(bar.body, nodesOf(body));
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

  // The sections' rows, buttons and boxes keep their elements from one
  // render to the next, as a run renders them every second: put leaves a
  // node it is given again where it is, so text selected in it, the focus on
  // it and a click begun on it all stay. Only what changed is written. A
  // section that keeps nodes returns a list of them, not a fragment, which
  // would take them out of the page.
  const keptRows = new Map(); // key → its kv row
  function row(key, label, value, cls) {
    let el = keptRows.get(key);
    if (!el) keptRows.set(key, (el = kv(label, '', cls)));
    const name = `kv${cls ? ` ${cls}` : ''}`;
    if (el.className !== name) el.className = name;
    const shown = el.querySelector('.v');
    setText(shown, value);
    if (shown.title !== shown.textContent) shown.title = shown.textContent;
    return el;
  }
  const disable = (el, off) => {
    if (el.disabled !== !!off) el.disabled = !!off;
  };
  const tallyOf = (label) => {
    const n = h('b');
    return { n, node: h('div', null, n, h('span', { text: label })) };
  };
  const setTally = (tally, n) => {
    setText(tally.n, String(n));
    if (n === 0) tally.node.dataset.zero = 'true';
    else delete tally.node.dataset.zero;
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

  // The tool profile and the sandbox of the next runs: chosen here, kept
  // by the server where the terminal cockpit reads them too.
  const runPrefs = { profile: null, sandbox: null, profiles: [], sandboxes: [], ticket: 0 };
  async function refreshRunPrefs() {
    const ticket = runPrefs.ticket;
    let saved;
    try {
      saved = await cockpit.api('/api/preferences');
    } catch {
      return;
    }
    if (ticket !== runPrefs.ticket) return;
    runPrefs.profile = saved?.tool_profile || 'auto';
    runPrefs.sandbox = saved?.sandbox || 'off';
    runPrefs.profiles = saved?.tool_profiles || [];
    runPrefs.sandboxes = saved?.sandboxes || [];
    cockpit.render();
  }
  async function saveRunPref(key, value) {
    const ticket = ++runPrefs.ticket;
    if (key === 'tool_profile') runPrefs.profile = value;
    else runPrefs.sandbox = value;
    cockpit.render();
    try {
      await cockpit.api('/api/preferences', { method: 'PUT', body: { [key]: value } });
      cockpit.toast(key === 'tool_profile' ? `Tools for the next runs: ${value}` : `Sandbox for the next runs: ${value}`, 'info', 'run-prefs');
    } catch (error) {
      if (ticket === runPrefs.ticket) cockpit.toast(`Not saved: ${error.message}`, 'error', 'run-prefs');
    }
  }
  // choice is a key · · · <select> row. It keeps its elements from one
  // render to the next, as a run renders the section every second and more:
  // a select made anew would close the menu the user has open. While it has
  // the focus, the value the user is choosing stays.
  const choices = new Map(); // key → { row, select, list }
  function choice(label, key, current, options, title) {
    let made = choices.get(key);
    if (!made) {
      const select = h('select', { class: 'ins-select', 'aria-label': label, title, onchange: () => saveRunPref(key, select.value) });
      made = { select, row: h('div', { class: 'kv kv-choice' }, h('span', { class: 'k', text: label }), h('span', { class: 'dots' }), select), list: '' };
      choices.set(key, made);
    }
    const { select, row } = made;
    const list = JSON.stringify(options.map((option) => [option.value, option.description || '']));
    const fresh = list !== made.list;
    if (fresh) {
      made.list = list;
      select.replaceChildren(...options.map((option) => h('option', { value: option.value, text: option.value, title: option.description || '' })));
    }
    const value = current || options[0]?.value || '';
    if (fresh || (select.value !== value && document.activeElement !== select)) select.value = value;
    const description = options.find((option) => option.value === select.value)?.description || '';
    if (row.title !== description) row.title = description;
    return row;
  }
  const connectionActions = h('div', { class: 'ins-actions' },
    h('button', { class: 'act', type: 'button', onclick: () => service('connection')?.open?.() }, 'Connection…'));
  cockpit.interval(() => { if (document.visibilityState === 'visible' && open()) refreshRunPrefs(); }, 10000);
  refreshRunPrefs();

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
      // A list, not a fragment: the rows it keeps stay where they are.
      return kvs(
        row('run.state', 'State', session.PHASE_LABEL[phase] || phase, `state-${phase}`),
        row('run.elapsed', 'Elapsed', v.run ? fmt.timer(Date.now() - v.run.started) : '—'),
        row('run.activity', 'Activity', v.run?.activity || '—'),
        row('run.effort', 'Effort', service('effort')?.current?.() || state.config?.thinking || '—'),
        row('run.provider', 'Provider', link ? [link.source === 'gateway' ? 'gateway' : link.provider, API_LABEL[api]].filter(Boolean).join(' · ') : '—'),
        row('run.model', 'Model', next || 'runner default'),
        row('run.default', 'Default', session.defaultModel() || 'runner default'),
        row('run.endpoint', 'Endpoint', link?.source === 'gateway' ? `accounts gateway${state.config?.accounts?.url ? ` · ${host(state.config.accounts.url)}` : ''}`
          : link?.base_url ? (via ? `accounts gateway · ${host(link.base_url)}` : host(link.base_url)) : 'provider default'),
        runPrefs.profiles.length ? choice('Tools', 'tool_profile', runPrefs.profile, runPrefs.profiles,
          'The built-in tools the next runs work with: edit (Read/Edit/Write) for Claude, patch (apply_patch) for GPT, code (one Code tool: JavaScript that calls the tools), shell (Bash only); all but code have TranscriptSearch, which finds what compaction took out of the context, and code has transcriptSearch()') : null,
        runPrefs.sandboxes.length ? choice('Sandbox', 'sandbox', runPrefs.sandbox, runPrefs.sandboxes,
          'Where the next runs work: the workspace, a git worktree of its own, or a worktree with the commands in the workspace\'s container') : null,
        cockpit.has('connection') ? connectionActions : null);
    },
  });
  cockpit.commands.register({
    name: 'tools', args: '<profile>', help: 'The built-in tools of the next runs: auto, edit, patch, code or shell', order: 135,
    run: (arg) => {
      const value = (arg || '').trim().toLowerCase();
      const known = runPrefs.profiles.map((option) => option.value);
      if (!value) return cockpit.toast(`Tools: ${runPrefs.profile || 'auto'}. Profiles: ${known.join(', ')}.`);
      if (!known.includes(value)) return cockpit.toast(`Tool profiles: ${known.join(', ')}.`, 'error');
      return saveRunPref('tool_profile', value);
    },
    complete: () => runPrefs.profiles.map((option) => ({ value: option.value, label: option.value, detail: option.description, current: option.value === runPrefs.profile })),
  });
  cockpit.commands.register({
    name: 'sandbox', args: '<mode>', help: 'Where the next runs work: off, worktree or container', order: 136,
    run: (arg) => {
      const value = (arg || '').trim().toLowerCase();
      const known = runPrefs.sandboxes.map((option) => option.value);
      if (!value) return cockpit.toast(`Sandbox: ${runPrefs.sandbox || 'off'}. Modes: ${known.join(', ')}.`);
      if (!known.includes(value)) return cockpit.toast(`Sandbox modes: ${known.join(', ')}.`, 'error');
      return saveRunPref('sandbox', value);
    },
    complete: () => runPrefs.sandboxes.map((option) => ({ value: option.value, label: option.value, detail: option.description, current: option.value === runPrefs.sandbox })),
  });

  const meterFill = h('i');
  const meter = h('div', { class: 'meter-bar' }, meterFill);
  const compactButton = h('button', {
    class: 'act', type: 'button',
    title: 'Summarize the conversation to free context; the next prompt starts from the summary (/compact in the composer)',
    onclick: () => session.compact(),
  }, 'Compact');
  const tokenActions = h('div', { class: 'ins-actions' }, compactButton);
  // Tokens per second: output tokens over the time their turns took, from
  // the request to the response, as the server measures them (Usage).
  const perSecond = (tokens, ms) => (tokens > 0 && ms > 0 ? (tokens * 1000) / ms : NaN);
  const rate = (n) => (n >= 100 ? String(Math.round(n)) : n.toFixed(1));
  cockpit.contribute('inspector.section', {
    id: 'tokens', title: 'Tokens', order: 20,
    render: (summary, v) => {
      const usage = v.usage || {};
      const cachedShare = usage.input ? Math.round((usage.cached / usage.input) * 100) : 0;
      const tip = `${cachedShare}% of input tokens were cached`;
      if (meter.title !== tip) meter.title = tip;
      // Inline styles are refused by the page's policy; properties are not.
      if (meterFill.style.width !== `${cachedShare}%`) meterFill.style.width = `${cachedShare}%`;
      disable(compactButton, v.fresh || !!session.runBlocked(v));
      // The latest turn's, then the session's: one turn timed has but one.
      const last = perSecond(usage.last_output, usage.last_ms);
      const average = perSecond(usage.timed_output, usage.timed_ms);
      const speed = row('tokens.tps', 'TPS', !Number.isFinite(last) ? '—'
        : usage.timed_ms === usage.last_ms ? rate(last) : `${rate(last)} · avg ${rate(average)}`);
      const speedTip = Number.isFinite(last)
        ? `Output tokens per second: the latest turn wrote ${fmt.tokens(usage.last_output)} in ${fmt.duration(usage.last_ms)}; the average is over the session's turns. Timed from each request to its response, so the wait for the first token counts too`
        : 'Output tokens per second, once a turn reports its output tokens';
      if (speed.title !== speedTip) speed.title = speedTip;
      return kvs(
        row('tokens.context', 'Context', fmt.tokens(usage.context ?? NaN)),
        row('tokens.input', 'Input', fmt.tokens(usage.input ?? NaN)),
        row('tokens.cached', 'Cached', usage.input ? `${fmt.tokens(usage.cached)} · ${cachedShare}%` : '—'),
        meter,
        row('tokens.output', 'Output', fmt.tokens(usage.output ?? NaN)),
        row('tokens.reasoning', 'Reasoning', fmt.tokens(usage.reasoning ?? NaN)),
        speed,
        row('tokens.turns', 'Turns', usage.turns ? String(usage.turns) : '—'),
        tokenActions);
    },
  });

  // What the session's tokens come to in money, at API prices: each
  // model's at its own — a session may change models from one prompt to
  // the next — as the accounts gateway prices its requests' (GET
  // /api/usage/price), in the user's unit, dollars or credits of their own.
  // Prices read stay, model by model, for the sessions viewed again; the
  // models of the session in view are read again a minute on, or as soon
  // as the Accounts view changes a price or the unit.
  const pricing = { key: null, at: 0, ticket: 0, unit: null, prices: new Map(), error: '' };
  const PRICES_KEPT = 60000; // how long prices read are taken as they are
  function priceModels(ids) {
    const key = ids.join('\n');
    if (key === pricing.key && Date.now() - pricing.at < PRICES_KEPT) return;
    pricing.key = key;
    pricing.at = Date.now();
    const ticket = ++pricing.ticket;
    cockpit.api(`/api/usage/price?${new URLSearchParams(ids.slice(0, 64).map((id) => ['model', id]))}`).then((answer) => {
      if (ticket !== pricing.ticket) return;
      pricing.unit = answer?.unit || null;
      for (const m of answer?.models || []) pricing.prices.set(m.id, m.price);
      pricing.error = '';
      cockpit.render();
    }, (error) => {
      if (ticket !== pricing.ticket) return;
      pricing.error = error.message;
      cockpit.render();
    });
  }
  // Prices or the unit changed in the Accounts view.
  cockpit.on('accounts:prices', () => {
    pricing.at = 0;
    cockpit.render();
  });

  // money reads an amount of dollars in the user's unit, as the Accounts
  // view does: "$1,234.56", "$0.042", "<$0.001", "12,346 credits".
  const numberFormats = new Map();
  const number = (value, digits, most = digits) => {
    const key = `${digits}:${most}`;
    if (!numberFormats.has(key)) numberFormats.set(key, new Intl.NumberFormat('en-US', { minimumFractionDigits: digits, maximumFractionDigits: most }));
    return numberFormats.get(key).format(value);
  };
  function money(usd, unit) {
    if (!Number.isFinite(usd)) return '—';
    const dollars = !unit || unit.name === 'USD' || !(unit.usd > 0);
    const v = dollars ? usd : usd / unit.usd;
    const abs = Math.abs(v);
    const sign = v < 0 ? '−' : '';
    if (dollars) {
      if (abs > 0 && abs < 0.001) return `${sign}<$0.001`;
      return `${sign}$${abs === 0 ? '0' : number(abs, abs < 0.1 ? 3 : abs < 10000 ? 2 : 0)}`;
    }
    const text = abs === 0 ? '0' : abs < 1 ? String(+abs.toPrecision(2)) : number(abs, 0, abs < 100 ? 1 : 0);
    return `${sign}${text} ${unit.name}`;
  }

  // costOf is what a model's tokens come to at its price, in dollars, kind
  // by kind: the input's cache reads and writes at their own rates, the
  // rest at the input's; the output with the reasoning it holds.
  function costOf(m, price) {
    const read = m.cached || 0;
    const written = m.cache_write || 0;
    const fresh = Math.max(0, (m.input || 0) - read - written);
    return {
      input: (fresh * (price.input || 0)) / 1e6,
      cache_read: (read * (price.cache_read || 0)) / 1e6,
      cache_write: (written * (price.cache_write || 0)) / 1e6,
      output: ((m.output || 0) * (price.output || 0)) / 1e6,
      // What caching saved: the cache reads at the input's price, less
      // what they and the writes' premium cost.
      saved: (read * ((price.input || 0) - (price.cache_read || 0)) - written * ((price.cache_write || 0) - (price.input || 0))) / 1e6,
    };
  }
  const PRICE_SOURCE = { custom: 'your price', list: 'list price', openrouter: 'OpenRouter\'s price' };
  // tokensOf says what a model's tokens were, kind by kind.
  function tokensOf(m) {
    const caches = [m.cached && `${fmt.tokens(m.cached)} read from the cache`, m.cache_write && `${fmt.tokens(m.cache_write)} written to it`].filter(Boolean);
    return `${fmt.tokens(m.input || 0)} input${caches.length ? ` (${caches.join(', ')})` : ''} · ${fmt.tokens(m.output || 0)} output${m.reasoning ? ` (${fmt.tokens(m.reasoning)} reasoning)` : ''}`;
  }

  // The Cost section's rows keep their elements too; a row's tip, the
  // whole of it, says what its amount is of.
  const costRows = new Map(); // key → its kv row
  function costRow(key, label, value, tip, cls) {
    let el = costRows.get(key);
    if (!el) {
      costRows.set(key, (el = kv(label, '', cls)));
      el.querySelector('.v').removeAttribute('title');
    }
    const name = `kv ${cls}`;
    if (el.className !== name) el.className = name;
    setText(el.querySelector('.v'), value);
    if (el.title !== tip) el.title = tip;
    return el;
  }
  const costNote = h('p', { class: 'none' });
  const pricesButton = h('button', {
    class: 'act', type: 'button',
    title: 'Price the models the cockpit knows no price of, in Accounts → Usage',
    onclick: () => service('accounts')?.showUsage?.(),
  }, 'Set prices…');
  const costActions = h('div', { class: 'ins-actions' }, pricesButton);
  const COST_TIP = 'What the session\'s tokens come to at API prices — the providers\' list prices, or yours — each model\'s at its own';
  cockpit.contribute('inspector.section', {
    id: 'cost', title: 'Cost', order: 25,
    shown: (summary) => summary?.usage?.turns > 0,
    render: (summary, v) => {
      const usage = v.usage || {};
      // The tokens of each model that answered, in the order they first
      // did; a server that does not count them apart has them all of no
      // model known.
      const models = usage.models || [{ ...usage, model: '' }];
      const off = session.state.config?.accounts?.state === 'off';
      const ids = [...new Set(models.map((m) => m.model).filter(Boolean))].sort();
      if (!off && ids.length) priceModels(ids);
      const unit = pricing.unit;
      const sum = { input: 0, cache_read: 0, cache_write: 0, output: 0, saved: 0 };
      const unpriced = []; // the models of no price
      let priced = 0;
      let waiting = false; // for prices not read yet
      let unknown = false; // turns that recorded no model
      let estimated = false;
      const rows = models.map((m) => {
        const label = m.model || 'model not recorded';
        const about = `${label} · ${m.turns} turn${m.turns === 1 ? '' : 's'}\n${tokensOf(m)}`;
        const key = `cost.model.${m.model}`;
        if (!m.model) {
          unknown = true;
          return costRow(key, label, '—', `${about}\nThese turns recorded no model to price their tokens at`, 'cost-model');
        }
        const price = pricing.prices.get(m.model);
        if (!price) {
          waiting = true;
          return costRow(key, label, off || pricing.error ? '—' : '…', about, 'cost-model');
        }
        if (price.source === 'none') {
          unpriced.push(m.model);
          return costRow(key, label, 'no price', `${about}\nThe cockpit knows no price of this model: set one to count its tokens`, 'cost-model cost-unpriced');
        }
        priced++;
        const cost = costOf(m, price);
        for (const kind of Object.keys(sum)) sum[kind] += cost[kind];
        const guess = price.source === 'estimate';
        estimated ||= guess;
        const rates = `${money(price.input, unit)} input · ${money(price.cache_read, unit)} cache read · ${money(price.cache_write, unit)} cache write · ${money(price.output, unit)} output, per million tokens`;
        const source = guess ? `estimated from ${price.like || 'a model of its family'}` : PRICE_SOURCE[price.source] || price.source;
        // ≈ marks an estimate, as the Accounts view does.
        return costRow(key, label, `${guess ? '≈ ' : ''}${money(cost.input + cost.cache_read + cost.cache_write + cost.output, unit)}`,
          `${about}\n${rates}: ${source}${price.rule ? ` (${price.rule})` : ''}`, 'cost-model');
      });
      // A total that leaves tokens out is at least what it says.
      const partial = waiting || unknown || unpriced.length > 0;
      const value = !priced ? (off || pricing.error || !waiting ? '—' : '…')
        : `${partial ? '≥ ' : estimated ? '≈ ' : ''}${money(sum.input + sum.cache_read + sum.cache_write + sum.output, unit)}`;
      const tip = !priced ? COST_TIP
        : `${COST_TIP}: input ${money(sum.input, unit)} · cache reads ${money(sum.cache_read, unit)} · cache writes ${money(sum.cache_write, unit)} · output ${money(sum.output, unit)}`
          + `${sum.saved > 0 ? `\nCaching saved ${money(sum.saved, unit)}` : ''}${partial ? '\nWithout the tokens of the models it has no price of' : ''}`;
      let note = '';
      if (off) note = 'Prices come from the accounts gateway, which is off';
      else if (pricing.error && waiting) note = `No prices: ${pricing.error}`;
      else if (unpriced.length) note = `No price of ${unpriced.join(', ')}: ${unpriced.length > 1 ? 'their' : 'its'} tokens are not counted`;
      setText(costNote, note);
      return kvs(
        costRow('cost.total', 'Total', value, tip, 'cost-total'),
        ...rows,
        note ? costNote : null,
        unpriced.length && !off && cockpit.has('accounts') ? costActions : null);
    },
  });

  const GLYPH = { done: '✓', failed: '✕', running: '■', queued: '□', canceled: '⊘', interrupted: '◌' };
  const toolCount = h('span', { id: 'ins-tools-count' });
  const toolsHead = h('h2', null, 'Tools ', toolCount);
  const tallies = { done: tallyOf('Done'), failed: tallyOf('Failed'), live: tallyOf('Live'), other: tallyOf('Other') };
  const tallyBox = h('div', { class: 'tally' }, Object.values(tallies).map((tally) => tally.node));
  const recentList = h('ol', { class: 'recent' });
  const recentRows = new Map(); // entry ID → { li, button, glyph, name, input }
  const noTools = h('p', { class: 'none', text: 'No tool calls yet' });
  cockpit.contribute('inspector.section', {
    id: 'tools', order: 30,
    title: (summary, v) => {
      setText(toolCount, String([...v.entries.values()].filter((e) => e.kind === 'tool').length));
      return toolsHead;
    },
    render: (summary, v) => {
      const tools = [...v.entries.values()].filter((e) => e.kind === 'tool');
      const stateOf = (e) => service('timeline')?.toolState?.(e, !!v.run) || e.tool?.state || 'queued';
      const count = (s) => tools.filter((e) => stateOf(e) === s).length;
      const recent = tools.slice(-10).reverse();
      setTally(tallies.done, count('done'));
      setTally(tallies.failed, count('failed'));
      setTally(tallies.live, count('running') + count('queued'));
      setTally(tallies.other, count('canceled') + count('interrupted'));
      const items = recent.map((e) => {
        let made = recentRows.get(e.id);
        if (!made) {
          const glyph = h('span', { class: 'g' });
          const name = h('span', { class: 'n' });
          const input = h('span', { class: 'i' });
          const button = h('button', { type: 'button', onclick: () => service('timeline')?.reveal?.(e.id) }, glyph, name, input);
          made = { li: h('li', null, button), button, glyph, name, input };
          recentRows.set(e.id, made);
        }
        const s = stateOf(e);
        const text = e.tool?.input || '';
        if (made.button.dataset.state !== s) made.button.dataset.state = s;
        if (made.button.title !== text) made.button.title = text;
        setText(made.glyph, GLYPH[s]);
        setText(made.name, e.tool?.name || 'Tool');
        setText(made.input, text.split('\n')[0]);
        return made.li;
      });
      const ids = new Set(recent.map((e) => e.id));
      for (const id of [...recentRows.keys()]) if (!ids.has(id)) recentRows.delete(id);
      put(recentList, items);
      return kvs(tallyBox, recent.length ? recentList : noTools);
    },
  });

  // The Session section's buttons act on the session in view when clicked.
  const actionsButton = h('button', {
    class: 'act', type: 'button', 'aria-haspopup': 'menu', 'aria-expanded': 'false',
    onclick: (event) => {
      const v = view();
      if (v && !v.fresh) service('menu')?.toggle?.(event.currentTarget, () => session.menuItems(v.ws, v.id));
    },
  }, 'Actions…');
  const copyIdButton = h('button', {
    class: 'act', type: 'button',
    onclick: () => {
      const v = view();
      if (v && !v.fresh) cockpit.copy(v.id, 'Session ID copied');
    },
  }, 'Copy ID');
  const copyTuiButton = h('button', {
    class: 'act', type: 'button',
    onclick: () => {
      const v = view();
      if (v && !v.fresh) cockpit.copy(`kou-conveyor-tui -workspace ${shellQuote(session.currentWorkspace()?.path || session.state.config?.workspace || '.')} -session ${v.id}`, 'Resume command copied');
    },
  }, 'Copy TUI command');
  const sessionActions = h('div', { class: 'ins-actions' }, actionsButton, copyIdButton, copyTuiButton);
  cockpit.contribute('inspector.section', {
    id: 'session', title: 'Session', order: 40,
    render: (summary, v) => {
      for (const button of [actionsButton, copyIdButton, copyTuiButton]) disable(button, v.fresh);
      return kvs(
        row('session.id', 'ID', v.fresh ? 'unsaved' : v.id.slice(0, 13)),
        // A view that read only the transcript's end (a chat on a canvas)
        // counts those before it, and starts with its first prompt.
        row('session.entries', 'Entries', String((v.earlier || 0) + v.order.length)),
        row('session.started', 'Started', v.earlier > 0 ? (v.earlierPrompts?.[0]?.at ? fmt.stamp(v.earlierPrompts[0].at) : '—')
          : v.order.length ? fmt.stamp(v.entries.get(v.order[0]).at) : '—'),
        sessionActions);
    },
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
