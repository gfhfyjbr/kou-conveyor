// help: the keyboard sheet (?, /help, the button at the foot of the rail).
// It lists what plugins say of their keys — contributions to "help.keys":
// { keys: ['⌘', 'K'], text, order, group } — and the slash commands in
// effect, so it always tells the cockpit as its plugins make it.
//
// Its head — the title, a filter, the close — and its foot stay; the body
// between them scrolls, in sections whose titles hold at its top while
// their lines pass under them. A line goes in the section its group names;
// one with none, in its plugin's (the terminal's, the canvas's) or else in
// one its order gives it. The filter takes words of what a line does, the
// names of its keys (cmd, shift, esc…) and, a lone character, a key itself.

const SEARCH = '<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="7" cy="7" r="4.5" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M10.5 10.5 14 14" stroke="currentColor" stroke-width="1.4"/></svg>';

// The sections, in this order; one a plugin names follows them, by the
// order of its first line.
const SECTIONS = ['Prompt', 'Sessions', 'View', 'Terminal', 'Canvas'];
const BY_PLUGIN = { terminal: 'Terminal', canvas: 'Canvas' };
const BY_ORDER = [[100, 'Prompt'], [200, 'Sessions'], [300, 'View']];

// What the filter knows a key by, besides the key itself.
const NAMES = {
  '⌘': 'cmd command', '⇧': 'shift', '⌥': 'opt option alt', '⌃': 'ctrl control',
  '↵': 'enter return', '⏎': 'enter return', '⌫': 'backspace delete', '⌦': 'delete',
  '↑': 'up arrow', '↓': 'down arrow', '←': 'left arrow', '→': 'right arrow',
  esc: 'escape', ctrl: 'control', alt: 'opt option', '`': 'backtick', ',': 'comma',
  '/': 'slash', '?': 'question', $: 'dollar', '+': 'plus', '−': 'minus', '-': 'minus',
};

// The key column is as wide as the widest keys, up to this; wider ones wrap.
const LEAD_MAX = 176;

function groupOf(row) {
  if (typeof row.group === 'string' && row.group.trim()) return row.group.trim();
  if (BY_PLUGIN[row.plugin]) return BY_PLUGIN[row.plugin];
  const order = Number(row.order) || 0;
  return BY_ORDER.find(([below]) => order < below)?.[1] || 'More';
}

const rankOf = (group) => {
  const at = SECTIONS.indexOf(group);
  return at < 0 ? SECTIONS.length : at;
};

// combosOf: a line's keys as combinations; a lone space separates two.
function combosOf(keys) {
  const combos = [[]];
  for (const key of keys) {
    if (key === ' ') combos.push([]);
    else combos.at(-1).push(key);
  }
  return combos.filter((combo) => combo.length);
}

// tokensOf: the names a key goes by, in lower case — itself, its parts
// (Shift+Esc, ←→, + −) and the words NAMES gives them.
function tokensOf(key) {
  const lower = key.toLowerCase();
  const tokens = new Set([lower]);
  const parts = /^[a-z]\w*(\+\w+)+$/.test(lower) ? lower.split('+')
    : /[a-z0-9]/.test(lower) ? lower.split(/\s+/)
      : [...lower].filter((c) => c.trim());
  for (const part of parts) tokens.add(part);
  for (const token of [...tokens]) {
    for (const word of (NAMES[token] || '').split(' ')) if (word) tokens.add(word);
  }
  return tokens;
}

// nameOf: a key as a word, so that "cmd+k" finds ⌘K.
const nameOf = (key) => (NAMES[key.toLowerCase()] || key.toLowerCase()).split(' ')[0];

const termsOf = (value) => value.trim().toLowerCase().split(/\s+/).filter(Boolean);

// matches: whether a line has every term — a lone character as one of its
// keys, a longer term anywhere in it.
const matches = (line, terms) => terms.every((term) => (term.length === 1 ? line.tokens.has(term) : line.hay.includes(term)));

// hit: whether a key is what a term found.
const hit = (tokens, terms) => terms.some((term) => (term.length === 1 ? tokens.has(term) : [...tokens].some((token) => token.includes(term))));

