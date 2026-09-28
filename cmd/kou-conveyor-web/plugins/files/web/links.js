// Links in prompts: the $ references of a text, read as the cockpits' Go
// code reads them (cmd/internal/cockpit/links.go); completing the one being
// typed in a textarea; and what the model saw of the files a sent prompt
// linked.

import { fmt, h } from '/kernel/dom.js';

const MAX_REFERENCE = 1024;
// A $ starts a reference at the start of the text, after a space, a comma,
// or an opening bracket or quote.
const OPENERS = '([{<"\',';
const SPACE = /\s/;

function opens(text, i) {
  return i === 0 || SPACE.test(text[i - 1]) || OPENERS.includes(text[i - 1]);
}

function inRanges(ranges, i) {
  return ranges.some(([from, to]) => i >= from && i < to);
}

// codeRanges returns where text has code, as [from, to) pairs: fenced
// blocks, and code spans.
export function codeRanges(text) {
  const ranges = [];
  let fence = '';
  let fenceStart = 0;
  let offset = 0;
  for (const line of text.split(/(?<=\n)/)) {
    const trimmed = line.replace(/^ +/, '');
    if (line.length - trimmed.length <= 3) {
      const marker = /^(`{3,}|~{3,})/.exec(trimmed)?.[1] || '';
      if (!fence && marker) {
        fence = marker;
        fenceStart = offset;
      } else if (fence && marker.startsWith(fence) && !trimmed.slice(marker.length).trim()) {
        ranges.push([fenceStart, offset + line.length]);
        fence = '';
      }
    }
    offset += line.length;
  }
  if (fence) ranges.push([fenceStart, text.length]);
  const run = (at) => /^`+/.exec(text.slice(at))[0].length;
  for (let at = 0; at < text.length;) {
    const open = text.indexOf('`', at);
    if (open < 0) break;
    if (inRanges(ranges, open)) {
      at = open + 1;
      continue;
    }
    const length = run(open);
    let closed = false;
    for (let search = open + length; search < text.length;) {
      const next = text.indexOf('`', search);
      if (next < 0) break;
      const other = run(next);
      if (other === length) {
        ranges.push([open, next + other]);
        at = next + other;
        closed = true;
        break;
      }
      search = next + other;
    }
    if (!closed) at = open + length;
  }
  return ranges;
}

// linkQuery finds the reference being typed at the end of before, the text
// before the caret: { start } of its $, the path typed so far, and whether
// it is quoted; null where none is, or where its lines are being typed.
export function linkQuery(before) {
  const code = codeRanges(before);
  const quote = before.lastIndexOf('$"');
  if (quote >= 0 && opens(before, quote) && !inRanges(code, quote)) {
    const name = before.slice(quote + 2);
    if (!/["\n]/.test(name) && name.length <= MAX_REFERENCE) return { start: quote, query: name, quoted: true };
  }
  let word = before.length;
  while (word > 0 && !SPACE.test(before[word - 1])) word--;
  for (let at = word; at < before.length; at++) {
    if (before[at] !== '$' || !opens(before, at) || inRanges(code, at)) continue;
    const rest = before.slice(at + 1);
    if (rest.length > MAX_REFERENCE || rest.startsWith('"')) return null;
    const colon = rest.lastIndexOf(':');
    if (colon >= 0 && colon < rest.length - 1 && /^[0-9-]+$/.test(rest.slice(colon + 1))) return null;
    return { start: at, query: rest, quoted: false };
  }
  return null;
}

// linkLabel is how a prompt links a path: $path, or quoted where the path
// has spaces.
export function linkLabel(path) {
  return SPACE.test(path) && !path.includes('"') ? `$"${path}"` : `$${path}`;
}

// edit replaces text in a textarea the way typing does, so undo takes it
// back, and tells the page it changed.
function edit(input, start, end, text) {
  input.focus({ preventScroll: true });
  input.setSelectionRange(start, end);
  if (!document.execCommand('insertText', false, text)) {
    input.setRangeText(text, start, end, 'end');
    input.dispatchEvent(new Event('input', { bubbles: true }));
  }
}

// pathNode shows a path: its folder faint, its name bold.
function pathNode(path) {
  const trimmed = path.endsWith('/') ? path.slice(0, -1) : path;
  const cut = trimmed.lastIndexOf('/') + 1;
  return h('span', { class: 'file-path' },
    cut ? h('span', { class: 'file-dir', text: path.slice(0, cut) }) : null,
    h('b', { text: path.slice(cut) }));
}

// createCompleter completes the $ reference being typed in a textarea: box
// (with list in it) shows what it can complete to, which matches(query)
// fetches. Typing opens it; moving the caret away or Esc closes it. ↑↓
// choose, Tab completes, and so does Enter once a choice was made with
// the arrows or the pointer: otherwise Enter is the textarea's, so a
// prompt that ends in a path is sent as it is. A folder completed lists
// what is in it. key(event) takes the keys it handles; typed() and moved()
// say what the textarea did.
export function createCompleter({ input, box, list, matches, onChange = () => {} }) {
  const ui = { items: [], cursor: 0, token: null, open: false, moved: false, dismissed: null, seq: 0 };
  let timer = 0;

  function tokenAtCaret() {
    if (document.activeElement !== input || input.selectionStart !== input.selectionEnd) return null;
    return linkQuery(input.value.slice(0, input.selectionStart));
  }

  function close() {
    clearTimeout(timer);
    ui.seq++;
    if (!ui.open && !ui.items.length) return;
    ui.open = false;
    ui.items = [];
    ui.token = null;
    render();
  }

  // typed follows what is typed: a reference at the caret opens the list.
  function typed() {
    const token = tokenAtCaret();
    if (!token || ui.dismissed === input.value) return close();
    ui.dismissed = null;
    ui.token = token;
    clearTimeout(timer);
    timer = setTimeout(() => load(token), ui.open ? 60 : 0);
  }

  // moved follows the caret: it closes the list once the caret leaves the
  // reference, and follows it inside.
  function moved() {
    if (!ui.open) return;
    const token = tokenAtCaret();
    if (!token) return close();
    if (token.start !== ui.token?.start || token.query !== ui.token?.query) {
      ui.token = token;
      load(token);
    }
  }

  async function load(token) {
    const seq = ++ui.seq;
    let found = [];
    try {
      found = (await matches(token.query)) || [];
    } catch {
      found = [];
    }
    if (seq !== ui.seq || tokenAtCaret()?.start !== token.start) return;
    // A $ nothing completes, such as $HOME, is text: no list.
    ui.items = found;
    ui.cursor = 0;
    ui.moved = false;
    ui.open = found.length > 0;
    render();
  }

  function render() {
    box.hidden = !ui.open;
    if (!ui.open) {
      list.replaceChildren();
      onChange();
      return;
    }
    // Over the textarea, unless it is too near the top for that: a prompt
    // being edited high in the transcript.
    box.classList.toggle('below', input.getBoundingClientRect().top < Math.min(400, window.innerHeight * 0.45));
    list.replaceChildren(...ui.items.map((item, n) => h('li', {
      role: 'option', id: `${list.id}-${n}`, 'aria-selected': String(n === ui.cursor), data: { directory: item.directory ? 'true' : null },
      onmousemove: () => {
        if (ui.cursor !== n) {
          ui.cursor = n;
          ui.moved = true;
          render();
        }
      },
      onmousedown: (event) => {
        event.preventDefault(); // the textarea keeps the focus
        accept(n);
      },
    }, h('span', { class: 'file-glyph', 'aria-hidden': 'true', text: item.directory ? '▢' : '▤' }), pathNode(item.path),
    h('small', { text: item.directory ? 'folder' : '' }))));
    list.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' });
    onChange();
  }

  // accept puts choice n in place of the reference: a file with a space
  // after it, a folder so that completing goes on inside it.
  function accept(n) {
    const item = ui.items[n];
    const token = tokenAtCaret() || ui.token;
    if (!item || !token) return;
    const caret = input.selectionStart;
    const after = input.value.slice(caret);
    let end = caret + /^\S*/.exec(after)[0].length;
    if (token.quoted) {
      const closing = after.indexOf('"');
      end = closing >= 0 && !after.slice(0, closing).includes('\n') ? caret + closing + 1 : caret;
    }
    const label = linkLabel(item.path);
    const text = item.directory ? label : `${label} `;
    edit(input, token.start, end, text);
    if (item.directory && label.endsWith('"')) {
      // Inside the quotes, where the folder's entries complete.
      const at = token.start + label.length - 1;
      input.setSelectionRange(at, at);
    }
    close();
    if (item.directory) typed();
  }

  function key(event) {
    if (!ui.open || event.isComposing || event.altKey || event.metaKey) return false;
    const step = (by) => {
      event.preventDefault();
      ui.cursor = (ui.cursor + by + ui.items.length) % ui.items.length;
      ui.moved = true;
      render();
      return true;
    };
    const ctrl = event.ctrlKey && !event.shiftKey;
    switch (true) {
      case event.key === 'Escape':
        event.preventDefault();
        event.stopPropagation(); // not the first Esc of an Esc Esc
        ui.dismissed = input.value;
        close();
        return true;
      case event.key === 'ArrowDown' || (ctrl && event.key === 'n'):
        return step(1);
      case event.key === 'ArrowUp' || (ctrl && event.key === 'p'):
        return step(-1);
      case event.ctrlKey:
        return false;
      case event.key === 'Tab' && !event.shiftKey:
      case event.key === 'Enter' && !event.shiftKey && ui.moved:
        event.preventDefault();
        accept(ui.cursor);
        return true;
    }
    return false;
  }

  function destroy() {
    clearTimeout(timer);
    ui.seq++;
  }

  return { typed, moved, key, close, destroy, get open() { return ui.open; } };
}

// ---------------------------------------------------------------- sent prompts

// describeLink says what the model saw of a file a prompt linked.
export function describeLink(file) {
  const lines = (n) => `${n} line${n === 1 ? '' : 's'}`;
  if (file.error) return `not read: ${file.error}`;
  if (file.directory) {
    if (!file.lines) return 'empty folder';
    return file.to < file.lines ? `${file.to} of ${file.lines} entries` : `${file.lines} ${file.lines === 1 ? 'entry' : 'entries'}`;
  }
  if (file.binary) return `binary · ${fmt.bytes(file.size || 0)}`;
  if (!file.lines && !file.size) return 'empty';
  if (!file.from) return file.lines ? `${lines(file.lines)} · none shown` : `${fmt.bytes(file.size || 0)} · none shown`;
  if (file.from === 1 && file.to === file.lines) return `all ${lines(file.lines)}`;
  const range = file.from === file.to ? `line ${file.from}` : `lines ${file.from}–${file.to}`;
  return file.lines ? `${range} of ${file.lines}` : `${range} · ${fmt.bytes(file.size || 0)}`;
}

// snapshotNode shows what the model saw of a file: its lines, numbered, or
// a folder's entries.
export function snapshotNode(file) {
  const rows = (file.content || '').split('\n');
  if (!file.content) {
    return h('div', { class: 'file-view empty', text: describeLink(file) });
  }
  const width = String(file.from + rows.length - 1).length;
  return h('div', { class: 'file-view', data: { directory: file.directory ? 'true' : null } },
    h('pre', null, rows.map((row, n) => h('span', { class: 'file-line' },
      file.directory ? null : h('i', { 'aria-hidden': 'true', text: String(file.from + n).padStart(width, ' ') }),
      row || ' '))),
    (file.to < file.lines || file.from > 1) ? h('footer', null, `${describeLink(file)} · the model reads the rest from the file itself`) : null);
}
