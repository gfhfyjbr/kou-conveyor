// sidebar: the side panel (layout.panel "sidebar") of tabs. A tab is one of
// a kind, and the kinds are contributions to "sidebar.tab" — the built-in
// Inspector, Terminal and Files as much as a plugin's:
//
//   { id, title, icon, description, order, multiple, hello, key,
//     create(tab) → view }
//
// icon is SVG markup of the plugin's own; multiple false keeps one tab of
// the kind; hello false leaves it out of Hello; key is the key that opens
// it, for Hello to show. create(tab) makes the view of a tab when it first
// shows — tab is { id, kind, state, save(state), setTitle(text, detail),
// setBadge(text), close(), activate(), visible(), sidebar } — and returns
//
//   { node, title, focus(), shown(), hidden(), resized(), close(), dispose() }
//
// node is what the tab shows. shown and hidden say when it is in view,
// resized when its size changed; close runs when the user closes the tab
// (returning false keeps it) and dispose whenever the view goes: the tab
// closed, its plugin loaded anew or the workspace left. What a tab saves
// comes back to it after a reload, per workspace.
//
// A title is short, what tells the tab from the others; setTitle's detail
// is the whole of it (a shell's path and what runs there), which the tab's
// tooltip says. The tabs say their titles while all of them fit; else each
// is its icon, the shown one's title beside them, and those that do not
// fit even so are in the menu of "⋯ N".
//
// With no tab open the sidebar says Hello, which lists the kinds to open
// and the sections plugins add to "sidebar.hello" ({ id, order, render(tab)
// → node }); "+" opens Hello as a tab, which becomes the kind picked.
// Settings, a kind of its own, shows the sections plugins add to
// "settings.section" ({ id, title, order, render() → node }).
//
// It provides the sidebar service: open(kind, { state, focus, reuse }),
// close(id), activate(id), toggle(kind), show(), hide(), isOpen(),
// visible(id), tabs(), active(), kinds(), next(step).
const PANEL = '<svg viewBox="0 0 16 16" aria-hidden="true"><rect x="1.5" y="2.5" width="13" height="11" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M10 2.5v11" stroke="currentColor" stroke-width="1.4"/></svg>';
const PLUS = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 3v10M3 8h10" stroke="currentColor" stroke-width="1.4"/></svg>';
const HELLO = '<svg viewBox="0 0 16 16" aria-hidden="true"><rect x="2.5" y="2.5" width="11" height="11" fill="none" stroke="currentColor" stroke-width="1.4"/><rect x="6.5" y="6.5" width="3" height="3" fill="currentColor"/></svg>';
const SLIDERS = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M1.5 4.5h6.5M12 4.5h2.5M1.5 11.5h2.5M8 11.5h6.5" stroke="currentColor" stroke-width="1.4"/><rect x="8.5" y="2.5" width="3" height="4" fill="none" stroke="currentColor" stroke-width="1.4"/><rect x="4.5" y="9.5" width="3" height="4" fill="none" stroke="currentColor" stroke-width="1.4"/></svg>';

