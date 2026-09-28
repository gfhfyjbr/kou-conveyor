// explorer: the workspace's files, as a tab of the sidebar ("sidebar.tab"
// files). Its folders show as a tree, a folder read when it opens, marked
// with what git sees changed (M A D R U ?) and dimmed where git ignores
// them; "Go to file" finds one by name. A file shows below the tree with
// its lines numbered and highlighted by chroma on the server (explorer.go):
// the start of a large file comes at once and the rest of its colours
// follow, and only the lines in view are drawn. A line number clicked (and
// another with Shift) picks lines, which "$" links in the prompt; a picture
// shows as one. What the agent changes shows as it changes it.
import { createCode } from './code.js';

const ICON = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M1.5 3.5h4.5l1.5 1.5h7v8.5h-13z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="miter"/></svg>';
const REFRESH = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M13 8a5 5 0 1 1-1.6-3.7M13 2.5v3h-3" fill="none" stroke="currentColor" stroke-width="1.4"/></svg>';
const COLLAPSE = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 6l4-3 4 3M4 10l4 3 4-3" fill="none" stroke="currentColor" stroke-width="1.4"/></svg>';
const TREE = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2.5 3h11M5.5 7h8M5.5 11h8M2.5 3v8h3" fill="none" stroke="currentColor" stroke-width="1.4"/></svg>';
const GIT = { M: 'modified', A: 'added', D: 'deleted', R: 'renamed', U: 'conflict', '?': 'untracked', T: 'type changed', C: 'copied' };

// Files by the colour of their kind, as the tree marks them.
const KINDS = [
  [/\.(go|mod|sum)$/i, 'go'], [/\.(m?js|cjs|jsx)$/i, 'js'], [/\.(ts|tsx|mts)$/i, 'ts'], [/\.(css|scss|less)$/i, 'css'],
  [/\.(html?|svg|xml)$/i, 'markup'], [/\.(md|mdx|txt|rst)$/i, 'text'], [/\.(json|ya?ml|toml|ini|env|conf)$/i, 'data'],
  [/\.(sh|bash|zsh|fish)$|^(makefile|dockerfile)$/i, 'shell'], [/\.(py|rb|php|pl|lua)$/i, 'script'], [/\.(rs|c|h|cc|cpp|hpp|zig|swift|kt|java|cs)$/i, 'native'],
  [/\.(png|jpe?g|gif|webp|bmp|ico|avif)$/i, 'image'], [/\.(lock|sum)$/i, 'lock'],
];
const kindOf = (name) => KINDS.find(([pattern]) => pattern.test(name))?.[1] || 'other';

