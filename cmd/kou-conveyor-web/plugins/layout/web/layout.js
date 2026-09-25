// layout: the frame of the page, and nothing in it. Other plugins fill it
// through the slots it offers:
//
//   rail.head       the head of the rail (the mark)
//   rail.foot       the foot of the rail, before its buttons
//   rail.actions    the buttons at the foot of the rail
//   bar.crumbs      the stage's bar: what is in view
//   bar.end         the stage's bar: badges and buttons on the right
//   stage.main      the stage's scroller (the transcript)
//   dock            under the stage: banners, the queue, the composer
//   dock.float      over the dock (the button that jumps to the latest)
//   stage.overlay   over the stage (a picture shown large)
//   overlays        over everything: dialogs, menus, toasts
//
// and through two contribution points:
//
//   layout.view     { id, title, order, badge, rail, page, select(), shown(), hidden() }:
//                   a view with a tab in the rail. rail is shown in the rail
//                   while the view is; page, if any, takes the stage's place.
//   layout.panel    { id, order, width, minWidth, node, opened(), closed() }:
//                   a side panel; one is open at a time. width is where it
//                   starts, minWidth (pixels) the least it can be made.
//
// The rail and the panels are as wide as the user makes them: their inner
// edges drag, or step with the arrow keys once focused, a double-click
// gives an edge its default back, and the widths are kept. The stage keeps
// room beside them as the window narrows.
//
// It provides the layout service: views, panels, their widths and the rail
// of narrow screens.
const MENU = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2 4h12M2 8h12M2 12h12" stroke="currentColor" stroke-width="1.4"/></svg>';
const MARK = '<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="1.5" y="1.5" width="17" height="17" fill="none" stroke="currentColor" stroke-width="2"/><rect x="9" y="9" width="7" height="7" class="mark-core"/></svg>';

// The least widths, in pixels: the rail's, a panel's that names none, and
// what the stage keeps beside them while the window has room for it.
const RAIL_MIN = 200;
const PANEL_MIN = 260;
const STAGE_MIN = 420;
// How far the arrow keys move a focused edge; with Shift, further.
const STEP = 16;
const LEAP = 64;

// keptWidths reads the widths kept, leaving out what is not a width.
function keptWidths(saved) {
  const out = {};
  if (saved && typeof saved === 'object') {
    for (const [key, value] of Object.entries(saved)) if (Number.isFinite(value) && value > 0) out[key] = Math.round(value);
  }
  return out;
}

