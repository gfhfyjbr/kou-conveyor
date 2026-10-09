// files: files in prompts (links.js). A $ links a file or a folder of the
// workspace: "$cmd/main.go", "$cmd/main.go:120-160" for those lines, "$cmd/"
// for a folder's entries, "$~/notes.md" or "$/etc/hosts" outside it, and
// $"a b.txt" for a path with spaces. Typing $ lists the workspace's files
// over the composer: typing filters, ↑↓ choose, Tab completes (Enter too,
// once a choice was made), Esc closes, and a folder completed lists what is
// in it. The runner reads what the model sees of each link as the prompt
// runs — the lines asked for, else the beginning of the file, the whole of
// a small one — and the model reads the rest from the file itself. A strip
// above the composer's text shows what it links; under a sent prompt its
// files open to what the model saw. It provides the files service:
// attach({ input, container }) for other textareas, such as a prompt being
// edited; complete(query), links(text), query(before) and label(path).
import { createCompleter, describeLink, linkLabel, linkQuery, snapshotNode } from './links.js';

export default function activate(cockpit) {
  const { h, fmt } = cockpit;
  const session = cockpit.use('session');
  const composer = cockpit.use('composer');
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const view = () => session.view?.();
  const hot = cockpit.hot.data;
  // Kept across versions: what the model saw of the files of prompts, by
  // session and message, and which of a prompt's files are shown open.
  hot.snapshots ??= new Map();
  hot.open ??= new Set();

  const complete = (query) => cockpit.api(`/files?q=${encodeURIComponent(query)}&limit=60`).then((data) => data?.files || []);
  const links = (text) => cockpit.api('/links', { method: 'POST', body: { text } }).then((data) => data?.links || []);

  // ---------------------------------------------------------------- completing

  let boxes = 0;
  function suggestions() {
    const list = h('ul', { id: `file-suggestions-${++boxes}`, role: 'listbox', 'aria-label': 'Files of the workspace' });
    const keys = h('footer', null,
      h('span', null, h('kbd', { text: '↑' }), h('kbd', { text: '↓' }), ' choose'),
      h('span', null, h('kbd', { text: 'Tab' }), ' complete'),
      h('span', null, h('kbd', { text: 'Esc' }), ' close'),
      h('span', { class: 'files-note', text: '$path:10-40 links lines' }));
    return { box: h('div', { class: 'files-suggest', hidden: true }, list, keys), list };
  }

  const suggest = suggestions();
  suggest.box.id = 'file-suggestions';
  cockpit.ui.mount('composer.above', { id: 'file-suggestions', order: 12, node: suggest.box });

  // The composer loaded anew has a new textarea.
  let attached = null;
  function attachComposer() {
    const input = composer.input;
    if (!input || attached?.input === input) return;
    attached?.ctl.destroy();
    attached = { input, ctl: createCompleter({ input, box: suggest.box, list: suggest.list, matches: complete }) };
  }
  cockpit.on('composer:ready', attachComposer);
  cockpit.on('service', (name) => { if (name === 'composer') attachComposer(); });
  attachComposer();
  cockpit.onDispose(() => attached?.ctl.destroy());
  const ctl = () => (attached?.input === composer.input ? attached.ctl : null);

  // Its keys come before the commands' and the composer's own.
  cockpit.hooks.tap('composer.key', (event) => !!ctl()?.key(event), { order: -1 });
  cockpit.on('composer:input', () => {
    ctl()?.typed();
    scheduleStrip();
  });
  cockpit.on('composer:blur', () => ctl()?.close());
  cockpit.listen(document, 'selectionchange', () => {
    if (attached && document.activeElement === attached.input) attached.ctl.moved();
  });

  // attach completes references in another textarea: container holds the
  // list, over the textarea. Call it before adding keys of the textarea's
  // own, which the list's keys come before.
  function attach({ input, container }) {
    const { box, list } = suggestions();
    container.prepend(box);
    const completer = createCompleter({ input, box, list, matches: complete });
    const onKey = (event) => {
      if (completer.key(event)) event.stopImmediatePropagation();
    };
    const onSelection = () => {
      if (document.activeElement === input) completer.moved();
    };
    input.addEventListener('keydown', onKey);
    input.addEventListener('input', completer.typed);
    input.addEventListener('blur', completer.close);
    document.addEventListener('selectionchange', onSelection);
    return {
      destroy() {
        completer.destroy();
        input.removeEventListener('keydown', onKey);
        input.removeEventListener('input', completer.typed);
        input.removeEventListener('blur', completer.close);
        document.removeEventListener('selectionchange', onSelection);
        box.remove();
      },
    };
  }

  // ---------------------------------------------------------------- the strip

  // The strip says what the composer's text links, as the server reads it:
  // a $ that names nothing is no link.
  const strip = h('div', { class: 'links', id: 'links', hidden: true });
  cockpit.ui.mount('composer.above', { id: 'links', order: 22, node: strip });
  const state = { timer: 0, seq: 0, text: null };
  cockpit.onDispose(() => clearTimeout(state.timer));

  function scheduleStrip() {
    clearTimeout(state.timer);
    state.timer = setTimeout(refreshStrip, 250);
  }

  async function refreshStrip() {
    const text = composer.value?.() || '';
    if (text === state.text) return;
    const seq = ++state.seq;
    let found = [];
    if (text.includes('$')) {
      try {
        found = await links(text);
      } catch {
        return; // the server says nothing now; the next key asks again
      }
    }
    if (seq !== state.seq) return;
    state.text = text;
    showStrip(found);
  }

  function linkDetail(link) {
    if (link.directory) return 'folder';
    if (link.start_line && link.end_line) return link.start_line === link.end_line ? `line ${link.start_line}` : `lines ${link.start_line}–${link.end_line}`;
    if (link.start_line) return `from line ${link.start_line}`;
    return fmt.bytes(link.size || 0);
  }

  function showStrip(found) {
    const was = !strip.hidden;
    strip.hidden = !found.length;
    strip.replaceChildren(...found.map((link) => h('button', {
      class: 'link-chip', type: 'button', data: { directory: link.directory ? 'true' : null },
      title: `${link.path} — ${link.directory ? 'the model sees its entries' : link.start_line ? 'the model sees these lines' : 'the model sees its beginning, or all of a small file'}, and reads the rest itself. A click goes to it in the text.`,
      onmousedown: (event) => event.preventDefault(), // the composer keeps the focus
      onclick: () => caretAfter(link.label),
    }, h('span', { class: 'file-glyph', 'aria-hidden': 'true', text: link.directory ? '▢' : '▤' }),
    h('span', { class: 'link-label', text: link.label }), h('small', { text: linkDetail(link) }))));
    if (was !== !strip.hidden) composer.autosize?.();
  }

  // caretAfter puts the composer's caret right after a reference.
  function caretAfter(label) {
    const input = composer.input;
    const at = input?.value.indexOf(label);
    if (!input || at < 0) return;
    input.focus();
    input.setSelectionRange(at + label.length, at + label.length);
  }

  // ---------------------------------------------------------------- sent prompts

  // snapshots returns what the model saw of the files a prompt linked,
  // loading it the first time it is asked for.
  function snapshots(v, entry) {
    const message = entry.id.replace(/^input:/, '');
    const key = `${v.ws || ''}/${v.id}/${message}`;
    let got = hot.snapshots.get(key);
    if (!got) {
      got = { files: null, error: '' };
      hot.snapshots.set(key, got);
      cockpit.api(cockpit.wsPath(v.ws, `/sessions/${encodeURIComponent(v.id)}/files/${encodeURIComponent(message)}`))
        .then((data) => { got.files = data?.files || []; })
        .catch((error) => {
          got.error = error?.message || 'could not load it';
          hot.snapshots.delete(key); // asked again when it is opened again
        })
        .finally(() => service('timeline')?.schedule?.(entry.id));
    }
    return got;
  }

  function toggle(entry, n) {
    const key = `${entry.id}#${n}`;
    if (hot.open.has(key)) hot.open.delete(key);
    else hot.open.add(key);
    service('timeline')?.schedule?.(entry.id);
  }

  function promptFiles(entry, ctx) {
    if (!entry.files?.length || entry.state === 'pending') return null;
    const v = ctx.view;
    const opened = [];
    const chips = entry.files.map((file, n) => {
      const open = hot.open.has(`${entry.id}#${n}`);
      if (open) opened.push(n);
      return h('button', {
        class: 'prompt-file', type: 'button', 'aria-expanded': String(open),
        data: { directory: file.directory ? 'true' : null, failed: file.error ? 'true' : null },
        title: `${file.path} — the model saw ${describeLink(file)}${file.error ? '' : '. A click shows it.'}`,
        onclick: (event) => {
          event.stopPropagation();
          toggle(entry, n);
        },
      }, h('span', { class: 'file-glyph', 'aria-hidden': 'true', text: file.directory ? '▢' : '▤' }),
      h('span', { class: 'link-label', text: file.label }), h('small', { text: describeLink(file) }));
    });
    const node = h('div', { class: 'prompt-files' }, h('div', { class: 'prompt-file-chips' }, chips));
    if (opened.length) {
      const got = snapshots(v, entry);
      for (const n of opened) {
        const file = entry.files[n];
        const seen = got.files?.[n];
        const body = got.error ? h('div', { class: 'file-view empty', text: `Could not load it: ${got.error}` })
          : seen ? snapshotNode(seen) : h('div', { class: 'file-view empty', text: 'Loading…' });
        node.append(h('section', { class: 'prompt-file-open' },
          h('header', null, h('span', { class: 'file-glyph', 'aria-hidden': 'true', text: file.directory ? '▢' : '▤' }),
            h('b', { text: file.path }), h('small', { text: `as the model saw it · ${describeLink(file)}` }),
            seen?.content ? ctx.button('Copy', () => ctx.copy(seen.content, 'Lines copied'), 'Copy what the model saw') : null,
            ctx.button('Close', () => toggle(entry, n), 'Close')),
          body));
      }
    }
    return node;
  }

  cockpit.contribute('timeline.decoration', { kinds: ['user'], order: 12, render: promptFiles });

  // ---------------------------------------------------------------- what others use

  cockpit.provide('files', { attach, complete, links, query: linkQuery, label: linkLabel });
  cockpit.contribute('help.keys', { keys: ['$'], text: 'Link a file or folder of the workspace: typing lists them · Tab completes · $path:10-40 links those lines · the model reads the rest itself', order: 46 });

  // Loaded anew, it shows what the composer's text links; the composer says
  // what $ does.
  scheduleStrip();
  cockpit.render();
}