export default function activate(cockpit) {
  const { h, svg } = cockpit;
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  // With a mouse the filter takes the focus; on a touch screen that would
  // bring the keyboard up over the sheet.
  const fine = window.matchMedia('(hover: hover) and (pointer: fine)');
  const narrow = window.matchMedia('(max-width: 560px)');

  const filter = h('input', {
    id: 'sheet-filter', type: 'search', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off',
    enterkeyhint: 'search', 'aria-label': 'Filter keys and commands', 'aria-controls': 'sheet-body',
    oninput: () => { draw(); body.scrollTop = 0; },
    onkeydown: onFilterKey,
  });
  const clear = h('button', { class: 'act sheet-clear', type: 'button', hidden: true, onclick: () => reset() }, 'Clear');
  const close = h('button', { class: 'icon', id: 'sheet-close', type: 'button', 'aria-label': 'Close', title: 'Close (Esc)', onclick: () => hide() }, '×');
  const body = h('div', { class: 'sheet-scroll', id: 'sheet-body', tabindex: '-1' });
  const count = h('span', { class: 'sheet-count', 'aria-live': 'polite' });
  const escape = h('span', { text: 'close' });
  const overlay = h('div', { class: 'overlay', id: 'sheet', hidden: true, onmousedown: (event) => { if (event.target === overlay) hide(); } },
    h('div', { class: 'sheet ticks', role: 'dialog', 'aria-labelledby': 'sheet-title', onkeydown: onSheetKey },
      h('header', { class: 'sheet-head' },
        h('span', { class: 'label', id: 'sheet-title', text: 'Keyboard' }),
        h('label', { class: 'sheet-find' }, svg(SEARCH), filter),
        clear,
        close),
      body,
      h('footer', { class: 'sheet-foot' },
        count,
        h('span', { class: 'sheet-hint' }, h('kbd', { text: '↑' }), h('kbd', { text: '↓' }), ' scroll'),
        h('span', { class: 'sheet-hint' }, h('kbd', { text: 'Esc' }), ' ', escape))));
  cockpit.ui.mount('overlays', { id: 'sheet', order: 70, node: overlay });
  cockpit.contribute('overlay', { id: 'sheet', order: 70, modal: false, isOpen: () => !overlay.hidden, close: () => hide() });

  const place = () => { filter.placeholder = narrow.matches ? 'Filter' : 'Filter keys and commands'; };
  place();
  cockpit.listen(narrow, 'change', place);

  // lines: what the sheet lists — the keys plugins name, by section, and the
  // commands — with what the filter looks in.
  function lines() {
    const keys = cockpit.contributions('help.keys').map((row) => {
      const group = groupOf(row);
      const combos = combosOf(Array.isArray(row.keys) ? row.keys.map(String) : []);
      let text = String(row.text ?? '');
      if (group === 'Canvas') text = text.replace(/^Canvas:\s*(.)/i, (_, first) => first.toUpperCase()); // its section says so
      const caps = combos.map((combo) => combo.map((key) => ({ key, tokens: tokensOf(key) })));
      const tokens = new Set(caps.flat().flatMap((cap) => [...cap.tokens]));
      const hay = [...tokens, ...combos.map((combo) => combo.join('')), ...combos.map((combo) => combo.map(nameOf).join('+')), text, group]
        .join(' ').toLowerCase();
      return { group, caps, text, tokens, hay };
    });
    const commands = (service('commands')?.list?.() || []).map((command) => {
      const aliases = (command.aliases || []).map(String);
      const names = [command.name, ...aliases];
      const tokens = new Set(['/', ...names, ...names.map((name) => `/${name}`)]);
      const hay = [...tokens, command.args || '', command.help || '', 'commands'].join(' ').toLowerCase();
      return { name: command.name, args: String(command.args || ''), aliases, text: String(command.help || ''), tokens, hay };
    });
    const groups = [...new Set(keys.map((line) => line.group))].sort((a, b) => rankOf(a) - rankOf(b));
    return { keys, commands, groups };
  }

  // marked: text with what the terms find in it marked.
  function marked(text, terms) {
    const lower = text.toLowerCase();
    const spans = [];
    if (lower.length === text.length) {
      for (const term of terms) {
        if (term.length < 2) continue;
        for (let at = lower.indexOf(term); at >= 0; at = lower.indexOf(term, at + term.length)) spans.push([at, at + term.length]);
      }
    }
    if (!spans.length) return [text];
    spans.sort((a, b) => a[0] - b[0]);
    const merged = [];
    for (const [start, end] of spans) {
      const last = merged.at(-1);
      if (last && start <= last[1]) last[1] = Math.max(last[1], end);
      else merged.push([start, end]);
    }
    const out = [];
    let at = 0;
    for (const [start, end] of merged) {
      if (start > at) out.push(text.slice(at, start));
      out.push(h('mark', { text: text.slice(start, end) }));
      at = end;
    }
    if (at < text.length) out.push(text.slice(at));
    return out;
  }

  // prose: a line's text; its clauses, " · " apart, with the dot faint.
  const prose = (text, terms) => text.split(' · ').map((clause, n) => [
    n ? [' ', h('span', { class: 'sep', text: '·' }), ' '] : null,
    marked(clause, terms),
  ]);

  function keyRow(line, terms) {
    return h('div', { class: 'sheet-row' },
      h('dt', null, h('span', { class: 'sheet-lead' }, line.caps.map((combo) => h('span', { class: 'combo' },
        combo.map((cap) => h('kbd', { class: terms.length && hit(cap.tokens, terms) ? 'hit' : null, text: cap.key })))))),
      h('dd', null, prose(line.text, terms)));
  }

  // A command: its name, then what it takes and its other names over what
  // it does — a command's arguments are too long for the key column.
  function commandRow(line, terms) {
    const sig = [
      line.args ? h('span', { class: 'args' }, marked(line.args, terms)) : null,
      line.aliases.length ? h('span', { class: 'aka' }, 'or ', line.aliases.map((alias, n) => [n ? ', ' : '', h('code', null, marked(`/${alias}`, terms))])) : null,
    ].filter(Boolean);
    return h('div', { class: 'sheet-row' },
      h('dt', null, h('code', { class: 'sheet-lead' }, marked(`/${line.name}`, terms))),
      h('dd', null,
        sig.length ? h('span', { class: 'sig' }, sig) : null,
        line.text ? h('span', { class: 'what' }, prose(line.text, terms)) : null));
  }

  function section(title, list, note) {
    return h('section', { class: 'sheet-group' },
      h('h3', { class: 'sheet-group-head' }, h('span', { text: title }), h('i', { 'aria-hidden': 'true' }), note || null),
      list);
  }

  let model = { keys: [], commands: [], groups: [] };

  function draw() {
    const raw = filter.value.trim();
    const terms = termsOf(raw);
    const pick = (list) => (terms.length ? list.filter((line) => matches(line, terms)) : list);
    const keys = pick(model.keys);
    const commands = pick(model.commands);
    const nodes = model.groups.map((group) => {
      const rows = keys.filter((line) => line.group === group);
      return rows.length ? section(group, h('dl', { class: 'sheet-list' }, rows.map((line) => keyRow(line, terms)))) : null;
    });
    if (commands.length) {
      nodes.push(section('Commands',
        h('dl', { class: 'sheet-list sheet-commands', id: 'sheet-commands' }, commands.map((line) => commandRow(line, terms))),
        h('small', null, 'type ', h('kbd', { text: '/' }), ' in the composer · ', h('kbd', { text: 'Tab' }), ' completes')));
    }
    const found = keys.length + commands.length;
    if (!found) {
      nodes.push(h('div', { class: 'sheet-empty' },
        h('p', null, 'Nothing matches ', h('q', { text: raw })),
        h('p', { class: 'sheet-empty-hint', text: 'Try what it does — split, theme — or a key: cmd k, shift, esc' })));
    }
    body.replaceChildren(...nodes.filter(Boolean));
    const total = model.keys.length + model.commands.length;
    count.textContent = !terms.length ? `${model.keys.length} shortcuts · ${model.commands.length} commands`
      : found ? `${found} of ${total}` : 'No matches';
    clear.hidden = !filter.value;
    escape.textContent = filter.value ? 'clear' : 'close';
    if (!terms.length) fit();
  }

  // fit sets the key column as wide as the widest keys, so that the text of
  // every section starts at one line. It measures the layout — offsetWidth,
  // which no transform of the sheet's opening changes; its rounding, the
  // 1px — and so the sheet must show.
  function fit() {
    if (overlay.hidden) return;
    body.style.removeProperty('--lead');
    let widest = 0;
    for (const lead of body.querySelectorAll('.sheet-lead')) widest = Math.max(widest, lead.offsetWidth + 1);
    if (widest > 1) body.style.setProperty('--lead', `${Math.min(widest, LEAD_MAX)}px`);
  }

  function reset() {
    filter.value = '';
    draw();
    body.scrollTop = 0;
    filter.focus({ preventScroll: true });
  }

  // The sheet takes its own Esc, wherever its focus is: the cockpit's closes
  // the first overlay open by order, which may be one under the sheet. In
  // the filter Esc empties it first, then closes the sheet; ? in it empty
  // closes it, as ? opened it and put the focus there ("question" finds the
  // key) — not on a touch screen, where one put it there oneself, to type;
  // the arrows and the page keys there scroll the lines.
  function onFilterKey(event) {
    if (event.isComposing || event.altKey || event.ctrlKey || event.metaKey) return;
    if (event.key === '?' && !filter.value && fine.matches) {
      event.preventDefault();
      event.stopPropagation();
      hide();
      return;
    }
    if (event.shiftKey) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      if (filter.value) reset();
      else hide();
      return;
    }
    const page = Math.max(48, body.clientHeight - 56);
    const by = { ArrowDown: 48, ArrowUp: -48, PageDown: page, PageUp: -page }[event.key];
    if (!by) return;
    event.preventDefault();
    body.scrollBy({ top: by });
  }

  // Out of the filter the keys that scroll the lines scroll them and do
  // nothing else; a character typed goes to the filter, not to the keys of
  // what is under the sheet; ? closes it, as it opened it.
  const SCROLLING = new Set(['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End', ' ']);
  function onSheetKey(event) {
    if (event.target === filter || event.defaultPrevented || event.isComposing) return;
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    if (event.key === 'Escape' && !event.shiftKey) {
      event.preventDefault();
      event.stopPropagation();
      hide();
      return;
    }
    if (SCROLLING.has(event.key)) {
      event.stopPropagation();
      return;
    }
    if (event.key.length !== 1) return;
    event.stopPropagation();
    if (event.key === '?' && !filter.value) {
      event.preventDefault();
      hide();
      return;
    }
    filter.focus({ preventScroll: true });
  }

  // The sheet gives the focus back to where it was when it closes.
  let before = null;

  function open() {
    if (overlay.hidden) {
      before = document.activeElement !== document.body ? document.activeElement : null;
      filter.value = '';
      model = lines();
      overlay.hidden = false;
      draw();
      body.scrollTop = 0;
    }
    (fine.matches ? filter : body).focus({ preventScroll: true });
  }
  function hide() {
    if (overlay.hidden) return;
    overlay.hidden = true;
    if (overlay.contains(document.activeElement)) {
      if (before?.isConnected && before.getClientRects().length) before.focus();
      else document.activeElement.blur();
    }
    before = null;
  }

  // What plugins say changes while the sheet is open: it follows.
  const refresh = () => {
    if (overlay.hidden) return;
    model = lines();
    draw();
  };

  const button = h('button', { class: 'icon', id: 'help-open', type: 'button', title: 'Keyboard shortcuts (?)', 'aria-label': 'Keyboard shortcuts', onclick: open }, '?');
  cockpit.ui.mount('rail.actions', { id: 'help-open', order: 30, node: button });
  cockpit.keys.register({ key: '?', run: () => (overlay.hidden ? open() : hide()) });
  cockpit.commands.register({ name: 'help', aliases: ['?'], help: 'Keyboard shortcuts and commands', order: 250, run: open });
  cockpit.palette.register({ group: 'Actions', icon: '?', label: 'Keyboard shortcuts', hint: '?', order: 190, run: open });
  cockpit.contribute('help.keys', { keys: ['?'], text: 'This sheet', order: 240 });
  cockpit.on('point:help.keys', refresh);
  cockpit.on('point:commands', refresh);
  cockpit.provide('help', { open, close: hide });
}