export default function activate(cockpit) {
  const { h, svg, prefs } = cockpit;
  const hot = cockpit.hot.data;
  // The panel open: kept, and taken from the choices the cockpit kept
  // before panels were plugins.
  if (!('panel' in hot)) {
    const saved = prefs.get('panel', undefined);
    hot.panel = saved !== undefined ? saved
      : prefs.get('changes', false) ? 'changes' : prefs.get('inspector', window.innerWidth >= 1280) ? 'inspector' : null;
  }
  hot.view ??= null;
  // The widths the user gave the rail ('rail') and the panels (by id), in
  // pixels; what has none has its default.
  const widths = keptWidths(prefs.get('widths', null));

  // ---------------------------------------------------------------- frame

  const shell = h('div', { class: 'shell', id: 'shell', data: { rail: 'closed', tab: 'sessions' } });
  const railHead = h('header', { class: 'rail-head' });
  const tabs = h('nav', { class: 'rail-tabs', id: 'rail-tabs', role: 'tablist', 'aria-label': 'Views' });
  const railViews = h('div', { class: 'slot-contents' });
  const footStart = h('div', { class: 'slot-contents' });
  const railActions = h('div', { class: 'rail-actions' });
  const rail = h('aside', { class: 'rail', id: 'rail', 'aria-label': 'Sessions' },
    railHead, tabs, railViews, h('footer', { class: 'rail-foot' }, footStart, railActions));
  const scrim = h('div', { class: 'scrim', id: 'scrim', onclick: () => setRail(false) });
  const pages = h('div', { class: 'slot-contents' });
  const railToggle = h('button', { class: 'icon rail-toggle', id: 'rail-toggle', type: 'button', 'aria-label': 'Sessions', onclick: () => setRail() }, svg(MENU));
  const crumbs = h('div', { class: 'slot-contents' });
  const barEnd = h('div', { class: 'bar-right' });
  const bar = h('header', { class: 'bar' }, railToggle, crumbs, barEnd);
  const scroll = h('section', { class: 'scroll', id: 'scroll' });
  const dockInner = h('div', { class: 'dock-inner' });
  const dock = h('footer', { class: 'dock' }, dockInner);
  const stageOverlay = h('div', { class: 'slot-contents' });
  const stage = h('main', { class: 'stage' }, pages, bar, scroll, dock, stageOverlay);
  const side = h('div', { class: 'slot-contents' });
  const railEdge = edgeHandle('rail', 'Resize the sidebar');
  const sideEdge = edgeHandle('side', 'Resize the panel');
  sideEdge.hidden = true;
  shell.append(rail, railEdge, scrim, stage, side, sideEdge);
  const overlays = h('div', { class: 'slot-contents', id: 'overlays' });
  // The rail has its width before the shell first shows, so it does not
  // ease into it.
  applyRail();
  document.body.prepend(shell);
  document.body.append(overlays);
  cockpit.onDispose(() => {
    shell.remove();
    overlays.remove();
  });

  const slots = {
    'rail.head': railHead, 'rail.foot': footStart, 'rail.actions': railActions, 'bar.crumbs': crumbs, 'bar.end': barEnd,
    'stage.main': scroll, dock: dockInner, 'dock.float': dock, 'stage.overlay': stageOverlay, overlays,
  };
  for (const [name, el] of Object.entries(slots)) cockpit.ui.slot(name, el);

  cockpit.ui.mount('rail.head', {
    id: 'mark', order: 0,
    node: h('a', { class: 'mark', href: '#/', 'aria-label': 'kou-conveyor — new session' }, svg(MARK), h('span', null, 'kou', h('i', { text: '-' }), 'conveyor')),
  });

  // ---------------------------------------------------------------- views

  const views = () => cockpit.contributions('layout.view', { unique: 'id' });
  const viewNamed = (id) => views().find((v) => v.id === id) || null;
  let shownView = null; // the view whose shown() ran last

  function currentView() {
    const list = views();
    return list.find((v) => v.id === hot.view) || list[0] || null;
  }

  // show makes a view the one in view; a view that is not there yet shows
  // once it is.
  function show(id) {
    if (!id) return;
    hot.view = id;
    renderViews();
  }

  function renderViews() {
    const list = views();
    const active = currentView();
    const id = active?.id || hot.view || '';
    // The rail: each view's part, the active one shown.
    const railNodes = [];
    const pageNodes = [];
    for (const v of list) {
      if (v.rail instanceof Node) {
        v.rail.classList.add('rail-view');
        v.rail.hidden = v !== active;
        railNodes.push(v.rail);
      }
      if (v.page instanceof Node) {
        v.page.classList.add('page');
        v.page.hidden = v !== active;
        pageNodes.push(v.page);
      }
    }
    place(railViews, railNodes);
    place(pages, pageNodes);
    // The tabs, when there is more than one view.
    tabs.hidden = list.length < 2;
    place(tabs, list.map((v) => tabFor(v, v === active)));
    if (shell.dataset.tab !== id) shell.dataset.tab = id;
    shell.toggleAttribute('data-page', !!(active?.page instanceof Node));
    cockpit.store.set('view', id);
    if (shownView !== active) {
      const previous = shownView;
      shownView = active;
      if (previous) cockpit.safely(() => previous.hidden?.());
      if (active) cockpit.safely(() => active.shown?.());
      if (cockpit.has('menu')) cockpit.use('menu').close();
      cockpit.emit('layout:view', id, previous?.id || '');
    }
    applyPanel();
    cockpit.render();
  }

  const tabNodes = new WeakMap();
  function tabFor(v, selected) {
    let node = tabNodes.get(v);
    if (!node) {
      node = h('button', {
        class: 'rail-tab', id: `tab-${v.id}`, type: 'button', role: 'tab', data: { tab: v.id },
        onclick: () => (v.select ? cockpit.safely(v.select) : show(v.id)),
      }, v.title || v.id, v.badge instanceof Node ? v.badge : null);
      tabNodes.set(v, node);
    }
    node.title = v.tabTitle || v.title || v.id;
    node.setAttribute('aria-selected', String(selected));
    return node;
  }

  // place makes box's children the nodes given, moving as few as it can.
  function place(box, nodes) {
    const want = new Set(nodes);
    for (const child of [...box.children]) if (!want.has(child)) child.remove();
    let at = box.firstChild;
    for (const node of nodes) {
      if (node === at) at = at.nextSibling;
      else box.insertBefore(node, at);
    }
  }

  cockpit.on('point:layout.view', renderViews);

  // ---------------------------------------------------------------- panels

  const panels = () => cockpit.contributions('layout.panel', { unique: 'id' });
  let openedPanel = null;

  function applyPanel() {
    const list = panels();
    place(side, list.map((p) => p.node).filter((node) => node instanceof Node));
    const open = list.find((p) => p.id === hot.panel) || null;
    for (const p of list) {
      if (!(p.node instanceof Node)) continue;
      p.node.classList.add('side-panel');
      p.node.toggleAttribute('data-open', p === open);
      // A panel that floats over the stage, on a narrow screen, takes the
      // width chosen too.
      if (widths[p.id]) p.node.style.setProperty('--side-width', `${widths[p.id]}px`);
      else p.node.style.removeProperty('--side-width');
      shell.dataset[p.id] = p === open ? 'open' : 'closed';
    }
    const page = shell.hasAttribute('data-page');
    if (open && !page) {
      const width = widths[open.id] ? `${widths[open.id]}px` : open.width || 'var(--inspector)';
      shell.style.setProperty('--side', `clamp(${panelMin(open)}px, ${width}, calc(100vw - var(--rail) - ${STAGE_MIN}px))`);
    } else {
      shell.style.setProperty('--side', '0px');
    }
    shell.dataset.panel = open?.id || '';
    if (openedPanel !== open) {
      const previous = openedPanel;
      openedPanel = open;
      if (previous) cockpit.safely(() => previous.closed?.());
      if (open) cockpit.safely(() => open.opened?.());
    }
    placeSideEdge();
  }

  function setPanel(id) {
    hot.panel = id || null;
    prefs.set('panel', hot.panel);
    applyPanel();
    cockpit.emit('layout:panel', hot.panel);
    cockpit.render();
  }

  cockpit.on('point:layout.panel', () => {
    applyPanel();
    cockpit.render();
  });

  // ---------------------------------------------------------------- widths

  // An edge is the rail's ('rail') or the open panel's ('side'): a strip over
  // it that drags, steps with the arrow keys once focused, and gives the
  // default width back on a double-click.
  function edgeHandle(edge, label) {
    const handle = h('div', {
      class: 'resizer', id: `${edge}-resizer`, role: 'separator', tabindex: '0', 'aria-orientation': 'vertical',
      'aria-label': label, 'aria-controls': edge === 'rail' ? 'rail' : null, data: { edge },
      title: 'Drag to resize · double-click restores the width',
    });
    handle.addEventListener('pointerdown', (event) => cockpit.safely(() => drag(edge, event)));
    handle.addEventListener('dblclick', () => cockpit.safely(() => restore(edge)));
    handle.addEventListener('keydown', (event) => cockpit.safely(() => stepKey(edge, event)));
    handle.addEventListener('focus', () => cockpit.safely(() => describe(edge)));
    return handle;
  }

  const openNode = () => (openedPanel?.node instanceof Element && !shell.hasAttribute('data-page') ? openedPanel.node : null);
  // keyOf is what an edge's width is kept under: 'rail', or the open panel's id.
  const keyOf = (edge) => (edge === 'rail' ? 'rail' : openNode() ? openedPanel.id : null);
  const floats = (el) => getComputedStyle(el).position === 'fixed';
  const panelMin = (p) => (Number.isFinite(p?.minWidth) && p.minWidth > 0 ? p.minWidth : PANEL_MIN);

  // widthOf is how wide an edge's column is now.
  function widthOf(edge) {
    const node = edge === 'rail' ? rail : openNode();
    return node ? Math.round(node.getBoundingClientRect().width) : 0;
  }

  // limits are the least and the most an edge's width can be now: the stage
  // keeps STAGE_MIN beside the rail and a panel that stands beside it, and a
  // panel that floats over the stage can take most of the window.
  function limits(edge) {
    const room = window.innerWidth;
    const node = openNode();
    if (edge === 'rail') {
      const panel = node && !floats(node) ? node.getBoundingClientRect().width : 0;
      return [RAIL_MIN, Math.max(RAIL_MIN, Math.floor(room - panel - STAGE_MIN))];
    }
    const least = panelMin(openedPanel);
    if (node && floats(node)) return [least, Math.max(least, Math.floor(room * 0.92))];
    const beside = floats(rail) ? 0 : rail.getBoundingClientRect().width;
    return [least, Math.max(least, Math.floor(room - beside - STAGE_MIN))];
  }

  // applyRail gives the rail the width chosen for it; as the window
  // narrows, the stage keeps STAGE_MIN. The rail that slides over the stage
  // on a narrow screen takes the width as it was chosen (--rail-width).
  function applyRail() {
    if (widths.rail) {
      shell.style.setProperty('--rail', `clamp(${RAIL_MIN}px, ${widths.rail}px, calc(100vw - ${STAGE_MIN}px))`);
      shell.style.setProperty('--rail-width', `${widths.rail}px`);
    } else {
      shell.style.removeProperty('--rail');
      shell.style.removeProperty('--rail-width');
    }
  }

  // adjust changes the widths chosen and applies them, keeping the
  // transcript at its end if it was there.
  function adjust(edge, change) {
    const atEnd = scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight < 24;
    change();
    if (edge === 'rail') applyRail();
    else applyPanel();
    if (atEnd) scroll.scrollTop = scroll.scrollHeight;
  }

  // setWidth gives an edge a width within its limits, or its default back
  // (null).
  function setWidth(edge, width, bounds = limits(edge)) {
    const key = keyOf(edge);
    if (!key) return;
    adjust(edge, () => {
      if (width == null) delete widths[key];
      else widths[key] = Math.round(Math.min(Math.max(width, bounds[0]), bounds[1]));
    });
  }

  // instantly makes a change to the widths without the grid easing into it.
  function instantly(change) {
    shell.style.transition = 'none';
    change();
    void shell.offsetWidth; // the grid takes the widths now
    shell.style.transition = '';
  }

  const save = () => prefs.set('widths', Object.keys(widths).length ? { ...widths } : null);

  // describe tells assistive technology where an edge is, and how far it goes.
  function describe(edge) {
    const handle = edge === 'rail' ? railEdge : sideEdge;
    const [least, most] = limits(edge);
    handle.setAttribute('aria-valuemin', String(least));
    handle.setAttribute('aria-valuemax', String(most));
    handle.setAttribute('aria-valuenow', String(Math.min(Math.max(widthOf(edge), least), most)));
  }

  // drag has an edge follow the pointer that went down on it until it lets
  // go; Esc puts the width back.
  let dragging = null;
  function drag(edge, event) {
    const key = keyOf(edge);
    if (event.button !== 0 || dragging || !key) return;
    event.preventDefault();
    const handle = event.currentTarget;
    const start = { x: event.clientX, width: widthOf(edge), kept: widths[key] };
    const bounds = limits(edge);
    const stop = new AbortController();
    const { signal } = stop;
    let moved = false;
    dragging = stop;
    try {
      handle.setPointerCapture(event.pointerId);
    } catch { /* the pointer went already */ }
    shell.dataset.resizing = edge;
    const finish = (keep) => {
      if (signal.aborted) return;
      stop.abort();
      dragging = null;
      if (moved && !keep) {
        adjust(edge, () => {
          if (start.kept) widths[key] = start.kept;
          else delete widths[key];
        });
      }
      void shell.offsetWidth; // the width put back does not ease in
      delete shell.dataset.resizing;
      if (moved && keep) save();
      describe(edge);
    };
    handle.addEventListener('pointermove', (e) => cockpit.safely(() => {
      const dx = e.clientX - start.x;
      if (!moved && !dx) return;
      moved = true;
      setWidth(edge, start.width + (edge === 'rail' ? dx : -dx), bounds);
    }), { signal });
    handle.addEventListener('pointerup', () => finish(true), { signal });
    handle.addEventListener('lostpointercapture', () => finish(true), { signal });
    handle.addEventListener('pointercancel', () => finish(false), { signal });
    window.addEventListener('keydown', (e) => {
      if (e.key !== 'Escape') return;
      e.preventDefault();
      e.stopPropagation();
      finish(false);
    }, { capture: true, signal });
  }
  cockpit.onDispose(() => dragging?.abort());

  // stepKey moves a focused edge: ← and → by STEP, or LEAP with Shift; Home
  // and End to the least and the most it can be.
  function stepKey(edge, event) {
    if (event.altKey || event.ctrlKey || event.metaKey || !keyOf(edge)) return;
    const bounds = limits(edge);
    const way = { ArrowLeft: -1, ArrowRight: 1 }[event.key];
    let width;
    if (way) width = widthOf(edge) + (edge === 'rail' ? way : -way) * (event.shiftKey ? LEAP : STEP);
    else if (event.key === 'Home') width = bounds[0];
    else if (event.key === 'End') width = bounds[1];
    else return;
    event.preventDefault();
    instantly(() => setWidth(edge, width, bounds));
    save();
    describe(edge);
  }

  // restore gives an edge its default width back.
  function restore(edge) {
    if (!keyOf(edge)) return;
    instantly(() => setWidth(edge, null));
    save();
    describe(edge);
  }

  // The side edge sits on the open panel's inner edge, wherever the panel's
  // style puts it — beside the stage, or over it — and follows it as it
  // eases open or the window changes. The stage's width is news too:
  // "layout:resize".
  let watched = null;
  let stageWidth = -1;
  const watcher = new ResizeObserver((entries) => {
    for (const entry of entries) {
      if (entry.target === stage) {
        const width = Math.round(entry.contentRect.width);
        if (width === stageWidth) continue;
        stageWidth = width;
        cockpit.emit('layout:resize', width);
      } else if (entry.target === watched) {
        placeSideEdge();
      }
    }
  });
  watcher.observe(stage);
  cockpit.onDispose(() => watcher.disconnect());

  function placeSideEdge() {
    const node = openNode();
    if (watched !== node) {
      if (watched) watcher.unobserve(watched);
      watched = node;
      if (node) {
        watcher.observe(node);
        sideEdge.setAttribute('aria-label', `Resize ${(node.getAttribute('aria-label') || openedPanel.id).toLowerCase()}`);
        if (node.id) sideEdge.setAttribute('aria-controls', node.id);
        else sideEdge.removeAttribute('aria-controls');
      }
    }
    const width = node ? node.getBoundingClientRect().width : 0;
    sideEdge.hidden = width < 1;
    if (width >= 1) sideEdge.style.setProperty('--edge', `${width}px`);
  }

  // ---------------------------------------------------------------- rail of narrow screens

  function setRail(open) {
    const next = open ?? shell.dataset.rail !== 'open';
    shell.dataset.rail = next ? 'open' : 'closed';
  }
  cockpit.contribute('overlay', {
    id: 'rail', order: 90, modal: false,
    isOpen: () => shell.dataset.rail === 'open',
    close: () => setRail(false),
  });

  cockpit.provide('layout', {
    shell, stage, rail,
    scroller: () => scroll,
    view: () => currentView()?.id || hot.view || '',
    views: () => views().map((v) => v.id),
    show,
    panel: () => hot.panel,
    panelOpen: (id) => hot.panel === id && panels().some((p) => p.id === id),
    openPanel: (id) => setPanel(id),
    closePanel: (id) => { if (!id || hot.panel === id) setPanel(null); },
    togglePanel: (id) => setPanel(hot.panel === id ? null : id),
    // The widths of the rail ('rail') and the panels, by id: width is what
    // one has now (0 when it is not shown), setWidth chooses one as a drag
    // does, or gives the default back (null).
    width: (id) => (id === 'rail' ? widthOf('rail') : openNode() && openedPanel.id === id ? widthOf('side') : 0),
    setWidth(id, px) {
      if (px == null) delete widths[id];
      else if (Number.isFinite(px) && px > 0) widths[id] = Math.round(px);
      else return;
      instantly(() => {
        applyRail();
        applyPanel();
      });
      save();
    },
    rail: setRail,
    railOpen: () => shell.dataset.rail === 'open',
  });

  renderViews();
}