export default function activate(cockpit) {
  const { h, svg, fmt } = cockpit;
  const service = (name) => (cockpit.has(name) ? cockpit.use(name) : null);
  const views = new Set();

  // prompt links a path in the composer, where its cursor is.
  function linkInPrompt(path, lines) {
    const composer = service('composer');
    const input = composer?.input;
    if (!input) return cockpit.toast('The composer plugin is off.', 'error');
    const label = (service('files')?.label?.(path) || `$${path}`) + (lines ? `:${lines.from}${lines.to > lines.from ? `-${lines.to}` : ''}` : '');
    const start = input.selectionStart ?? input.value.length;
    const end = input.selectionEnd ?? start;
    const before = input.value.slice(0, start);
    const text = `${before && !/\s$/.test(before) ? ' ' : ''}${label} `;
    input.focus({ preventScroll: true });
    input.setSelectionRange(start, end);
    if (!document.execCommand('insertText', false, text)) {
      input.setRangeText(text, start, end, 'end');
      input.dispatchEvent(new Event('input', { bubbles: true }));
    }
    cockpit.toast(`${label} is in the prompt`);
  }

  function absolute(path) {
    const root = service('session')?.currentWorkspace?.()?.path || '';
    return path && path !== '.' ? `${root}/${path}` : root;
  }

  // terminalAt opens a terminal in a folder of the workspace.
  function terminalAt(dir) {
    const bar = service('sidebar');
    if (!bar || !bar.kinds().some((k) => k.id === 'terminal')) return cockpit.toast('The terminal plugin is off.', 'error');
    bar.open('terminal', { state: { layout: { t: 'pane', id: null, cwd: absolute(dir) } } });
  }

  // ---------------------------------------------------------------- a Files tab

  function createFiles(tab) {
    const saved = tab.state || {};
    const state = {
      expanded: new Set(Array.isArray(saved.expanded) ? saved.expanded : []),
      file: typeof saved.file === 'string' ? saved.file : null,
      split: Number.isFinite(saved.split) ? Math.min(0.85, Math.max(0.12, saved.split)) : 0.42,
      tree: saved.tree !== false,
      wrap: !!saved.wrap,
    };
    const dirs = new Map(); // path → { entries, loading, error, truncated }
    let visible = false;
    let disposed = false;

    const search = h('input', { class: 'fx-search', type: 'search', placeholder: 'Go to file…', spellcheck: 'false', autocomplete: 'off', 'aria-label': 'Go to file' });
    const treeToggle = h('button', { class: 'icon small', type: 'button', title: 'Show or hide the tree', 'aria-label': 'Toggle the tree', 'aria-pressed': 'true', onclick: () => setTree(!state.tree) }, svg(TREE));
    const bar = h('div', { class: 'fx-bar' },
      h('label', { class: 'search fx-find' }, search),
      h('button', { class: 'icon small', type: 'button', title: 'Read the folders again', 'aria-label': 'Refresh', onclick: () => refresh() }, svg(REFRESH)),
      h('button', { class: 'icon small', type: 'button', title: 'Close every folder', 'aria-label': 'Collapse all', onclick: () => { state.expanded.clear(); persist(); drawTree(); } }, svg(COLLAPSE)),
      treeToggle);
    const tree = h('div', { class: 'fx-tree', role: 'tree', 'aria-label': 'Files of the workspace', tabindex: '0' });
    const results = h('div', { class: 'fx-results', role: 'listbox', hidden: true });
    const upper = h('div', { class: 'fx-upper' }, tree, results);
    const divider = h('div', { class: 'fx-divider', role: 'separator', 'aria-orientation': 'horizontal', tabindex: '0', title: 'Drag to resize' });
    const fileHead = h('div', { class: 'fx-head' });
    const fileBody = h('div', { class: 'fx-body' });
    const lower = h('div', { class: 'fx-file' }, fileHead, fileBody);
    const root = h('div', { class: 'fx' }, bar, upper, divider, lower);

    function persist() {
      tab.save({ expanded: [...state.expanded], file: state.file, split: state.split, tree: state.tree, wrap: state.wrap });
    }

    function layoutSplit() {
      const open = !!state.file;
      root.classList.toggle('with-file', open);
      root.classList.toggle('no-tree', open && !state.tree);
      root.style.setProperty('--split', `${(state.split * 100).toFixed(2)}%`);
      treeToggle.setAttribute('aria-pressed', String(state.tree || !open));
      treeToggle.disabled = !open;
    }

    function setTree(on) {
      state.tree = on;
      persist();
      layoutSplit();
    }

    // Dragging the divider shares the height between the tree and the file.
    divider.addEventListener('pointerdown', (event) => {
      if (event.button !== 0) return;
      event.preventDefault();
      divider.setPointerCapture(event.pointerId);
      const box = root.getBoundingClientRect();
      const top = box.top + bar.getBoundingClientRect().height;
      const move = (e) => {
        state.split = Math.min(0.85, Math.max(0.12, (e.clientY - top) / (box.bottom - top)));
        layoutSplit();
      };
      const up = () => {
        divider.removeEventListener('pointermove', move);
        divider.removeEventListener('pointerup', up);
        persist();
      };
      divider.addEventListener('pointermove', move);
      divider.addEventListener('pointerup', up);
    });
    divider.addEventListener('keydown', (event) => {
      const by = { ArrowUp: -0.05, ArrowDown: 0.05 }[event.key];
      if (!by) return;
      event.preventDefault();
      state.split = Math.min(0.85, Math.max(0.12, state.split + by));
      layoutSplit();
      persist();
    });

    // ---------------------------------------------------------------- the tree

    async function load(path) {
      const dir = dirs.get(path) || { entries: null };
      dirs.set(path, dir);
      dir.loading = true;
      try {
        const data = await cockpit.api(`/tree?path=${encodeURIComponent(path)}`);
        dir.entries = data.entries || [];
        dir.truncated = !!data.truncated;
        dir.error = '';
      } catch (error) {
        dir.error = error.status === 404 ? 'gone' : error.message;
        if (error.status === 404 && path !== '.') state.expanded.delete(path);
      }
      dir.loading = false;
      return dir;
    }

    async function refresh() {
      const paths = ['.', ...[...state.expanded].sort()];
      await Promise.all(paths.map((path) => load(path)));
      if (disposed) return;
      drawTree();
      if (state.file) openFile(state.file, { keep: true });
    }

    const child = (dir, name) => (dir === '.' ? name : `${dir}/${name}`);
    let focused = null;
    function drawTree() {
      const rows = [];
      const walk = (path, depth) => {
        const dir = dirs.get(path);
        if (!dir?.entries) {
          if (dir?.error && depth === 0) rows.push(h('div', { class: 'fx-note', text: dir.error }));
          return;
        }
        for (const entry of dir.entries) {
          const full = child(path, entry.name);
          const open = entry.dir && state.expanded.has(full);
          const row = h('div', {
            class: 'fx-row', role: 'treeitem', tabindex: '-1',
            data: {
              path: full, dir: entry.dir ? 'true' : null, kind: entry.dir ? null : kindOf(entry.name),
              ignored: entry.ignored ? 'true' : null, git: entry.git || null, changed: entry.changed ? 'true' : null,
              current: full === state.file ? 'true' : null,
            },
            'aria-expanded': entry.dir ? String(open) : null,
            'aria-level': String(depth + 1),
            title: [full, entry.git ? GIT[entry.git] : '', entry.ignored ? 'ignored by git' : '', entry.link ? (entry.broken ? 'a link that leads out of the workspace' : 'a link') : '', entry.size ? fmt.bytes(entry.size) : ''].filter(Boolean).join(' · '),
          },
          h('span', { class: entry.dir ? 'chev' : 'fx-dot', 'aria-hidden': 'true' }),
          h('span', { class: 'fx-name', text: entry.name + (entry.dir ? '/' : '') }),
          entry.link ? h('span', { class: 'fx-link', text: '↗' }) : null,
          h('span', { class: 'fx-mark', text: entry.git || (entry.changed ? '•' : '') }));
          row.style.setProperty('--depth', String(depth));
          rows.push(row);
          if (open) {
            const inner = dirs.get(full);
            if (!inner?.entries && !inner?.loading) load(full).then(() => { if (!disposed) drawTree(); });
            if (!inner?.entries) rows.push(h('div', { class: 'fx-note', text: inner?.error || 'Reading…' }));
            else walk(full, depth + 1);
          }
        }
        if (dir.truncated) rows.push(h('div', { class: 'fx-note', text: 'The folder holds more than is shown.' }));
      };
      walk('.', 0);
      if (!rows.length && dirs.get('.')?.entries) rows.push(h('div', { class: 'fx-note', text: 'The workspace is empty.' }));
      tree.replaceChildren(...rows);
      if (focused) tree.querySelector(`[data-path="${CSS.escape(focused)}"]`)?.setAttribute('tabindex', '0');
    }

    function toggleDir(path, open = !state.expanded.has(path)) {
      if (open) state.expanded.add(path);
      else {
        state.expanded.delete(path);
        for (const other of [...state.expanded]) if (other.startsWith(`${path}/`)) state.expanded.delete(other);
      }
      persist();
      drawTree();
    }

    tree.addEventListener('click', (event) => {
      const row = event.target instanceof Element ? event.target.closest('.fx-row') : null;
      if (!row) return;
      focused = row.dataset.path;
      if (row.dataset.dir) toggleDir(row.dataset.path);
      else openFile(row.dataset.path);
    });
    tree.addEventListener('keydown', (event) => {
      const rows = [...tree.querySelectorAll('.fx-row')];
      if (!rows.length) return;
      let at = rows.findIndex((row) => row.dataset.path === focused);
      const row = rows[at];
      const go = (n) => {
        const next = rows[Math.max(0, Math.min(rows.length - 1, n))];
        focused = next.dataset.path;
        rows.forEach((r) => r.setAttribute('tabindex', '-1'));
        next.setAttribute('tabindex', '0');
        next.focus();
        next.scrollIntoView({ block: 'nearest' });
      };
      switch (event.key) {
        case 'ArrowDown': event.preventDefault(); go(at + 1); break;
        case 'ArrowUp': event.preventDefault(); go(at < 0 ? 0 : at - 1); break;
        case 'ArrowRight':
          event.preventDefault();
          if (row?.dataset.dir && !state.expanded.has(row.dataset.path)) toggleDir(row.dataset.path, true);
          else go(at + 1);
          break;
        case 'ArrowLeft': {
          event.preventDefault();
          if (row?.dataset.dir && state.expanded.has(row.dataset.path)) toggleDir(row.dataset.path, false);
          else if (row) {
            const parent = row.dataset.path.split('/').slice(0, -1).join('/');
            if (parent) go(rows.findIndex((r) => r.dataset.path === parent));
          }
          break;
        }
        case 'Enter':
          event.preventDefault();
          if (row?.dataset.dir) toggleDir(row.dataset.path);
          else if (row) openFile(row.dataset.path);
          break;
        default: return;
      }
      at = -1;
    });
    tree.addEventListener('contextmenu', (event) => {
      const row = event.target instanceof Element ? event.target.closest('.fx-row') : null;
      if (!row) return;
      event.preventDefault();
      const path = row.dataset.path;
      const isDir = !!row.dataset.dir;
      service('menu')?.open(row, [
        !isDir ? { icon: '▸', label: 'Open', run: () => openFile(path) } : null,
        { icon: '$', label: 'Link in the prompt', detail: service('files')?.label?.(path + (isDir ? '/' : '')) || `$${path}`, run: () => linkInPrompt(path + (isDir ? '/' : '')) },
        { icon: '⧉', label: 'Copy the path', detail: path, run: () => cockpit.copy(path, 'Path copied') },
        { icon: '⧉', label: 'Copy the full path', run: () => cockpit.copy(absolute(path), 'Path copied') },
        { separator: true },
        { icon: '❯', label: 'Terminal here', run: () => terminalAt(isDir ? path : path.split('/').slice(0, -1).join('/') || '.') },
      ]);
    });

    // ---------------------------------------------------------------- finding a file

    let searchTicket = 0;
    let searchTimer = 0;
    let found = [];
    let pick = 0;
    search.addEventListener('input', () => {
      clearTimeout(searchTimer);
      const query = search.value.trim();
      if (!query) {
        results.hidden = true;
        tree.hidden = false;
        return;
      }
      searchTimer = setTimeout(async () => {
        const ticket = ++searchTicket;
        let data;
        try {
          data = await cockpit.api(`/files?q=${encodeURIComponent(query)}&limit=60`);
        } catch {
          return;
        }
        if (ticket !== searchTicket || disposed) return;
        found = data.files || [];
        pick = 0;
        drawResults();
      }, 90);
    });
    search.addEventListener('keydown', (event) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        search.value = '';
        results.hidden = true;
        tree.hidden = false;
        tree.focus();
      } else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault();
        pick = Math.max(0, Math.min(found.length - 1, pick + (event.key === 'ArrowDown' ? 1 : -1)));
        drawResults();
      } else if (event.key === 'Enter') {
        event.preventDefault();
        choose(found[pick]);
      }
    });

    function drawResults() {
      tree.hidden = true;
      results.hidden = false;
      results.replaceChildren(...(found.length ? found.map((match, n) => {
        const path = match.path.replace(/\/$/, '');
        const name = path.split('/').pop();
        const dir = path.split('/').slice(0, -1).join('/');
        return h('button', {
          class: 'fx-result', type: 'button', role: 'option', 'aria-selected': String(n === pick),
          data: { kind: match.directory ? null : kindOf(name) }, onclick: () => choose(match),
        }, h('span', { class: 'fx-name', text: name + (match.directory ? '/' : '') }), h('small', { text: dir }));
      }) : [h('div', { class: 'fx-note', text: 'No file by that name.' })]));
      results.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' });
    }

    async function choose(match) {
      if (!match) return;
      const path = match.path.replace(/\/$/, '');
      search.value = '';
      results.hidden = true;
      tree.hidden = false;
      // The folders on the way open.
      const parts = path.split('/');
      for (let i = 1; i < parts.length + (match.directory ? 1 : 0); i++) state.expanded.add(parts.slice(0, i).join('/'));
      persist();
      await Promise.all([...state.expanded].filter((p) => !dirs.get(p)?.entries).map((p) => load(p)));
      focused = path;
      drawTree();
      tree.querySelector(`[data-path="${CSS.escape(path)}"]`)?.scrollIntoView({ block: 'center' });
      if (!match.directory) openFile(path);
    }

    // ---------------------------------------------------------------- a file

    let code = null;
    let fileTicket = 0;
    let shown = null; // what the file view shows: { path, version }
    async function openFile(path, { keep = false } = {}) {
      const ticket = ++fileTicket;
      const first = !keep || shown?.path !== path;
      state.file = path;
      persist();
      layoutSplit();
      for (const row of tree.querySelectorAll('.fx-row[data-current]')) delete row.dataset.current;
      tree.querySelector(`[data-path="${CSS.escape(path)}"]`)?.setAttribute('data-current', 'true');
      if (first) {
        fileHead.replaceChildren(head(path, null));
        fileBody.replaceChildren(h('div', { class: 'fx-note', text: 'Reading…' }));
      }
      let data;
      try {
        data = await cockpit.api(`/file?path=${encodeURIComponent(path)}`);
      } catch (error) {
        if (ticket !== fileTicket) return;
        fileHead.replaceChildren(head(path, null));
        fileBody.replaceChildren(h('div', { class: 'fx-note', text: error.status === 404 ? 'The file is gone.' : error.message }));
        shown = null;
        code = null;
        return;
      }
      if (ticket !== fileTicket || disposed) return;
      if (keep && shown?.path === path && shown.version === data.version) return; // it did not change
      const scroll = keep && shown?.path === path && code ? code.node.scrollTop : 0;
      const lines = keep && shown?.path === path ? code?.selection() : null;
      shown = { path, version: data.version };
      fileHead.replaceChildren(head(path, data));
      if (data.image) {
        code = null;
        fileBody.replaceChildren(h('div', { class: 'fx-picture' },
          h('img', { src: cockpit.wsPath(cockpit.host.workspace(), `/raw?path=${encodeURIComponent(path)}&v=${encodeURIComponent(data.version)}`), alt: path })));
        return;
      }
      if (data.binary || data.too_large) {
        code = null;
        fileBody.replaceChildren(h('div', { class: 'fx-note', text: data.binary ? `A binary file of ${fmt.bytes(data.size)}; its content is not shown.` : `A file of ${fmt.bytes(data.size)}: too large to show here. Open it in a terminal.` }));
        return;
      }
      code = createCode(h, data.text || '', data.runs || [], data.classes || [], {
        wrap: state.wrap,
        onLine: () => fileHead.replaceChildren(head(path, data)),
      });
      fileBody.replaceChildren(code.node);
      code.draw(true);
      if (lines) code.select(lines.from, lines.to);
      if (scroll) code.node.scrollTop = scroll;
      if (!data.complete) {
        // The rest of the colours follow.
        cockpit.api(`/file?path=${encodeURIComponent(path)}&tokens=1&version=${encodeURIComponent(data.version)}`).then((more) => {
          if (ticket === fileTicket && code && more?.runs) code.highlight(more.runs);
        }, () => {});
      }
    }

    function head(path, data) {
      const lines = code?.selection();
      const meta = !data ? '' : data.image ? `${data.image.replace('image/', '').toUpperCase()} · ${fmt.bytes(data.size)}`
        : data.binary || data.too_large ? fmt.bytes(data.size)
          : [data.language || 'text', `${code?.lines ?? ''} lines`, fmt.bytes(data.size)].filter(Boolean).join(' · ');
      const name = path.split('/').pop();
      const dir = path.split('/').slice(0, -1).join('/');
      return h('div', { class: 'fx-headrow' },
        h('button', { class: 'fx-path', type: 'button', title: `${path} — click to copy`, onclick: () => cockpit.copy(path, 'Path copied') },
          dir ? h('span', { class: 'fx-dir', text: `${dir}/` }) : null, h('b', { text: name })),
        h('span', { class: 'fx-meta', text: meta }),
        h('span', { class: 'fx-actions' },
          h('button', {
            class: 'act strong', type: 'button', title: lines ? 'Link these lines in the prompt' : 'Link the file in the prompt (click a line number, Shift-click another, to pick lines)',
            onclick: () => linkInPrompt(path, lines),
          }, lines ? `$ ${lines.from}${lines.to > lines.from ? `–${lines.to}` : ''}` : '$ Link'),
          code ? h('button', { class: 'act', type: 'button', title: lines ? 'Copy the lines picked' : 'Copy the text', onclick: () => cockpit.copy(code.text(), lines ? 'Lines copied' : 'Text copied') }, 'Copy') : null,
          code?.whole ? h('button', { class: 'act', type: 'button', 'aria-pressed': String(state.wrap), title: 'Wrap long lines', onclick: () => { state.wrap = !state.wrap; code?.setWrap(state.wrap); persist(); fileHead.replaceChildren(head(path, data)); } }, 'Wrap') : null,
          h('button', { class: 'icon small', type: 'button', title: 'Close the file', 'aria-label': 'Close the file', onclick: closeFile }, '×')));
    }

    function closeFile() {
      fileTicket++;
      state.file = null;
      shown = null;
      code = null;
      persist();
      fileHead.replaceChildren();
      fileBody.replaceChildren();
      layoutSplit();
      drawTree();
    }

    // ---------------------------------------------------------------- following changes

    let changedTimer = 0;
    const changed = () => {
      if (!visible) return;
      clearTimeout(changedTimer);
      changedTimer = setTimeout(() => refresh(), 400);
    };
    const offs = [
      cockpit.on('session:changes', changed),
      cockpit.on('finish', changed),
    ];

    layoutSplit();
    const view = {
      node: root,
      title: 'Files',
      shown() {
        visible = true;
        refresh();
      },
      hidden() { visible = false; },
      focus: () => (search.value ? search : tree).focus(),
      dispose() {
        disposed = true;
        clearTimeout(changedTimer);
        clearTimeout(searchTimer);
        for (const off of offs) off();
        views.delete(view);
      },
      openFile,
      find: () => { search.focus(); search.select(); },
    };
    if (state.file) queueMicrotask(() => openFile(state.file));
    views.add(view);
    return view;
  }

  cockpit.contribute('sidebar.tab', {
    id: 'files', title: 'Files', icon: ICON, order: 30, key: 'F',
    description: 'The workspace\'s files as a tree, with what git sees changed, and any file highlighted — the start of a large one at once.',
    create: createFiles,
  });
  cockpit.onDispose(() => { for (const view of [...views]) cockpit.safely(() => view.dispose()); });

  // open shows a file of the workspace in a Files tab.
  function open(path) {
    const bar = service('sidebar');
    if (!bar) return cockpit.toast('Files need the sidebar plugin, which is off.', 'error');
    const existing = bar.tabs().find((t) => t.kind === 'files');
    if (existing) {
      bar.activate(existing.id, { focus: !path });
      if (path) bar.view(existing.id)?.openFile?.(path);
      return existing.id;
    }
    return bar.open('files', { state: path ? { file: path } : null });
  }
  cockpit.provide('explorer', { open });

  cockpit.keys.register({ key: ['f', 'а'], views: ['sessions'], run: () => { const id = open(); service('sidebar')?.view(id)?.find?.(); } });
  cockpit.contribute('help.keys', { keys: ['F'], text: 'Files', order: 217.5 });
  cockpit.commands.register({
    name: 'files', args: '[path]', help: 'Show the workspace\'s files, or open one', order: 217,
    complete: () => [],
    run: (arg) => open(String(arg || '').trim().replace(/^\$/, '').replace(/^\.\//, '') || null),
  });
  cockpit.contribute('palette.provider', {
    id: 'explorer', order: 159,
    items: () => [{ group: 'Sidebar', icon: '▤', label: 'Files', hint: 'F', order: 159, run: () => open() }],
  });

  // Settings: nothing yet but what the files show.
  cockpit.contribute('settings.section', {
    id: 'files', title: 'Files', order: 30,
    render: () => h('p', { class: 'set-note', text: 'Files are highlighted on the server with chroma, cached by their version: a large file shows its start at once and the rest of its colours follow; one over 8 MB is not shown. Git\'s ignored files are dimmed, and what git sees changed is marked.' }),
  });
}
