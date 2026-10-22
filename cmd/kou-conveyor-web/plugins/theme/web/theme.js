// theme: the design tokens and the components every plugin shares
// (theme.css); light or dark — T, /theme, the palette, a button at the foot
// of the rail; and the appearance — the accent, the shade of the greys, how
// round corners are (appearance.js) — in Settings and /appearance, which the
// terminal cockpit shares through the server's preferences. A plugin that
// only restyles the cockpit can redefine the tokens (--bg, --fg, --accent,
// --radius…) in its own style sheet.
import { ACCENTS, SHADES, CORNERS, MAX_RADIUS, DEFAULT, normalize, normalAccent, same, variables, accentColour, greys } from './appearance.js';

const ICON = '<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="5.5" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M8 2.5v11a5.5 5.5 0 0 0 0-11z" fill="currentColor"/></svg>';
// Where the page's storage keeps the appearance: prefs' key, which
// static/theme.js reads before the first paint.
const STORED = 'kou-conveyor.appearance';
// And whether the rail is compact (prefs' 'rail-compact').
const COMPACT = 'kou-conveyor.rail-compact';

export default function activate(cockpit) {
  const { h, svg, prefs } = cockpit;
  const root = document.documentElement;

  const current = () => root.dataset.theme || (matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark');
  function set(theme) {
    if (theme !== 'light' && theme !== 'dark') return;
    root.dataset.theme = theme;
    prefs.set('theme', theme);
    cockpit.emit('theme', theme);
  }
  const toggle = () => set(current() === 'light' ? 'dark' : 'light');

  // ---------------------------------------------------------------- appearance

  let look = normalize(prefs.get('appearance', null) || DEFAULT);
  // Whether the server keeps preferences, which the terminal cockpit reads
  // too: null until it says.
  let shared = null;
  let unsaved = {};
  let saving = 0;
  let timer = 0;

  // A compact rail stays one panel where round corners set its parts apart
  // (layout.css): this browser's choice, as the terminal has no rail.
  let compact = prefs.get('rail-compact', false) === true;
  function showCompact() {
    if (compact) root.dataset.railCompact = '';
    else delete root.dataset.railCompact;
  }
  function setCompact(on, { keep = true } = {}) {
    if (!!on === compact) return;
    compact = !!on;
    if (keep) prefs.set('rail-compact', compact || null);
    showCompact();
    cockpit.emit('appearance', { ...look, compact });
  }
  showCompact();

  // The tab's icon takes the accent and the shade too.
  const link = document.querySelector('link[rel="icon"]');
  const ownIcon = link?.getAttribute('href');
  function icon() {
    if (!link || !ownIcon) return;
    if (look.accent === DEFAULT.accent && look.shade === DEFAULT.shade) {
      if (link.getAttribute('href') !== ownIcon) link.setAttribute('href', ownIcon);
      return;
    }
    const g = greys(look.shade, true);
    const mark = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><rect width="32" height="32" fill="${g.bg}"/><rect x="5.5" y="5.5" width="21" height="21" fill="none" stroke="${g.fg}" stroke-width="3"/><rect x="15" y="15" width="9" height="9" fill="${accentColour(look.accent, true)}"/></svg>`;
    link.setAttribute('href', `data:image/svg+xml,${encodeURIComponent(mark)}`);
  }

  // show puts an appearance on the page — the --kou-* properties the tokens
  // read, and the corners' mark the ticks follow — and keeps it for the
  // next page's first paint, unless it came from there.
  function show(next, { keep = true } = {}) {
    look = normalize(next);
    const vars = variables(look);
    for (let i = root.style.length - 1; i >= 0; i--) {
      const name = root.style.item(i);
      if (name.startsWith('--kou-') && !(name in vars)) root.style.removeProperty(name);
    }
    for (const [name, value] of Object.entries(vars)) root.style.setProperty(name, value);
    if (look.radius > 0) root.dataset.corners = 'round';
    else delete root.dataset.corners;
    icon();
    if (keep) prefs.set('appearance', same(look, DEFAULT) ? null : { ...look, vars });
    cockpit.emit('appearance', { ...look, compact });
  }

  // choose shows a change at once and saves it a moment later, once the
  // slider or the colour picker comes to rest; a compact rail is this
  // browser's.
  function choose(change) {
    if (change && 'compact' in change) setCompact(change.compact);
    const next = normalize({ ...look, ...change });
    if (same(next, look)) return;
    for (const key of ['accent', 'shade', 'radius']) if (next[key] !== look[key]) unsaved[key] = next[key];
    show(next);
    clearTimeout(timer);
    timer = setTimeout(save, 350);
  }
  async function save() {
    const body = unsaved;
    unsaved = {};
    if (!Object.keys(body).length) return;
    await synced;
    if (!shared) return;
    saving++;
    try {
      await cockpit.api('/api/preferences', { method: 'PUT', body });
    } catch (error) {
      cockpit.toast(`The appearance stays in this browser: ${error.message}`, 'error', 'appearance');
    } finally {
      saving--;
    }
  }

  // The server's preferences — which the terminal cockpit writes too — win
  // over what this browser kept: when the page loads, and when it comes
  // back to the front.
  async function pull() {
    try {
      const p = await cockpit.api('/api/preferences');
      shared = !!p.shared;
      if (shared && !saving && !Object.keys(unsaved).length) {
        const theirs = normalize(p);
        if (!same(theirs, look)) show(theirs);
      }
    } catch {
      shared ??= false;
    }
    redraw();
  }
  show(look);
  let synced = pull();
  let pulled = Date.now();
  const again = () => {
    if (document.visibilityState !== 'visible' || Date.now() - pulled < 5000) return;
    pulled = Date.now();
    synced = pull();
  };
  cockpit.listen(document, 'visibilitychange', again);
  cockpit.listen(window, 'focus', again);
  // Another tab chose, or a canvas's chat in this one: so does this page.
  cockpit.listen(window, 'storage', (event) => {
    if (event.key === COMPACT || event.key === null) setCompact(prefs.get('rail-compact', false) === true, { keep: false });
    if (event.key !== STORED && event.key !== null) return;
    let theirs = DEFAULT;
    try { theirs = normalize(JSON.parse(localStorage.getItem(STORED)) || DEFAULT); } catch { /* none kept */ }
    if (!same(theirs, look)) show(theirs, { keep: false });
  });

  cockpit.provide('theme', { current, set, toggle, appearance: () => ({ ...look, compact }), choose });
  cockpit.ui.mount('rail.actions', {
    id: 'theme-toggle', order: 20,
    node: h('button', { class: 'icon', id: 'theme-toggle', type: 'button', title: 'Toggle light / dark (T)', 'aria-label': 'Toggle theme', onclick: toggle }, svg(ICON)),
  });
  cockpit.keys.register({ key: 't', run: toggle });
  cockpit.commands.register({ name: 'theme', help: 'Switch between light and dark', order: 230, run: toggle });
  cockpit.palette.register({ group: 'Actions', icon: '◐', label: 'Toggle light / dark', hint: 'T', order: 170, run: toggle });
  cockpit.contribute('help.keys', { keys: ['T'], text: 'Light / dark', order: 230 });

  // ---------------------------------------------------------------- Settings

  // The Appearance sections drawn, which follow every change: one made
  // here, in another tab, in the terminal cockpit, or light and dark.
  const drawn = new Set();
  let drawings = 0;
  function redraw() {
    for (const section of drawn) {
      if (!section.node.isConnected) drawn.delete(section);
      else cockpit.safely(() => section.sync());
    }
  }
  cockpit.on('appearance', redraw);
  cockpit.on('theme', redraw);
  cockpit.listen(matchMedia('(prefers-color-scheme: light)'), 'change', redraw);

  cockpit.contribute('settings.section', {
    id: 'appearance', title: 'Appearance', order: 5,
    render() {
      const group = `look-${++drawings}`;
      const option = (kind, value, text, mark) => h('label', { title: text },
        h('input', { type: 'radio', name: `${group}-${kind}`, value: String(value), data: { kind }, onchange: () => choose({ [kind]: kind === 'radius' ? Number(value) : value }) }),
        h('span', null, mark, text));

      const accents = h('div', { class: 'look', role: 'radiogroup', 'aria-label': 'Accent' },
        ACCENTS.map((a) => option('accent', a.name, a.label, h('i', { class: 'look-dot', data: { accent: a.name } }))));
      const customDot = h('i', { class: 'look-dot' });
      const customRadio = h('input', { type: 'radio', name: `${group}-accent`, value: '', tabindex: '-1', 'aria-hidden': 'true' });
      const picker = h('input', { type: 'color', 'aria-label': 'An accent colour of your own', oninput: (event) => choose({ accent: event.currentTarget.value }) });
      accents.append(h('label', { class: 'look-custom', title: 'A colour of your own' }, customRadio, picker, h('span', null, customDot, 'Custom')));

      const shades = h('div', { class: 'look', role: 'radiogroup', 'aria-label': 'Shade' },
        SHADES.map((s) => option('shade', s.name, s.label, h('i', { class: 'look-shade', data: { shade: s.name } }, h('i'), h('i'), h('i')))));

      const corners = h('div', { class: 'look', role: 'radiogroup', 'aria-label': 'Corners' },
        CORNERS.map((c) => {
          const mark = h('i', { class: 'look-corner' });
          mark.style.setProperty('--r', `${c.radius}px`);
          return option('radius', c.radius, c.label, mark);
        }));
      const radius = h('input', {
        type: 'range', min: '0', max: String(MAX_RADIUS), step: '1', 'aria-label': 'Corner radius',
        oninput: (event) => choose({ radius: Number(event.currentTarget.value) }),
      });
      const px = h('output');
      const compactBox = h('input', { type: 'checkbox', onchange: (event) => setCompact(event.currentTarget.checked) });
      const reset = h('button', { class: 'act', type: 'button', onclick: () => choose(DEFAULT) }, 'Reset to orange, graphite, sharp');
      const where = h('p', { class: 'set-note' });

      const node = h('div', null,
        h('div', { class: 'set-row' }, h('span', { class: 'label', text: 'Accent' }), accents,
          h('p', { class: 'set-note', text: 'The colour of what is current, chosen and at work. A colour of your own is made lighter or darker where it would not read on the background.' })),
        h('div', { class: 'set-row' }, h('span', { class: 'label', text: 'Shade' }), shades,
          h('p', { class: 'set-note', text: 'The tint of the greys: backgrounds, rules and quiet text. Graphite is warm; Neutral has no tint at all.' })),
        h('div', { class: 'set-row' }, h('span', { class: 'label', text: 'Corners' }), corners,
          h('div', { class: 'look-range' }, radius, px),
          h('label', { class: 'set-toggle' }, compactBox, 'Compact rail: one panel, no gaps'),
          h('p', { class: 'set-note', text: 'Sharp keeps the cockpit as it was made, square. Rounder corners go to what you press, type in and open — buttons, fields, tabs, cards, dialogs — and set the rail\'s parts apart, unless the rail is compact (kept in this browser).' })),
        h('div', { class: 'set-row' }, h('div', { class: 'ins-actions' }, reset), where));

      function sync() {
        const dark = current() === 'dark';
        for (const dot of accents.querySelectorAll('.look-dot[data-accent]')) dot.style.setProperty('--sw', accentColour(dot.dataset.accent, dark));
        const custom = !ACCENTS.some((a) => a.name === look.accent);
        customRadio.checked = custom;
        if (custom) {
          customDot.dataset.set = '';
          customDot.style.setProperty('--sw', accentColour(look.accent, dark));
          picker.value = look.accent;
        } else {
          delete customDot.dataset.set;
          customDot.style.removeProperty('--sw');
          picker.value = accentColour(look.accent, true);
        }
        for (const input of node.querySelectorAll('input[type="radio"][data-kind]')) {
          const kind = input.dataset.kind;
          input.checked = kind === 'radius' ? Number(input.value) === look.radius : input.value === look[kind];
        }
        for (const mark of shades.querySelectorAll('.look-shade')) {
          const g = greys(mark.dataset.shade, dark);
          const [ground, raised, text] = mark.children;
          ground.style.background = g['bg-2'];
          raised.style.background = g['bg-4'];
          text.style.background = g['fg-3'];
        }
        radius.value = String(look.radius);
        px.textContent = look.radius ? `${look.radius} px` : 'square';
        compactBox.checked = compact;
        compactBox.disabled = look.radius === 0;
        reset.disabled = same(look, DEFAULT);
        where.textContent = shared === true
          ? 'Kept in the server\'s preferences, which the terminal cockpit (kou-conveyor-tui) reads too: a change shows in both. Code keeps its own colours.'
          : shared === false ? 'Kept in this browser: the server keeps no preferences without KOU_CONVEYOR_CONFIG.' : '';
      }
      drawn.add({ node, sync });
      sync();
      return node;
    },
  });

  // ---------------------------------------------------------------- /appearance

  const openSettings = () => {
    if (!cockpit.has('sidebar')) return cockpit.toast('The appearance is chosen in Settings, which the sidebar shows.', 'error');
    cockpit.use('sidebar').open('settings', { reuse: true });
    requestAnimationFrame(() => document.querySelector('.set-section[data-section="appearance"]')?.scrollIntoView({ block: 'start' }));
    return undefined;
  };
  const corner = (value) => {
    const preset = CORNERS.find((c) => c.name === value);
    const n = preset ? preset.radius : Number(value.replace(/px$/, ''));
    return value !== '' && Number.isInteger(n) && n >= 0 && n <= MAX_RADIUS ? n : null;
  };
  cockpit.commands.register({
    name: 'appearance', aliases: ['accent', 'corners'], args: '[accent|shade|corners|compact] [value]', order: 231,
    help: 'The accent colour, the shade of the greys and how round corners are',
    complete: () => [
      ...ACCENTS.map((a) => ({ value: `accent ${a.name}`, label: `accent ${a.name}`, detail: 'Accent colour', current: look.accent === a.name })),
      ...SHADES.map((s) => ({ value: `shade ${s.name}`, label: `shade ${s.name}`, detail: 'Shade of the greys', current: look.shade === s.name })),
      ...CORNERS.map((c) => ({ value: `corners ${c.name}`, label: `corners ${c.name}`, detail: c.radius ? `${c.radius}px` : 'square', current: look.radius === c.radius })),
      { value: 'compact on', label: 'compact on', detail: 'The rail one panel, no gaps', current: compact },
      { value: 'compact off', label: 'compact off', detail: 'The rail\'s parts apart, when round', current: !compact },
      { value: 'reset', label: 'reset', detail: 'Orange on graphite, square corners' },
    ],
    run: (arg) => {
      const [what = '', ...rest] = String(arg || '').trim().toLowerCase().split(/\s+/).filter(Boolean);
      const value = rest.join(' ');
      if (!what) return openSettings();
      if (what === 'reset') return choose(DEFAULT);
      if (what === 'compact' && ['', 'on', 'off'].includes(value)) return setCompact(value ? value === 'on' : !compact);
      if (what === 'accent' && normalAccent(value)) return choose({ accent: value });
      if (what === 'shade' && SHADES.some((s) => s.name === value)) return choose({ shade: value });
      if ((what === 'corners' || what === 'radius') && corner(value) != null) return choose({ radius: corner(value) });
      // A value alone says what it is.
      if (!value && normalAccent(what)) return choose({ accent: what });
      if (!value && SHADES.some((s) => s.name === what)) return choose({ shade: what });
      if (!value && corner(what) != null) return choose({ radius: corner(what) });
      return cockpit.toast(`/appearance accent ${ACCENTS.map((a) => a.name).join('|')}|#rrggbb · shade ${SHADES.map((s) => s.name).join('|')} · corners ${CORNERS.map((c) => c.name).join('|')}|0–${MAX_RADIUS} · compact on|off · reset`, 'error');
    },
  });
  cockpit.palette.register({ group: 'Actions', icon: '◧', label: 'Appearance: accent, shade, corners…', order: 171, run: openSettings });
}