export default function activate(cockpit) {
  const { h, svg, prefs } = cockpit;
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const layout = () => service('layout');
  const hot = cockpit.hot.data;
  // The tabs of each workspace: { tabs: [{ id, kind, title, state }], active }.
  hot.sets ??= prefs.get('sidebar', null)?.workspaces || {};
  hot.ws ??= null;
  const live = new Map(); // tab ID → { kind, view, pane, created }

  // ---------------------------------------------------------------- the panel

  const strip = h('div', { class: 'sb-tabs', id: 'sidebar-tabs', role: 'tablist', 'aria-label': 'Sidebar tabs' });
  // Where the tabs do not all fit with their names (fitStrip): the menu of
  // all of them, and the name of the one shown.
  const more = h('button', {
    class: 'sb-more', id: 'sidebar-more', type: 'button', hidden: true, 'aria-haspopup': 'menu', 'aria-expanded': 'false',
    onclick: () => service('menu')?.toggle(more, () => tabItems()),
  });
  const captionTitle = h('span', { class: 'sb-caption-title' });
  const captionBadge = h('span', { class: 'sb-badge' });
  const caption = h('div', { class: 'sb-caption', id: 'sidebar-caption', hidden: true }, captionTitle, captionBadge,
    h('button', { class: 'sb-x', type: 'button', tabindex: '-1', 'aria-label': 'Close tab', title: 'Close tab', onclick: () => { const t = activeTab(); if (t) close(t.id); } }, '×'));
  const bar = h('div', { class: 'sb-bar' }, strip, more, caption);
  const add = h('button', { class: 'icon sb-add', id: 'sidebar-new', type: 'button', title: 'New tab', 'aria-label': 'New tab', onclick: () => open('hello') }, svg(PLUS));
  const body = h('div', { class: 'sb-body', id: 'sidebar-body' });
  const node = h('aside', { class: 'sidebar', id: 'sidebar', 'aria-label': 'Sidebar' },
    h('header', { class: 'sb-head' }, bar, add,
      h('button', { class: 'icon sb-close', id: 'sidebar-close', type: 'button', 'aria-label': 'Close sidebar', title: 'Close sidebar (S)', onclick: () => hide() }, '×')),
    body);
  cockpit.contribute('layout.panel', {
    id: 'sidebar', order: 20, width: 'var(--sidebar)', minWidth: 300, node,
    opened: () => { ensure(); sync(); },
    closed: () => sync(),
  });

  const button = h('button', { class: 'icon', id: 'sidebar-toggle', type: 'button', title: 'Sidebar (S)', 'aria-label': 'Toggle sidebar', 'aria-pressed': 'false', onclick: () => toggle() }, svg(PANEL));
  cockpit.ui.mount('bar.end', { id: 'sidebar-toggle', order: 70, node: button });

  const isOpen = () => !!layout()?.panelOpen?.('sidebar');
  const shown = () => isOpen() && !layout()?.shell?.hasAttribute?.('data-page');

  // ---------------------------------------------------------------- kinds

  const kinds = () => cockpit.contributions('sidebar.tab', { unique: 'id' });
  const kindOf = (id) => kinds().find((k) => k.id === id) || null;
  // icon draws a kind's icon; a kind with none (or gone) is the first
  // letter of its name.
  const icon = (kind, name = kind?.title || kind?.id || '') => {
    const markup = kind?.icon;
    if (markup instanceof Node) return markup.cloneNode(true);
    if (typeof markup === 'string' && markup.trim().startsWith('<svg')) return svg(markup);
    if (typeof markup === 'string' && markup.trim()) return h('span', { class: 'sb-glyph', text: markup });
    const letter = [...String(name).trim()][0]?.toUpperCase();
    return letter ? h('span', { class: 'sb-mono', text: letter }) : h('span', { class: 'sb-glyph', text: '◇' });
  };

  // ---------------------------------------------------------------- the tabs of a workspace

  const workspace = () => service('session')?.state?.ws || cockpit.host.workspace() || 'default';
  function set() {
    const ws = workspace();
    if (!hot.sets[ws]) {
      // The sidebar was the inspector: it starts with that.
      hot.sets[ws] = { tabs: [{ id: cockpit.uuid(), kind: 'inspector', title: 'Inspector', state: null }], active: null };
      hot.sets[ws].active = hot.sets[ws].tabs[0].id;
    }
    return hot.sets[ws];
  }
  const tabs = () => set().tabs;
  const tabNamed = (id) => tabs().find((t) => t.id === id) || null;
  const activeTab = () => tabNamed(set().active) || tabs()[0] || null;

  // What is kept of a tab: its title's detail only where it has one.
  const stored = (t) => ({ id: t.id, kind: t.kind, title: t.title || '', ...(t.detail ? { detail: t.detail } : {}), state: t.state ?? null });
  let saving = 0;
  function save() {
    clearTimeout(saving);
    saving = setTimeout(() => {
      // What each tab saves, and the tabs, per workspace.
      const workspaces = {};
      for (const [ws, s] of Object.entries(hot.sets)) {
        workspaces[ws] = { active: s.active, tabs: s.tabs.map(stored) };
      }
      prefs.set('sidebar', { version: 1, workspaces });
    }, 150);
  }
  cockpit.onDispose(() => {
    clearTimeout(saving);
    const workspaces = {};
    for (const [ws, s] of Object.entries(hot.sets)) workspaces[ws] = { active: s.active, tabs: s.tabs.map(stored) };
    prefs.set('sidebar', { version: 1, workspaces });
  });

  // ensure keeps a tab open: with none, the sidebar says Hello.
  function ensure() {
    const s = set();
    if (!s.tabs.length) {
      const hello = { id: cockpit.uuid(), kind: 'hello', title: 'Hello', state: null };
      s.tabs.push(hello);
      s.active = hello.id;
      save();
    }
    if (!tabNamed(s.active)) s.active = s.tabs[0].id;
  }

  // ---------------------------------------------------------------- views

  // handle is what a view knows of its tab.
  function handle(record) {
    return Object.freeze({
      id: record.id,
      kind: record.kind,
      get state() { return record.state; },
      save(state) {
        record.state = state ?? null;
        save();
      },
      setTitle(text, detail) {
        const title = String(text || '').trim();
        const whole = String(detail || '').trim();
        if (record.title === title && (record.detail || '') === whole) return;
        record.title = title;
        if (whole) record.detail = whole;
        else delete record.detail;
        drawStrip();
        save();
      },
      setBadge(text) {
        const badge = text == null || text === '' ? '' : String(text);
        if (record.badge === badge) return;
        record.badge = badge;
        drawStrip();
      },
      close: () => close(record.id),
      activate: () => activate(record.id),
      visible: () => shown() && set().active === record.id,
      sidebar: api,
    });
  }

  // viewOf makes a tab's view, the first time the tab shows, or anew when
  // its kind's plugin was loaded anew.
  function viewOf(record) {
    const kind = kindOf(record.kind);
    let entry = live.get(record.id);
    if (entry && entry.kind === kind) return entry;
    if (entry) drop(record.id);
    const pane = h('section', { class: 'sb-pane', role: 'tabpanel', data: { tab: record.id, kind: record.kind }, hidden: true });
    entry = { kind, view: null, pane, visible: false };
    live.set(record.id, entry);
    if (!kind) {
      pane.append(h('div', { class: 'sb-missing' },
        h('p', null, h('b', { text: record.title || record.kind }), ` needs the plugin that gives tabs of the kind "${record.kind}", which is not running.`),
        h('div', { class: 'ins-actions' },
          cockpit.has('plugins') ? h('button', { class: 'act', type: 'button', onclick: () => service('plugins')?.show?.() }, 'Plugins…') : null,
          h('button', { class: 'act', type: 'button', onclick: () => close(record.id, { force: true }) }, 'Close tab'))));
    } else {
      const view = cockpit.safely(() => kind.create(handle(record)));
      entry.view = view && typeof view === 'object' ? view : {};
      const content = entry.view.node;
      if (content instanceof Node) pane.append(content);
      else pane.append(h('p', { class: 'none', text: 'This tab has nothing to show.' }));
      if (entry.view.title && !record.title) record.title = String(entry.view.title);
    }
    body.append(pane);
    return entry;
  }

  // drop lets a tab's view go; the tab stays.
  function drop(id) {
    const entry = live.get(id);
    if (!entry) return;
    live.delete(id);
    if (entry.visible) cockpit.safely(() => entry.view?.hidden?.());
    cockpit.safely(() => entry.view?.dispose?.());
    entry.pane.remove();
  }
  cockpit.onDispose(() => { for (const id of [...live.keys()]) drop(id); });

  // sync shows the active tab's view, and tells views whether they show.
  function sync() {
    const s = set();
    const visible = shown();
    const active = activeTab();
    if (active && visible) viewOf(active);
    for (const [id, entry] of live) {
      const on = visible && id === active?.id;
      entry.pane.hidden = id !== active?.id;
      if (on !== entry.visible) {
        entry.visible = on;
        cockpit.safely(() => (on ? entry.view?.shown?.() : entry.view?.hidden?.()));
      }
    }
    // Views of tabs no longer there go.
    for (const id of [...live.keys()]) if (!s.tabs.some((t) => t.id === id)) drop(id);
    drawStrip();
    button.setAttribute('aria-pressed', String(isOpen()));
    cockpit.emit('sidebar:tab', active?.id || null, active?.kind || null);
  }

  // ---------------------------------------------------------------- the strip

  const tabNodes = new Map();
  let dragging = null;
  function drawStrip() {
    const s = set();
    const nodes = s.tabs.map((record) => {
      let el = tabNodes.get(record.id);
      if (!el) {
        el = h('div', {
          class: 'sb-tab', role: 'tab', tabindex: '0', draggable: 'true', data: { tab: record.id },
          onclick: (event) => { if (!event.target.closest('.sb-x')) activate(record.id); },
          onauxclick: (event) => { if (event.button === 1) { event.preventDefault(); close(record.id); } },
          onkeydown: (event) => {
            if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); activate(record.id); }
            if (event.key === 'Delete' || event.key === 'Backspace') {
              event.preventDefault();
              // The focus goes on to the tab shown now, so the arrows go on working.
              close(record.id).then((ok) => { if (ok) tabNodes.get(set().active)?.focus(); });
            }
            if (event.key === 'ArrowRight' || event.key === 'ArrowLeft') { event.preventDefault(); next(event.key === 'ArrowRight' ? 1 : -1, true); }
          },
          ondragstart: (event) => {
            dragging = record.id;
            event.dataTransfer.effectAllowed = 'move';
            event.dataTransfer.setData('text/plain', record.kind);
            el.dataset.dragging = 'true';
          },
          ondragend: () => { dragging = null; delete el.dataset.dragging; },
          ondragover: (event) => {
            if (!dragging || dragging === record.id) return;
            event.preventDefault();
            const box = el.getBoundingClientRect();
            move(dragging, record.id, event.clientX > box.left + box.width / 2);
          },
          ondrop: (event) => event.preventDefault(),
        },
        h('span', { class: 'sb-icon' }),
        h('span', { class: 'sb-title' }),
        h('span', { class: 'sb-badge' }),
        h('button', { class: 'sb-x', type: 'button', tabindex: '-1', 'aria-label': 'Close tab', title: 'Close tab', onclick: () => close(record.id) }, '×'));
        tabNodes.set(record.id, el);
      }
      const kind = kindOf(record.kind);
      const title = record.title || kind?.title || record.kind;
      const iconBox = el.querySelector('.sb-icon');
      if (iconBox.dataset.kind !== `${record.kind}:${kind ? 1 : 0}`) {
        iconBox.dataset.kind = `${record.kind}:${kind ? 1 : 0}`;
        iconBox.replaceChildren(icon(kind, kind?.title || record.kind));
      }
      const label = el.querySelector('.sb-title');
      if (label.textContent !== title) label.textContent = title;
      const badge = el.querySelector('.sb-badge');
      if (badge.textContent !== (record.badge || '')) badge.textContent = record.badge || '';
      // The tooltip says what the title leaves out (a terminal's whole path);
      // the name is the tab's own, for when it shows only its icon.
      const tip = record.detail || title;
      if (el.title !== tip) el.title = tip;
      el.setAttribute('aria-label', record.badge ? `${title} (${record.badge})` : title);
      el.dataset.kind = record.kind;
      el.setAttribute('aria-selected', String(record.id === s.active));
      return el;
    });
    for (const [id, el] of tabNodes) if (!s.tabs.some((t) => t.id === id)) { el.remove(); tabNodes.delete(id); }
    let at = strip.firstChild;
    for (const el of nodes) {
      if (el === at) at = at.nextSibling;
      else strip.insertBefore(el, at);
    }
    // The caption says the name of the tab shown, where the tabs are icons.
    const shownTab = tabNamed(s.active);
    const name = shownTab ? shownTab.title || kindOf(shownTab.kind)?.title || shownTab.kind : '';
    if (captionTitle.textContent !== name) captionTitle.textContent = name;
    if (captionBadge.textContent !== (shownTab?.badge || '')) captionBadge.textContent = shownTab?.badge || '';
    caption.title = shownTab?.detail || name;
    fitStrip();
  }

  // fitStrip fits the tabs to the strip's room. They say their names while
  // all of them fit so; else each is its icon and the caption beside them
  // says the shown one's name, and the icons that do not fit either are in
  // the menu of "⋯ N" — the shown tab's never: it takes the last place.
  const CAPTION_MIN = 96; // the least room the caption is left, in px
  const NAMES_SLACK = 12; // the names come back with this much to spare, so the strip does not flap
  let fittedTo = ''; // the room and the tabs the strip was last fitted to
  let settling = false; // while the panel's width moves
  function fitStrip(force = false) {
    const room = bar.clientWidth;
    const els = [...strip.children];
    // The panel closed, or only starting to open: nothing to fit to yet.
    if (room < 120 || settling || !els.length) return;
    const s = set();
    const key = `${room}|${s.active}|${s.tabs.map((t) => `${t.id}:${t.title}:${t.badge || ''}`).join('|')}`;
    if (key === fittedTo && !force) return;
    fittedTo = key;
    const wasIcons = bar.dataset.mode === 'icons';
    for (const el of els) el.hidden = false;
    more.hidden = true;
    caption.hidden = true;
    strip.scrollLeft = 0;
    bar.dataset.mode = 'names';
    bar.dataset.measure = '';
    // Round corners set the tabs apart (sidebar.css): the gaps take room too.
    const gap = parseFloat(getComputedStyle(strip).columnGap) || 0;
    const need = els.reduce((sum, el) => sum + el.getBoundingClientRect().width, 0) + gap * (els.length - 1);
    delete bar.dataset.measure;
    if (need + (wasIcons ? NAMES_SLACK : 0) <= room) return;
    bar.dataset.mode = 'icons';
    caption.hidden = false;
    const width = els[0].getBoundingClientRect().width;
    if (els.length * (width + gap) - gap + CAPTION_MIN <= room) return;
    more.hidden = false;
    more.textContent = `⋯ ${els.length}`;
    const fits = Math.max(1, Math.floor((room - CAPTION_MIN - more.getBoundingClientRect().width + gap) / (width + gap)));
    if (fits >= els.length) {
      more.hidden = true;
      return;
    }
    const shownAt = Math.max(0, els.findIndex((el) => el.getAttribute('aria-selected') === 'true'));
    const visible = els.slice(0, fits);
    if (shownAt >= fits) visible[fits - 1] = els[shownAt];
    for (const el of els) el.hidden = !visible.includes(el);
    const rest = els.length - fits;
    more.textContent = `⋯ ${rest}`;
    more.title = `All tabs: ${rest} more`;
    more.setAttribute('aria-label', `All tabs (${rest} more)`);
  }

  // The menu of "⋯ N": all the tabs, the shown one marked.
  function tabItems() {
    const s = set();
    return s.tabs.map((record) => {
      const kind = kindOf(record.kind);
      const title = record.title || kind?.title || record.kind;
      // The second line says what the title does not ("npm run dev · ~/app"
      // under "npm run dev" is "~/app").
      const whole = record.detail && record.detail !== title ? record.detail : '';
      return {
        icon: icon(kind, kind?.title || record.kind), label: record.badge ? `${title} (${record.badge})` : title,
        detail: whole.startsWith(`${title} · `) ? whole.slice(title.length + 3) : whole,
        current: record.id === s.active,
        run: () => activate(record.id),
      };
    });
  }

  // The strip fits its tabs anew when its room changes: once the panel's
  // width settles, not at each step of it opening.
  let settleTimer = 0;
  const settle = (event) => {
    if (event.propertyName !== 'grid-template-columns' || event.target !== layout()?.shell) return;
    clearTimeout(settleTimer);
    settling = event.type === 'transitionrun';
    // A transition that never says it ended does not stop the fitting.
    if (settling) settleTimer = setTimeout(() => { settling = false; fitStrip(); }, 600);
    else fitStrip();
  };
  for (const type of ['transitionrun', 'transitionend', 'transitioncancel']) cockpit.listen(document, type, settle);
  const roomWatch = new ResizeObserver(() => fitStrip());
  roomWatch.observe(bar);
  document.fonts?.ready.then(() => fitStrip(true));
  cockpit.onDispose(() => { roomWatch.disconnect(); clearTimeout(settleTimer); });

  function move(id, before, after) {
    const list = tabs();
    const from = list.findIndex((t) => t.id === id);
    if (from < 0) return;
    const [record] = list.splice(from, 1);
    let to = list.findIndex((t) => t.id === before);
    if (to < 0) to = list.length;
    else if (after) to++;
    list.splice(to, 0, record);
    drawStrip();
    save();
  }

  // ---------------------------------------------------------------- acting on tabs

  function show() {
    ensure();
    if (!isOpen()) layout()?.openPanel?.('sidebar');
    else sync();
  }

  function hide() {
    layout()?.closePanel?.('sidebar');
  }

  // activate makes a tab the one shown, showing the sidebar.
  function activate(id, { focus = true } = {}) {
    const s = set();
    if (!tabNamed(id)) return;
    s.active = id;
    save();
    show();
    sync();
    if (focus) {
      const entry = live.get(id);
      requestAnimationFrame(() => cockpit.safely(() => entry?.view?.focus?.()));
    }
  }

  // open opens a tab of a kind and shows it: the one there is of a kind
  // that has one, or with reuse; in place of Hello when Hello shows. A tab
  // opened again can have the ID it had, and its place (index).
  function open(kindId, { state = null, focus = true, reuse = false, title = '', id = '', index } = {}) {
    const kind = kindOf(kindId);
    const s = set();
    if (kind?.multiple === false || reuse) {
      const existing = s.tabs.find((t) => t.kind === kindId);
      if (existing) {
        if (state != null) {
          const entry = live.get(existing.id);
          existing.state = state;
          cockpit.safely(() => entry?.view?.restore?.(state));
        }
        activate(existing.id, { focus });
        return existing.id;
      }
    }
    const record = { id: id && !tabNamed(id) ? id : cockpit.uuid(), kind: kindId, title: title || kind?.title || kindId, state };
    const current = activeTab();
    if (kindId !== 'hello' && current?.kind === 'hello' && (index == null || s.tabs.length === 1)) {
      // Hello becomes what was picked.
      drop(current.id);
      s.tabs.splice(s.tabs.indexOf(current), 1, record);
    } else if (Number.isInteger(index) && index >= 0) {
      s.tabs.splice(Math.min(index, s.tabs.length), 0, record);
    } else {
      const at = current ? s.tabs.indexOf(current) + 1 : s.tabs.length;
      s.tabs.splice(at, 0, record);
    }
    s.active = record.id;
    save();
    activate(record.id, { focus });
    return record.id;
  }

  // close closes a tab: its view may keep it. The last tab closed leaves
  // Hello; Hello, the last tab, closes the sidebar.
  async function close(id, { force = false } = {}) {
    const s = set();
    const record = tabNamed(id);
    if (!record) return false;
    if (record.kind === 'hello' && s.tabs.length === 1) {
      hide();
      return true;
    }
    const entry = live.get(id);
    if (!force && entry?.view?.close) {
      const keep = await cockpit.safely(() => entry.view.close());
      if (keep === false) return false;
    }
    const at = s.tabs.indexOf(record);
    if (at < 0) return false;
    s.tabs.splice(at, 1);
    drop(id);
    if (s.active === id) s.active = (s.tabs[at] || s.tabs[at - 1])?.id || null;
    ensure();
    save();
    sync();
    return true;
  }

  // toggle shows or hides the sidebar; with a kind, shows a tab of it, or
  // hides the sidebar when it shows one.
  function toggle(kindId) {
    if (!kindId) {
      if (isOpen()) hide();
      else show();
      return;
    }
    const current = activeTab();
    if (shown() && current?.kind === kindId) hide();
    else open(kindId, { reuse: true });
  }

  function next(step = 1, focusTab = false) {
    const list = tabs();
    if (list.length < 2) return;
    const at = list.indexOf(activeTab());
    const target = list[(at + step + list.length) % list.length];
    activate(target.id, { focus: !focusTab });
    if (focusTab) tabNodes.get(target.id)?.focus();
  }

  const api = {
    open, close, activate, toggle, show, hide, isOpen, next,
    visible: (id) => shown() && set().active === id,
    tabs: () => tabs().map((t) => ({ id: t.id, kind: t.kind, title: t.title || '' })),
    active: () => { const t = activeTab(); return t ? { id: t.id, kind: t.kind, title: t.title || '' } : null; },
    kinds: () => kinds().map((k) => ({ id: k.id, title: k.title || k.id, description: k.description || '' })),
    view: (id) => live.get(id)?.view || null,
    node,
  };
  cockpit.provide('sidebar', api);

  // ---------------------------------------------------------------- following the page

  cockpit.on('point:sidebar.tab', () => {
    // A kind loaded anew makes its tabs' views anew; one that went leaves
    // its tabs saying so.
    for (const [id, entry] of live) {
      const record = tabNamed(id);
      if (!record || kindOf(record.kind) !== entry.kind) drop(id);
    }
    sync();
  });
  cockpit.on('layout:panel', () => sync());
  cockpit.on('layout:view', () => sync());
  cockpit.on('session:workspace', (ws) => {
    if (hot.ws === ws) return;
    // The last workspace's views go; its shells run on.
    for (const id of [...live.keys()]) drop(id);
    hot.ws = ws;
    ensure();
    sync();
  });
  // A view is told when its size changes.
  const sized = new ResizeObserver(() => {
    const entry = live.get(activeTab()?.id);
    if (entry?.visible) cockpit.safely(() => entry.view?.resized?.());
  });
  sized.observe(body);
  cockpit.onDispose(() => sized.disconnect());

  // ---------------------------------------------------------------- Hello

  cockpit.contribute('sidebar.tab', {
    id: 'hello', title: 'Hello', icon: HELLO, order: 0, hello: false,
    create(tab) {
      const list = h('div', { class: 'hello-kinds' });
      const extra = h('div', { class: 'hello-extra' });
      const root = h('div', { class: 'hello', id: `hello-${tab.id.slice(0, 8)}` },
        h('header', { class: 'hello-head' },
          h('span', { class: 'label', text: 'Hello' }),
          h('h2', { text: 'What should the sidebar show?' }),
          h('p', { text: 'Pick a tab. Tabs stay per workspace and come back after a reload; plugins add kinds of their own.' })),
        list, extra,
        h('footer', { class: 'hello-keys' },
          h('span', null, h('kbd', { text: 'S' }), ' sidebar'),
          h('span', null, h('kbd', { text: '[' }), h('kbd', { text: ']' }), ' tabs'),
          h('span', null, h('kbd', { text: '+' }), ' new tab')));
      const draw = () => {
        const opened = new Set(tabs().map((t) => t.kind));
        list.replaceChildren(...kinds().filter((k) => k.hello !== false).map((kind) => {
          const single = kind.multiple === false && opened.has(kind.id);
          return h('button', {
            class: 'hello-kind', type: 'button', data: { kind: kind.id },
            onclick: () => open(kind.id, { reuse: kind.multiple === false }),
          },
          h('span', { class: 'hello-icon' }, icon(kind)),
          h('span', { class: 'hello-text' },
            h('b', { text: kind.title || kind.id }),
            kind.description ? h('small', { text: kind.description }) : null),
          single ? h('span', { class: 'hello-open', text: 'open' }) : null,
          kind.key ? h('kbd', { text: kind.key }) : null);
        }));
        extra.replaceChildren(...cockpit.contributions('sidebar.hello', { unique: 'id' })
          .map((section) => cockpit.safely(() => section.render(tab)))
          .filter((n) => n instanceof Node));
      };
      const offKinds = cockpit.on('point:sidebar.tab', draw);
      const offHello = cockpit.on('point:sidebar.hello', draw);
      return {
        node: root, title: 'Hello',
        shown: draw,
        focus: () => list.querySelector('button')?.focus(),
        dispose: () => { offKinds(); offHello(); },
      };
    },
  });

  // ---------------------------------------------------------------- Settings

  cockpit.contribute('sidebar.tab', {
    id: 'settings', title: 'Settings', icon: SLIDERS, order: 90, multiple: false,
    description: 'How the cockpit\'s parts behave: the terminal\'s theme and font, the files shown, and what plugins add.',
    create() {
      const sections = h('div', { class: 'settings-sections' });
      const draw = () => {
        const list = cockpit.contributions('settings.section', { unique: 'id' });
        sections.replaceChildren(...list.map((section) => {
          const content = cockpit.safely(() => section.render());
          return h('section', { class: 'ins set-section', data: { section: section.id } },
            h('h2', { text: section.title || section.id }),
            content instanceof Node ? content : h('p', { class: 'none', text: content == null ? '' : String(content) }));
        }));
        if (!list.length) sections.append(h('p', { class: 'none sb-pad', text: 'No plugin has settings here.' }));
      };
      const off = cockpit.on('point:settings.section', draw);
      return { node: sections, title: 'Settings', shown: draw, dispose: off };
    },
  });

  // ---------------------------------------------------------------- keys, commands, the palette

  cockpit.keys.register({ key: 's', views: ['sessions'], run: () => toggle() });
  cockpit.keys.register({ key: ']', views: ['sessions'], when: () => shown(), run: () => next(1) });
  cockpit.keys.register({ key: '[', views: ['sessions'], when: () => shown(), run: () => next(-1) });
  cockpit.contribute('help.keys', { keys: ['S'], text: 'Sidebar', order: 215 });
  cockpit.contribute('help.keys', { keys: ['[', ']'], text: 'Previous / next sidebar tab', order: 216 });
  cockpit.commands.register({
    name: 'sidebar', args: '[kind]', help: 'Show or hide the sidebar, or open a tab of a kind', order: 215,
    complete: () => kinds().filter((k) => k.id !== 'hello').map((k) => ({ value: k.id, label: k.title || k.id, detail: k.description || '' })),
    run: (arg) => {
      const wanted = String(arg || '').trim().toLowerCase();
      if (!wanted) return toggle();
      const kind = kinds().find((k) => k.id === wanted || (k.title || '').toLowerCase() === wanted);
      if (!kind) return cockpit.toast(`No kind of tab "${wanted}": ${kinds().map((k) => k.id).join(', ')}`, 'error');
      return open(kind.id, { reuse: kind.multiple === false });
    },
  });
  cockpit.contribute('palette.provider', {
    id: 'sidebar', order: 155,
    items: () => [
      { group: 'Sidebar', icon: '◧', label: isOpen() ? 'Hide sidebar' : 'Show sidebar', hint: 'S', order: 155, run: () => toggle() },
      { group: 'Sidebar', icon: '+', label: 'New sidebar tab…', order: 156, run: () => open('hello') },
      ...kinds().filter((k) => k.hello !== false).map((k, i) => ({
        group: 'Sidebar', icon: '▸', label: `Open ${k.title || k.id}`, hint: k.key || '', detail: k.description || '', order: 157 + i,
        run: () => open(k.id, { reuse: k.multiple === false }),
      })),
      ...(activeTab() && isOpen() ? [{ group: 'Sidebar', icon: '×', label: 'Close sidebar tab', order: 190, run: () => close(activeTab().id) }] : []),
    ],
  });

  // The inspector was a panel of its own: a page that had it open shows
  // the sidebar now.
  if (layout()?.panel?.() === 'inspector') layout().openPanel('sidebar');
  hot.ws = workspace();
  ensure();
  sync();
}
